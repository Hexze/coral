use chrono::{DateTime, Utc};

use super::bedwars::{Mode, WinstreakSnapshot};


const MIN_STREAK_THRESHOLD: u64 = 15;


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StreakSource {
    Urchin,
    Antisniper,
}


pub struct Streak {
    pub value: u64,
    pub approximate: bool,
    pub timestamp: DateTime<Utc>,
    pub source: StreakSource,
}


pub struct WinstreakHistory {
    pub streaks: Vec<Streak>,
}


pub fn calculate(snapshots: &[(DateTime<Utc>, WinstreakSnapshot)], modes: &[Mode]) -> WinstreakHistory {
    if snapshots.is_empty() {
        return WinstreakHistory { streaks: Vec::new() };
    }

    let mut streaks = Vec::new();
    let mut streak_start: Option<usize> = None;
    let mut peak_api_ws: Option<u64> = None;

    for (i, (_ts, stats)) in snapshots.iter().enumerate() {
        let (wins, losses) = combined_wins_losses(stats, modes);
        let api_ws = combined_api_winstreak(stats, modes);

        let delta_losses = if i > 0 {
            let (_, prev_losses) = combined_wins_losses(&snapshots[i - 1].1, modes);
            losses.saturating_sub(prev_losses)
        } else {
            0
        };

        if let Some(ws) = api_ws {
            peak_api_ws = Some(peak_api_ws.map_or(ws, |peak| peak.max(ws)));
        }

        if let (Some(start_idx), true) = (streak_start, delta_losses > 0) {
            let prev_idx = i - 1;
            let prev_ts = snapshots[prev_idx].0;
            let (start_wins, _) = combined_wins_losses(&snapshots[start_idx].1, modes);
            let (prev_wins, _) = combined_wins_losses(&snapshots[prev_idx].1, modes);

            let (value, approximate) = match peak_api_ws {
                Some(peak) => {
                    let delta_wins = wins.saturating_sub(prev_wins);
                    let mut best = peak;
                    if delta_losses == 1 {
                        best = match api_ws {
                            Some(after) => best.max(wins.saturating_sub(start_wins).saturating_sub(after)),
                            None => best.max(prev_wins.saturating_sub(start_wins)),
                        };
                    } else {
                        best = best.max(prev_wins.saturating_sub(start_wins));
                    }
                    (best, delta_wins >= 2 || best > peak)
                }
                None => (prev_wins.saturating_sub(start_wins), true),
            };

            if value >= MIN_STREAK_THRESHOLD {
                streaks.push(Streak {
                    value,
                    approximate,
                    timestamp: prev_ts,
                    source: StreakSource::Urchin,
                });
            }

            streak_start = None;
            peak_api_ws = None;
        }

        if streak_start.is_none() && (delta_losses > 0 || i == 0) {
            streak_start = Some(i);
            peak_api_ws = api_ws;
        }
    }

    streaks.sort_by(|a, b| b.value.cmp(&a.value));
    WinstreakHistory { streaks }
}


fn mode_wins_losses(stats: &WinstreakSnapshot, mode: Mode) -> (u64, u64) {
    match mode {
        Mode::Overall | Mode::Core => {
            let w = stats.solos.wins + stats.doubles.wins + stats.threes.wins + stats.fours.wins;
            let l = stats.solos.losses + stats.doubles.losses + stats.threes.losses + stats.fours.losses;
            (w, l)
        }
        Mode::Solos => (stats.solos.wins, stats.solos.losses),
        Mode::Doubles => (stats.doubles.wins, stats.doubles.losses),
        Mode::Threes => (stats.threes.wins, stats.threes.losses),
        Mode::Fours => (stats.fours.wins, stats.fours.losses),
        Mode::FourTeamModes => (stats.threes.wins + stats.fours.wins, stats.threes.losses + stats.fours.losses),
        Mode::FourVFour => (stats.four_v_four.wins, stats.four_v_four.losses),
    }
}


