use serde_json::Value;

use super::player::{calculate_network_level, color_code, extract_rank_prefix};


#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Overall,
    Core,
    Solos,
    Doubles,
    Threes,
    Fours,
    FourTeamModes,
    FourVFour,
}


impl Mode {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Overall => "Overall",
            Self::Core => "Core",
            Self::Solos => "Solos",
            Self::Doubles => "Doubles",
            Self::Threes => "Threes",
            Self::Fours => "Fours",
            Self::FourTeamModes => "4 Team Modes",
            Self::FourVFour => "4v4",
        }
    }

    pub fn all() -> &'static [Mode] {
        &[
            Self::Overall, Self::Core, Self::Solos, Self::Doubles,
            Self::Threes, Self::Fours, Self::FourTeamModes, Self::FourVFour,
        ]
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Overall => "Overall",
            Self::Core => "Core",
            Self::Solos => "1s",
            Self::Doubles => "2s",
            Self::Threes => "3s",
            Self::Fours => "4s",
            Self::FourTeamModes => "3s/4s",
            Self::FourVFour => "4v4",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "overall" => Some(Self::Overall),
            "core" => Some(Self::Core),
            "solos" => Some(Self::Solos),
            "doubles" => Some(Self::Doubles),
            "threes" => Some(Self::Threes),
            "fours" => Some(Self::Fours),
            "4 team modes" | "4_team_modes" | "fourteammodes" => Some(Self::FourTeamModes),
            "4v4" | "fourvfour" => Some(Self::FourVFour),
            _ => None,
        }
    }
}


#[derive(Clone, Default)]
pub struct GuildInfo {
    pub name: Option<String>,
    pub tag: Option<String>,
    pub tag_color: Option<String>,
    pub rank: Option<String>,
    pub joined: Option<i64>,
    pub weekly_gexp: Option<u64>,
}


impl GuildInfo {
    pub fn tag_with_color(&self) -> Option<String> {
        let tag = self.tag.as_ref()?;
        let color = self.tag_color.as_deref().unwrap_or("GRAY");
        Some(format!("{}[{}]", color_code(color), tag))
    }
}


pub fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 { numerator as f64 } else { numerator as f64 / denominator as f64 }
}


#[derive(Clone, Default)]
pub struct ModeStats {
    pub wins: u64,
    pub losses: u64,
    pub kills: u64,
    pub deaths: u64,
    pub final_kills: u64,
    pub final_deaths: u64,
    pub beds_broken: u64,
    pub beds_lost: u64,
    pub winstreak: Option<u64>,
}


impl ModeStats {
    pub fn wlr(&self) -> f64 { ratio(self.wins, self.losses) }
    pub fn kdr(&self) -> f64 { ratio(self.kills, self.deaths) }
    pub fn fkdr(&self) -> f64 { ratio(self.final_kills, self.final_deaths) }
    pub fn bblr(&self) -> f64 { ratio(self.beds_broken, self.beds_lost) }
}


#[derive(Clone, Default)]
pub struct SlumberInfo {
    pub tickets: u64,
    pub total_tickets_earned: u64,
    pub doublers: u64,
}


#[derive(Clone)]
pub struct Stats {
    pub username: String,
    pub display_name: String,
    pub rank_prefix: Option<String>,
    pub experience: u64,
    pub level: u32,
    pub games_played: u64,
    pub network_level: f64,
    pub achievement_points: u64,
    pub ranks_gifted: u64,
    pub first_login: Option<i64>,
    pub guild: GuildInfo,
    pub slumber: SlumberInfo,
    pub overall: ModeStats,
    pub solos: ModeStats,
    pub doubles: ModeStats,
    pub threes: ModeStats,
    pub fours: ModeStats,
    pub four_v_four: ModeStats,
}


impl Stats {
    pub fn get_mode_stats(&self, mode: Mode) -> ModeStats {
        match mode {
            Mode::Overall => self.overall.clone(),
            Mode::Core => self.core_stats(),
            Mode::Solos => self.solos.clone(),
            Mode::Doubles => self.doubles.clone(),
            Mode::Threes => self.threes.clone(),
            Mode::Fours => self.fours.clone(),
            Mode::FourTeamModes => self.four_team_modes_stats(),
            Mode::FourVFour => self.four_v_four.clone(),
        }
    }

