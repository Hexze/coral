use std::collections::HashMap;

use blacklist::{parse_replay, Replay};
use serenity::all::*;

use crate::utils::format_uuid_dashed;
use super::*;


#[derive(Debug, Clone)]
pub struct PlayerEntry {
    pub username: String,
    pub uuid: String,
    pub tag_type: String,
    pub reason: String,
    pub is_nicked: bool,
    pub status: PlayerStatus,
    pub reviewer: Option<String>,
    pub review_note: Option<String>,
    pub evidence: Vec<Evidence>,
    pub conflict_warning: Option<String>,
    pub accept_votes: Vec<u64>,
    pub reject_votes: Vec<u64>,
}


#[derive(Debug, Clone, PartialEq)]
pub enum PlayerStatus {
    Pending,
    Approved,
    Rejected,
}


#[derive(Debug, Clone)]
pub enum Evidence {
    Replay { replay: Replay, note: Option<String> },
    Attachment { filename: String },
}


#[derive(Debug, Clone)]
pub struct PendingAdd {
    pub identifier: String,
    pub username: String,
    pub is_nicked: bool,
}


#[derive(Debug, Clone)]
pub struct SubmissionState {
    pub submitter_id: u64,
    pub players: Vec<PlayerEntry>,
    pub submitted: bool,
    pub editing: Option<usize>,
    pub pending_add: Option<PendingAdd>,
}


pub struct ForumTags {
    pub pending: Option<ForumTagId>,
    pub approved: Option<ForumTagId>,
    pub rejected: Option<ForumTagId>,
    pub nicked: Option<ForumTagId>,
    pub awaiting_evidence: Option<ForumTagId>,
}


pub struct ConfirmationData {
    pub player_name: String,
    pub player_uuid: String,
    pub tag_type: String,
    pub reason: String,
    pub is_nicked: bool,
}


pub fn parse_state_from_message(message: &Message) -> Option<SubmissionState> {
    let container = find_container(message)?;
    let texts = extract_text_displays(message);

    let submitter_id = texts.iter().find_map(|t| {
        let start = t.find("<@")? + 2;
        let end = t[start..].find('>')? + start;
        t[start..end].parse::<u64>().ok()
    })?;

    let mut players = Vec::new();

    for part in &*container.components {
        match part {
            ContainerComponent::Section(section) => {
                let header_text = section.components.iter().find_map(|c| match c {
                    SectionComponent::TextDisplay(td) => td.content.clone(),
                    _ => None,
                });
                if let Some(header) = header_text {
                    if is_player_entry(&header) {
                        if let Some(username) = parse_player_ign(&header) {
                            players.push(new_player_entry(username, ""));
                        }
                    }
                }
            }
            ContainerComponent::TextDisplay(td) => {
                let Some(content) = &td.content else { continue };
                let trimmed = content.trim();

                if is_player_entry(trimmed) {
                    if let Some(username) = parse_player_ign(trimmed) {
                        players.push(new_player_entry(username, ""));
                    }
                    continue;
                }

                if is_tag_type_line(trimmed) {
                    if let Some(tag_name) = parse_tag_type_line(trimmed) {
                        if let Some(player) = players.last_mut() {
                            if player.tag_type.is_empty() {
                                player.tag_type = tag_name.to_string();
                            }
                        }
                    }
                    continue;
                }

                if let Some(player) = players.last_mut() {
                    if trimmed.starts_with('>') {
                        parse_player_details(player, trimmed);
                    } else if let Some(rest) = trimmed.strip_prefix("-# Currently ") {
                        let inner = rest.strip_prefix("**").and_then(|s| s.strip_suffix("**")).unwrap_or(rest);
                        let display = inner.split('>').next_back().unwrap_or(inner).trim();
                        if let Some(name) = lookup_tag_name_from_display(display) {
                            player.tag_type = name.to_string();
                        }
                    } else if let Some(uuid_str) = trimmed.strip_prefix("-# UUID: ") {
                        player.uuid = uuid_str.split_whitespace().next().unwrap_or("").replace('-', "");
                    } else if trimmed.contains("Nicked") && trimmed.starts_with("-#") {
                        player.is_nicked = true;
                    } else if let Some(status) = parse_status_line(trimmed) {
                        player.status = status.0;
                        player.reviewer = status.1;
                        player.review_note = status.2;
                    } else if let Some(votes) = parse_votes_line(trimmed) {
                        player.accept_votes = votes.0;
                        player.reject_votes = votes.1;
                    } else {
                        for line in trimmed.lines() {
                            if let Some(evidence) = parse_evidence_line(line.trim()) {
                                player.evidence.push(evidence);
                            }
                        }
                    }
                }
            }
            ContainerComponent::MediaGallery(gallery) => {
                for item in &*gallery.items {
                    let filename = attachment_filename_from_url(&item.media.url.to_string());
                    if let Some(player) = players.last_mut() {
                        player.evidence.push(Evidence::Attachment { filename });
                    }
                }
            }
            _ => {}
        }
    }

    let submitted = texts.iter().any(|t| {
        t.contains("Approved by") || t.contains("Rejected by") || t.contains("awaiting review")
    });

    let players: Vec<_> = players.into_iter().filter(|p| !p.tag_type.is_empty()).collect();

    Some(SubmissionState {
        submitter_id,
        players,
        submitted,
        editing: None,
        pending_add: None,
    })
}