fn combined_wins_losses(stats: &WinstreakSnapshot, modes: &[Mode]) -> (u64, u64) {
    if modes.len() == 1 {
        return mode_wins_losses(stats, modes[0]);
    }
    modes.iter().fold((0, 0), |(w, l), &mode| {
        let (mw, ml) = mode_wins_losses(stats, mode);
        (w + mw, l + ml)
    })
}


fn api_winstreak(stats: &WinstreakSnapshot, mode: Mode) -> Option<u64> {
    match mode {
        Mode::Overall | Mode::Core => stats.overall.winstreak,
        Mode::Solos => stats.solos.winstreak,
        Mode::Doubles => stats.doubles.winstreak,
        Mode::Threes => stats.threes.winstreak,
        Mode::Fours => stats.fours.winstreak,
        Mode::FourTeamModes => None,
        Mode::FourVFour => stats.four_v_four.winstreak,
    }
}


fn combined_api_winstreak(stats: &WinstreakSnapshot, modes: &[Mode]) -> Option<u64> {
    if modes.len() == 1 {
        return api_winstreak(stats, modes[0]);
    }
    None
}


#[cfg(test)]
mod tests {
    use super::super::bedwars::WinstreakModeData;
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> { Utc.timestamp_opt(secs, 0).unwrap() }

    fn snapshot(solos_wins: u64, solos_losses: u64, ws: Option<u64>) -> WinstreakSnapshot {
        WinstreakSnapshot {
            solos: WinstreakModeData { wins: solos_wins, losses: solos_losses, winstreak: ws },
            ..Default::default()
        }
    }

    #[test]
    fn empty_snapshots_returns_empty() {
        assert!(calculate(&[], &[Mode::Solos]).streaks.is_empty());
    }

    #[test]
    fn single_snapshot_no_streaks() {
        let snaps = vec![(ts(1000), snapshot(100, 50, Some(5)))];
        assert!(calculate(&snaps, &[Mode::Solos]).streaks.is_empty());
    }

    #[test]
    fn streak_below_threshold_not_recorded() {
        let snaps = vec![
            (ts(1000), snapshot(100, 50, Some(10))),
            (ts(2000), snapshot(110, 50, Some(10))),
            (ts(3000), snapshot(112, 51, Some(2))),
        ];
        assert!(calculate(&snaps, &[Mode::Solos]).streaks.is_empty());
    }

    #[test]
    fn streak_at_threshold_recorded() {
        let snaps = vec![
            (ts(1000), snapshot(100, 50, Some(15))),
            (ts(2000), snapshot(115, 50, Some(15))),
            (ts(3000), snapshot(116, 51, Some(1))),
        ];
        let history = calculate(&snaps, &[Mode::Solos]);
        assert_eq!(history.streaks.len(), 1);
        assert!(history.streaks[0].value >= MIN_STREAK_THRESHOLD);
    }

    #[test]
    fn no_api_winstreak_uses_delta_wins() {
        let snaps = vec![
            (ts(1000), WinstreakSnapshot {
                solos: WinstreakModeData { wins: 100, losses: 50, winstreak: None },
                ..Default::default()
            }),
            (ts(2000), WinstreakSnapshot {
                solos: WinstreakModeData { wins: 120, losses: 50, winstreak: None },
                ..Default::default()
            }),
            (ts(3000), WinstreakSnapshot {
                solos: WinstreakModeData { wins: 121, losses: 51, winstreak: None },
                ..Default::default()
            }),
        ];
        let history = calculate(&snaps, &[Mode::Solos]);
        assert_eq!(history.streaks.len(), 1);
        assert_eq!(history.streaks[0].value, 20);
        assert!(history.streaks[0].approximate);
    }

    #[test]
    fn streaks_sorted_by_value_descending() {
        let snaps = vec![
            (ts(1000), snapshot(100, 50, Some(16))),
            (ts(2000), snapshot(116, 50, Some(16))),
            (ts(3000), snapshot(117, 51, Some(1))),
            (ts(4000), snapshot(137, 51, Some(20))),
            (ts(5000), snapshot(138, 52, Some(1))),
        ];
        let history = calculate(&snaps, &[Mode::Solos]);
        assert_eq!(history.streaks.len(), 2);
        assert!(history.streaks[0].value >= history.streaks[1].value);
    }
}