    fn core_stats(&self) -> ModeStats {
        let (o, v) = (&self.overall, &self.four_v_four);
        ModeStats {
            wins: o.wins.saturating_sub(v.wins),
            losses: o.losses.saturating_sub(v.losses),
            kills: o.kills.saturating_sub(v.kills),
            deaths: o.deaths.saturating_sub(v.deaths),
            final_kills: o.final_kills.saturating_sub(v.final_kills),
            final_deaths: o.final_deaths.saturating_sub(v.final_deaths),
            beds_broken: o.beds_broken.saturating_sub(v.beds_broken),
            beds_lost: o.beds_lost.saturating_sub(v.beds_lost),
            winstreak: o.winstreak,
        }
    }

    pub fn get_combined_mode_stats(&self, modes: &[Mode]) -> ModeStats {
        if modes.len() == 1 {
            return self.get_mode_stats(modes[0]);
        }
        modes.iter().fold(ModeStats::default(), |acc, &mode| {
            let ms = self.get_mode_stats(mode);
            ModeStats {
                wins: acc.wins + ms.wins,
                losses: acc.losses + ms.losses,
                kills: acc.kills + ms.kills,
                deaths: acc.deaths + ms.deaths,
                final_kills: acc.final_kills + ms.final_kills,
                final_deaths: acc.final_deaths + ms.final_deaths,
                beds_broken: acc.beds_broken + ms.beds_broken,
                beds_lost: acc.beds_lost + ms.beds_lost,
                winstreak: None,
            }
        })
    }

    fn four_team_modes_stats(&self) -> ModeStats {
        let (t, f) = (&self.threes, &self.fours);
        ModeStats {
            wins: t.wins + f.wins,
            losses: t.losses + f.losses,
            kills: t.kills + f.kills,
            deaths: t.deaths + f.deaths,
            final_kills: t.final_kills + f.final_kills,
            final_deaths: t.final_deaths + f.final_deaths,
            beds_broken: t.beds_broken + f.beds_broken,
            beds_lost: t.beds_lost + f.beds_lost,
            winstreak: None,
        }
    }
}


pub fn combined_mode_name(modes: &[Mode]) -> String {
    if modes.len() == 1 {
        return modes[0].display_name().to_string();
    }
    let has = |m: Mode| modes.contains(&m);
    let exactly = |expected: &[Mode]| modes.len() == expected.len() && expected.iter().all(|m| has(*m));
    if exactly(&[Mode::Solos, Mode::Doubles, Mode::Threes, Mode::Fours, Mode::FourVFour]) {
        return "Overall".to_string();
    }
    if exactly(&[Mode::Solos, Mode::Doubles, Mode::Threes, Mode::Fours]) {
        return "Core".to_string();
    }
    if exactly(&[Mode::Threes, Mode::Fours]) {
        return "4 Team Modes".to_string();
    }
    if exactly(&[Mode::Solos, Mode::Doubles]) {
        return "8 Team Modes".to_string();
    }
    modes.iter().map(|m| m.short_name()).collect::<Vec<_>>().join("/")
}


pub fn extract(username: &str, player: &Value, guild: Option<GuildInfo>) -> Option<Stats> {
    let bw = player.get("stats")?.get("Bedwars")?;
    let experience = bw.get("Experience").and_then(|v| v.as_u64()).unwrap_or(0);

    let level = player
        .get("achievements")
        .and_then(|a| a.get("bedwars_level"))
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| calculate_level(experience)) as u32;

    let display_name = player.get("displayname").and_then(|v| v.as_str()).unwrap_or(username).to_string();
    let network_exp = player.get("networkExp").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let slumber = bw.get("slumber").map(|s| SlumberInfo {
        tickets: s.get("tickets").and_then(|v| v.as_u64()).unwrap_or(0),
        total_tickets_earned: s.get("total_tickets_earned").and_then(|v| v.as_u64()).unwrap_or(0),
        doublers: s.get("doublers").and_then(|v| v.as_u64()).unwrap_or(0),
    }).unwrap_or_default();

    Some(Stats {
        username: username.to_string(),
        display_name,
        rank_prefix: extract_rank_prefix(player),
        experience,
        level,
        games_played: bw.get("games_played_bedwars").and_then(|v| v.as_u64()).unwrap_or(0),
        network_level: calculate_network_level(network_exp),
        achievement_points: player.get("achievementPoints").and_then(|v| v.as_u64()).unwrap_or(0),
        ranks_gifted: player.get("giftingMeta").and_then(|g| g.get("ranksGiven")).and_then(|v| v.as_u64()).unwrap_or(0),
        first_login: player.get("firstLogin").and_then(|v| v.as_i64()),
        guild: guild.unwrap_or_default(),
        slumber,
        overall: extract_mode_stats(bw, ""),
        solos: extract_mode_stats(bw, "eight_one_"),
        doubles: extract_mode_stats(bw, "eight_two_"),
        threes: extract_mode_stats(bw, "four_three_"),
        fours: extract_mode_stats(bw, "four_four_"),
        four_v_four: extract_mode_stats(bw, "two_four_"),
    })
}