pub fn is_player_entry(text: &str) -> bool {
    text.starts_with("IGN - `")
}


fn is_tag_type_line(text: &str) -> bool {
    text.starts_with("**") && text.ends_with("**") && text.contains('>')
}


fn parse_player_ign(text: &str) -> Option<String> {
    text.strip_prefix("IGN - `")?.strip_suffix('`').map(|s| s.to_string())
}


fn parse_tag_type_line(text: &str) -> Option<&'static str> {
    let inner = text.strip_prefix("**")?.strip_suffix("**")?;
    let display = inner.split('>').next_back()?.trim();
    lookup_tag_name_from_display(display)
}


fn new_player_entry(username: String, tag_type: &str) -> PlayerEntry {
    PlayerEntry {
        username,
        uuid: String::new(),
        tag_type: tag_type.to_string(),
        reason: String::new(),
        is_nicked: false,
        status: PlayerStatus::Pending,
        reviewer: None,
        review_note: None,
        evidence: Vec::new(),
        conflict_warning: None,
        accept_votes: Vec::new(),
        reject_votes: Vec::new(),
    }
}


fn parse_player_details(player: &mut PlayerEntry, content: &str) {
    if let Some(reason) = content.lines().next().and_then(|l| l.strip_prefix("> ")) {
        player.reason = reason.to_string();
    }
}


fn parse_status_line(text: &str) -> Option<(PlayerStatus, Option<String>, Option<String>)> {
    let line = text.strip_prefix("-# ")?;

    if line.contains("Pending") {
        return Some((PlayerStatus::Pending, None, None));
    }
    if let Some(rest) = line.strip_prefix("Approved by ") {
        return Some((PlayerStatus::Approved, Some(rest.to_string()), None));
    }
    if let Some(rest) = line.strip_prefix("Rejected by ") {
        let (reviewer, note) = match rest.find(": \"") {
            Some(pos) => {
                let note = rest[pos + 3..].strip_suffix('"').unwrap_or(&rest[pos + 3..]);
                (rest[..pos].to_string(), Some(note.to_string()))
            }
            None => (rest.to_string(), None),
        };
        return Some((PlayerStatus::Rejected, Some(reviewer), note));
    }
    None
}


fn parse_votes_line(text: &str) -> Option<(Vec<u64>, Vec<u64>)> {
    let line = text.strip_prefix("-# Votes: ")?;
    let (mut accepts, mut rejects) = (Vec::new(), Vec::new());

    for token in line.split_whitespace() {
        if let Some(id_str) = token.strip_prefix('+') {
            if let Ok(id) = id_str.parse::<u64>() { accepts.push(id); }
        } else if let Some(id_str) = token.strip_prefix('-') {
            if let Ok(id) = id_str.parse::<u64>() { rejects.push(id); }
        }
    }

    if accepts.is_empty() && rejects.is_empty() { return None; }
    Some((accepts, rejects))
}


pub fn lookup_tag_name_from_display(display: &str) -> Option<&'static str> {
    blacklist::all().iter().find(|t| t.display_name == display).map(|t| t.name)
}


fn parse_evidence_line(line: &str) -> Option<Evidence> {
    let line = line.strip_prefix("- ").unwrap_or(line);
    if !line.starts_with("`/replay") { return None; }

    let command = line.split('`').nth(1)?;
    let replay = parse_replay(command)?;
    let note = line
        .split("Note: \"")
        .nth(1)
        .and_then(|s| s.strip_suffix('"'))
        .map(|s| s.to_string());
    Some(Evidence::Replay { replay, note })
}


pub fn render_replay_line(replay: &Replay, note: Option<&str>) -> String {
    match note {
        Some(n) => format!("- `{}` \u{2014} Note: \"{}\"", replay.format_command(), n),
        None => format!("- `{}`", replay.format_command()),
    }
}


pub fn render_player_details(player: &PlayerEntry) -> (String, String) {
    let mut reason_block = format!("> {}", crate::utils::sanitize_reason(&player.reason));
    if let Some(warning) = &player.conflict_warning {
        reason_block.push('\n');
        reason_block.push_str(warning);
    }

    let uuid_line = if player.is_nicked {
        "-# Nicked \u{2014} UUID could not be resolved".to_string()
    } else {
        format!("-# UUID: {}", format_uuid_dashed(&player.uuid))
    };

    (reason_block, uuid_line)
}


