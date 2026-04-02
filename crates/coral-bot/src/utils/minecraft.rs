pub fn format_uuid_dashed(uuid: &str) -> String {
    if uuid.len() != 32 { return uuid.to_string() }
    format!(
        "{}-{}-{}-{}-{}",
        &uuid[0..8], &uuid[8..12], &uuid[12..16], &uuid[16..20], &uuid[20..32]
    )
}


pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 { result.insert(0, ',') }
        result.insert(0, c);
    }
    result
}


pub fn sanitize_reason(reason: &str) -> String {
    let mut out = String::with_capacity(reason.len());
    for ch in reason.chars() {
        if matches!(ch, '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '#' | '[' | ']' | '(' | ')' | '-' | '@' | '<') {
            out.push('\\');
        } else if matches!(ch, '\n' | '\r') {
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    out
}


pub fn format_tag_detail(tag: &database::PlayerTagRow) -> String {
    if tag.tag_type == "replays_needed" {
        return match tag.expires_at {
            Some(ts) => format!("Expires <t:{}:R>", ts.timestamp()),
            None => "No expiration".into(),
        };
    }
    sanitize_reason(&tag.reason)
}


pub fn generate_api_key() -> String {
    uuid::Uuid::new_v4().to_string()
}