fn extract_mode_stats(bw: &Value, prefix: &str) -> ModeStats {
    let stat = |suffix: &str| -> u64 {
        bw.get(&format!("{prefix}{suffix}")).and_then(|v| v.as_u64()).unwrap_or(0)
    };
    let ws_key = if prefix.is_empty() { "winstreak".to_string() } else { format!("{prefix}winstreak") };

    ModeStats {
        wins: stat("wins_bedwars"),
        losses: stat("losses_bedwars"),
        kills: stat("kills_bedwars"),
        deaths: stat("deaths_bedwars"),
        final_kills: stat("final_kills_bedwars"),
        final_deaths: stat("final_deaths_bedwars"),
        beds_broken: stat("beds_broken_bedwars"),
        beds_lost: stat("beds_lost_bedwars"),
        winstreak: bw.get(&ws_key).and_then(|v| v.as_u64()),
    }
}


#[derive(Clone, Default)]
pub struct WinstreakModeData {
    pub wins: u64,
    pub losses: u64,
    pub winstreak: Option<u64>,
}


#[derive(Clone, Default)]
pub struct WinstreakSnapshot {
    pub overall: WinstreakModeData,
    pub solos: WinstreakModeData,
    pub doubles: WinstreakModeData,
    pub threes: WinstreakModeData,
    pub fours: WinstreakModeData,
    pub four_v_four: WinstreakModeData,
}


pub fn extract_winstreak_snapshot(player: &Value) -> Option<WinstreakSnapshot> {
    let bw = player.get("stats")?.get("Bedwars")?;

    let mode = |wins: &str, losses: &str, ws: &str| WinstreakModeData {
        wins: bw.get(wins).and_then(|v| v.as_u64()).unwrap_or(0),
        losses: bw.get(losses).and_then(|v| v.as_u64()).unwrap_or(0),
        winstreak: bw.get(ws).and_then(|v| v.as_u64()),
    };

    Some(WinstreakSnapshot {
        overall: mode("wins_bedwars", "losses_bedwars", "winstreak"),
        solos: mode("eight_one_wins_bedwars", "eight_one_losses_bedwars", "eight_one_winstreak"),
        doubles: mode("eight_two_wins_bedwars", "eight_two_losses_bedwars", "eight_two_winstreak"),
        threes: mode("four_three_wins_bedwars", "four_three_losses_bedwars", "four_three_winstreak"),
        fours: mode("four_four_wins_bedwars", "four_four_losses_bedwars", "four_four_winstreak"),
        four_v_four: mode("two_four_wins_bedwars", "two_four_losses_bedwars", "two_four_winstreak"),
    })
}


pub fn calculate_level(experience: u64) -> u64 {
    let level = 100 * (experience / 487000);
    let exp = experience % 487000;
    match exp {
        0..500 => level,
        500..1500 => level + 1,
        1500..3500 => level + 2,
        3500..7000 => level + 3,
        _ => level + 4 + (exp - 7000) / 5000,
    }
}


pub fn experience_for_level(level: u64) -> u64 {
    let base = (level / 100) * 487000;
    match level % 100 {
        0 => base,
        1 => base + 500,
        2 => base + 1500,
        3 => base + 3500,
        l => base + 7000 + (l - 4) * 5000,
    }
}


pub fn level_progress(experience: u64) -> f64 {
    let exp = experience % 487000;
    let raw = match exp {
        0..500 => exp as f64 / 500.0 / 100.0,
        500..1500 => (1.0 + (exp - 500) as f64 / 1000.0) / 100.0,
        1500..3500 => (2.0 + (exp - 1500) as f64 / 2000.0) / 100.0,
        3500..7000 => (3.0 + (exp - 3500) as f64 / 3500.0) / 100.0,
        _ => {
            let remaining = exp - 7000;
            ((4 + remaining / 5000) as f64 + (remaining % 5000) as f64 / 5000.0) / 100.0
        }
    };
    raw.fract()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_for_level_roundtrip() {
        for level in [0, 1, 2, 3, 4, 50, 99, 100, 150, 200, 500, 1000] {
            let xp = experience_for_level(level);
            assert_eq!(calculate_level(xp), level, "roundtrip failed for level {level}");
        }
    }
}