pub fn render_evidence_summary(player: &PlayerEntry) -> Option<String> {
    let replays: Vec<String> = player
        .evidence
        .iter()
        .filter_map(|e| match e {
            Evidence::Replay { replay, note } => Some(render_replay_line(replay, note.as_deref())),
            _ => None,
        })
        .collect();

    let media_count = player.evidence.iter().filter(|e| matches!(e, Evidence::Attachment { .. })).count();

    if replays.is_empty() && media_count == 0 { return None; }

    let mut block = replays.join("\n");
    if media_count > 0 {
        if !block.is_empty() { block.push('\n'); }
        block.push_str(&format!(
            "-# {} media attachment{}",
            media_count,
            if media_count == 1 { "" } else { "s" }
        ));
    }
    Some(block)
}


pub fn media_gallery_for(
    player: &PlayerEntry,
    existing_urls: &HashMap<String, String>,
) -> Option<CreateContainerComponent<'static>> {
    let items: Vec<CreateMediaGalleryItem> = player
        .evidence
        .iter()
        .filter_map(|e| match e {
            Evidence::Attachment { filename } => {
                let url = existing_urls
                    .get(filename)
                    .cloned()
                    .unwrap_or_else(|| format!("attachment://{filename}"));
                Some(CreateMediaGalleryItem::new(CreateUnfurledMediaItem::new(url)))
            }
            _ => None,
        })
        .collect();

    if items.is_empty() { return None; }
    Some(CreateContainerComponent::MediaGallery(CreateMediaGallery::new(items)))
}


pub fn render_status_line(player: &PlayerEntry) -> String {
    match &player.status {
        PlayerStatus::Pending => "-# Pending review".to_string(),
        PlayerStatus::Approved => "-# Approved".to_string(),
        PlayerStatus::Rejected => match &player.review_note {
            Some(note) => format!("-# Rejected: \"{note}\""),
            None => "-# Rejected".to_string(),
        },
    }
}


pub fn render_vote_status(player: &PlayerEntry) -> String {
    let (accepts, rejects) = (player.accept_votes.len(), player.reject_votes.len());
    if accepts > 0 && rejects > 0 {
        format!("-# {} accept, {} reject \u{2014} staff required", accepts, rejects)
    } else if accepts > 0 {
        format!("-# {}/3 accepting", accepts)
    } else {
        format!("-# {}/3 rejecting", rejects)
    }
}


pub fn has_votes(player: &PlayerEntry) -> bool {
    !player.accept_votes.is_empty() || !player.reject_votes.is_empty()
}


pub fn extract_media_urls_from_message(message: &Message, player_index: usize) -> Vec<String> {
    let Some(container) = find_container(message) else { return Vec::new() };

    let mut current_player = 0usize;
    let mut seen_first = false;
    let mut urls = Vec::new();

    for part in &*container.components {
        match part {
            ContainerComponent::Section(section) => {
                let has_player = section.components.iter().any(|c| match c {
                    SectionComponent::TextDisplay(td) => {
                        td.content.as_ref().is_some_and(|t| is_player_entry(t))
                    }
                    _ => false,
                });
                if has_player {
                    if seen_first { current_player += 1; }
                    seen_first = true;
                }
            }
            ContainerComponent::TextDisplay(td) => {
                if let Some(content) = &td.content {
                    if is_player_entry(content.trim()) {
                        if seen_first { current_player += 1; }
                        seen_first = true;
                    }
                }
            }
            ContainerComponent::MediaGallery(gallery) if seen_first && current_player == player_index => {
                for item in &*gallery.items {
                    urls.push(item.media.url.to_string());
                }
            }
            _ => {}
        }
    }
    urls
}


pub fn parse_confirmation_data(custom_id: &str, message: &Message) -> Option<ConfirmationData> {
    let stripped = custom_id.strip_prefix("review_confirm:")?;
    let parts: Vec<&str> = stripped.splitn(4, ':').collect();
    if parts.len() < 4 { return None; }

    let tag_type = parts[1].to_string();
    let player_uuid = if parts[2] == "none" { String::new() } else { parts[2].to_string() };
    let is_nicked = parts[3] == "true";

    let texts = extract_text_displays(message);
    let preview = texts.iter().find(|t| t.contains(" \u{2014} `"))?;
    let player_name = preview.split('`').nth(1)?.to_string();
    let reason = preview.split("\n> ").nth(1).unwrap_or("").to_string();

    Some(ConfirmationData { player_name, player_uuid, tag_type, reason, is_nicked })
}
