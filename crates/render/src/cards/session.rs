use chrono::{DateTime, Utc};
use image::DynamicImage;
use mctext::{MCText, NamedColor};

use hypixel::{BedwarsPlayerStats, Mode, SessionStats, color_code, combined_mode_name, level_progress};

use crate::canvas::{
    Align, BOX_BACKGROUND, CANVAS_BACKGROUND, Canvas, DrawContext, Image, Rgba, RgbaImage,
    RoundedRect, Shape, TextBlock, TextBox, blend,
};

use super::bedwars::TagIcon;
use super::common::{
    BAR_COLOR, color_name_to_named, colors, draw_progress_bar, format_number, format_percent,
    format_ratio, format_timestamp, stat_line,
};
use super::prestiges::{build_prestige_text, prestige_colors, prestige_star};

const BOX_CORNER_RADIUS: u32 = 18;
const CANVAS_WIDTH: u32 = 800;
const CANVAS_HEIGHT: u32 = 600;
const COL_WIDTH: u32 = 256;
const HEADER_Y: u32 = 0;
const HEADER_HEIGHT: u32 = 100;
const LEVEL_Y: u32 = 57;
const MAIN_ROW_Y: u32 = 116;
const STATS_BOX_WIDTH: u32 = 528;
const STATS_BOX_HEIGHT: u32 = 176;
const SKIN_BOX_HEIGHT: u32 = 368;
const SECOND_ROW_Y: u32 = MAIN_ROW_Y + STATS_BOX_HEIGHT + 16;
const SECOND_ROW_HEIGHT: u32 = 176;
const BOTTOM_ROW_Y: u32 = 500;
const BOTTOM_BOX_HEIGHT: u32 = 100;
const LEVEL_SCALE: f32 = 2.75;
const LEVEL_PADDING: u32 = 20;
const SKIN_PADDING: u32 = 12;


fn col_x(col: u32) -> u32 {
    match col {
        0 => 0,
        1 => 272,
        2 => 544,
        _ => 0,
    }
}


#[derive(Clone)]
pub enum SessionType {
    Custom(String),
    Daily,
    Weekly,
    Monthly,
    Yearly,
}


impl SessionType {
    pub fn display_name(&self) -> &str {
        match self {
            SessionType::Custom(name) => name,
            SessionType::Daily => "Daily",
            SessionType::Weekly => "Weekly",
            SessionType::Monthly => "Monthly",
            SessionType::Yearly => "Yearly",
        }
    }
}


pub fn render_session(
    current: &BedwarsPlayerStats,
    previous: &BedwarsPlayerStats,
    session_type: SessionType,
    started: DateTime<Utc>,
    ended: Option<DateTime<Utc>>,
    modes: &[Mode],
    skin: Option<&DynamicImage>,
    tags: &[TagIcon],
) -> RgbaImage {
    let current_mode = current.get_combined_mode_stats(modes);
    let previous_mode = previous.get_combined_mode_stats(modes);
    let session_stats = SessionStats::from_mode_stats(&previous_mode, &current_mode);
    let exp_gained = current.experience.saturating_sub(previous.experience);
    let games_played = current.games_played.saturating_sub(previous.games_played);
    let stars_gained = exp_gained as f64 / 5000.0;

    let mode_games = compute_mode_games(current, previous);
    let win_rate = if games_played > 0 {
        session_stats.wins as f64 / games_played as f64 * 100.0
    } else {
        0.0
    };
    let finals_per_star = if stars_gained > 0.01 {
        session_stats.final_kills as f64 / stars_gained
    } else {
        0.0
    };
    let clutch_rate = if session_stats.beds_lost > 0 {
        let clutches = session_stats.beds_lost.saturating_sub(session_stats.final_deaths);
        clutches as f64 / session_stats.beds_lost as f64 * 100.0
    } else {
        0.0
    };

    let ratio_change = RatioChangeBox {
        rows: [
            RatioChangeRow { label: "WLR:", value: current_mode.wlr(), delta: current_mode.wlr() - previous_mode.wlr(), color_fn: colors::wlr },
            RatioChangeRow { label: "FKDR:", value: current_mode.fkdr(), delta: current_mode.fkdr() - previous_mode.fkdr(), color_fn: colors::fkdr },
            RatioChangeRow { label: "KDR:", value: current_mode.kdr(), delta: current_mode.kdr() - previous_mode.kdr(), color_fn: colors::kdr },
            RatioChangeRow { label: "BBLR:", value: current_mode.bblr(), delta: current_mode.bblr() - previous_mode.bblr(), color_fn: colors::bblr },
        ],
    };

    let canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT)
        .background(CANVAS_BACKGROUND)
        .draw(0, HEADER_Y as i32, &HeaderSection::new(current, tags))
        .draw(0, LEVEL_Y as i32, &LevelSection::new(current.experience, stars_gained))
        .draw(col_x(0) as i32, MAIN_ROW_Y as i32, &SkinSection::new(skin, &combined_mode_name(modes), current.network_level))
        .draw(col_x(1) as i32, MAIN_ROW_Y as i32, &StatsSection::new(&session_stats));

    let is_single_non_overall = modes.len() == 1 && modes[0] != Mode::Overall;
    let canvas = if is_single_non_overall {
        canvas.draw(
            col_x(1) as i32, SECOND_ROW_Y as i32,
            &ModeShareBox::from_delta(current, previous, modes[0]),
        )
    } else {
        canvas.draw(
            col_x(1) as i32, SECOND_ROW_Y as i32,
            &VerticalGamesBox::new(&mode_games, COL_WIDTH, SECOND_ROW_HEIGHT),
        )
    };

    canvas
        .draw(col_x(2) as i32, SECOND_ROW_Y as i32, &ratio_change)
        .draw(col_x(2) as i32, BOTTOM_ROW_Y as i32, &efficiency_box(win_rate, finals_per_star, clutch_rate))
        .draw(col_x(0) as i32, BOTTOM_ROW_Y as i32, &session_box(&session_type, started, ended))
        .draw(col_x(1) as i32, BOTTOM_ROW_Y as i32, &GuildBox { stats: current })
        .build()
}


struct StatsSection<'a> {
    stats: &'a SessionStats,
}


impl<'a> StatsSection<'a> {
    fn new(stats: &'a SessionStats) -> Self { Self { stats } }
}


impl Shape for StatsSection<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        RoundedRect::new(STATS_BOX_WIDTH, STATS_BOX_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let main_scale = 2.0;
        let neg_scale = 1.5;
        let main_font = main_scale * 16.0;
        let neg_font = neg_scale * 16.0;
        let padding = 16;
        let line_height = (STATS_BOX_HEIGHT - padding * 2) / 4;

        let rows: [(&str, &str, f64, u64, u64, NamedColor, NamedColor); 4] = [
            ("WLR:", "Wins:", self.stats.wlr(), self.stats.wins, self.stats.losses, colors::session_wlr(self.stats.wlr()), colors::wins(self.stats.wins)),
            ("FKDR:", "Finals:", self.stats.fkdr(), self.stats.final_kills, self.stats.final_deaths, colors::session_fkdr(self.stats.fkdr()), colors::final_kills(self.stats.final_kills)),
            ("KDR:", "Kills:", self.stats.kdr(), self.stats.kills, self.stats.deaths, colors::kdr(self.stats.kdr()), colors::kills(self.stats.kills)),
            ("BBLR:", "Beds:", self.stats.bblr(), self.stats.beds_broken, self.stats.beds_lost, colors::session_bblr(self.stats.bblr()), colors::beds_broken(self.stats.beds_broken)),
        ];

        let mut max_ratio_w: f32 = 0.0;
        let mut max_right_w: f32 = 0.0;
        let mut measurements = Vec::new();

        for (ratio_label, pos_label, ratio, positive, negative, ratio_color, positive_color) in &rows {
            let ratio_text = MCText::new()
                .span(*ratio_label).color(NamedColor::Gray)
                .then(" ").then(&format_ratio(*ratio)).color(*ratio_color)
                .build();
            let (ratio_w, main_h) = ctx.renderer.measure(&ratio_text, main_font);

            let pos_text = MCText::new()
                .span(*pos_label).color(NamedColor::Gray)
                .then(" ").then(&format_number(*positive)).color(*positive_color)
                .build();
            let (pos_w, _) = ctx.renderer.measure(&pos_text, main_font);

            let neg_text = MCText::new()
                .span(" / ").color(NamedColor::DarkGray)
                .then(&format_number(*negative)).color(NamedColor::Gray)
                .build();
            let (neg_w, neg_h) = ctx.renderer.measure(&neg_text, neg_font);

            max_ratio_w = max_ratio_w.max(ratio_w);
            max_right_w = max_right_w.max(pos_w + neg_w);
            measurements.push((ratio_text, pos_text, neg_text, pos_w, main_h, neg_h));
        }

        let left_end = padding as f32 + max_ratio_w;
        let right_edge = STATS_BOX_WIDTH as f32 - padding as f32;
        let col_pos = left_end + (right_edge - left_end - max_right_w) / 2.0;

        for (i, (ratio_text, pos_text, neg_text, pos_w, main_h, neg_h)) in
            measurements.into_iter().enumerate()
        {
            let y = padding + i as u32 * line_height;
            TextBlock::new().push(ratio_text).scale(main_scale).draw(&mut ctx.at(padding as i32, y as i32));
            TextBlock::new().push(pos_text).scale(main_scale).draw(&mut ctx.at(col_pos as i32, y as i32));
            let neg_y = y as f32 + (main_h - neg_h) * 0.75;
            TextBlock::new().push(neg_text).scale(neg_scale).draw(&mut ctx.at((col_pos + pos_w) as i32, neg_y as i32));
        }
    }

    fn size(&self) -> (u32, u32) { (STATS_BOX_WIDTH, STATS_BOX_HEIGHT) }
}


struct RatioChangeRow {
    label: &'static str,
    value: f64,
    delta: f64,
    color_fn: fn(f64) -> NamedColor,
}


struct RatioChangeBox {
    rows: [RatioChangeRow; 4],
}


impl Shape for RatioChangeBox {
    fn draw(&self, ctx: &mut DrawContext) {
        let main_scale = 2.0f32;
        let sub_scale = 1.5f32;
        let main_font = main_scale * 16.0;
        let sub_font = sub_scale * 16.0;
        let padding = 16u32;

        RoundedRect::new(COL_WIDTH, SECOND_ROW_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let line_height = (SECOND_ROW_HEIGHT - padding * 2) / self.rows.len() as u32;

        for (i, row) in self.rows.iter().enumerate() {
            let y = padding + i as u32 * line_height;

            let label_text = MCText::new()
                .span(row.label).color(NamedColor::Gray)
                .then(" ").then(&format_ratio(row.value)).color((row.color_fn)(row.value))
                .build();
            let (label_w, main_h) = ctx.renderer.measure(&label_text, main_font);
            TextBlock::new().push(label_text).scale(main_scale).draw(&mut ctx.at(padding as i32, y as i32));

            let (sign, sign_color) = if row.delta > 0.005 {
                ("+", NamedColor::Green)
            } else if row.delta < -0.005 {
                ("-", NamedColor::Red)
            } else {
                ("+", NamedColor::Gray)
            };

            let delta_text = MCText::new()
                .span(" ").then(sign).color(sign_color)
                .then(&format_ratio(row.delta.abs())).color(sign_color)
                .build();
            let (_, sub_h) = ctx.renderer.measure(&delta_text, sub_font);
            let delta_x = padding as f32 + label_w;
            let delta_y = y as f32 + (main_h - sub_h) * 0.75;
            TextBlock::new().push(delta_text).scale(sub_scale).draw(&mut ctx.at(delta_x as i32, delta_y as i32));
        }
    }

    fn size(&self) -> (u32, u32) { (COL_WIDTH, SECOND_ROW_HEIGHT) }
}


struct ModeShareBox {
    wins_pct: f64,
    finals_pct: f64,
    kills_pct: f64,
    beds_pct: f64,
}


impl ModeShareBox {
    fn from_delta(current: &BedwarsPlayerStats, previous: &BedwarsPlayerStats, mode: Mode) -> Self {
        let cur_mode = current.get_mode_stats(mode);
        let prev_mode = previous.get_mode_stats(mode);
        let cur_overall = &current.overall;
        let prev_overall = &previous.overall;

        let pct = |mc: u64, mp: u64, oc: u64, op: u64| -> f64 {
            let md = mc.saturating_sub(mp);
            let od = oc.saturating_sub(op);
            if od == 0 { 0.0 } else { md as f64 / od as f64 * 100.0 }
        };

        Self {
            wins_pct: pct(cur_mode.wins, prev_mode.wins, cur_overall.wins, prev_overall.wins),
            finals_pct: pct(cur_mode.final_kills, prev_mode.final_kills, cur_overall.final_kills, prev_overall.final_kills),
            kills_pct: pct(cur_mode.kills, prev_mode.kills, cur_overall.kills, prev_overall.kills),
            beds_pct: pct(cur_mode.beds_broken, prev_mode.beds_broken, cur_overall.beds_broken, prev_overall.beds_broken),
        }
    }
}


impl Shape for ModeShareBox {
    fn draw(&self, ctx: &mut DrawContext) {
        let padding = 16u32;
        let bar_height = 28u32;
        let text_scale = 1.5f32;
        let text_font = text_scale * 16.0;

        RoundedRect::new(COL_WIDTH, SECOND_ROW_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let rows: [(&str, f64); 4] = [
            ("Wins", self.wins_pct),
            ("Finals", self.finals_pct),
            ("Kills", self.kills_pct),
            ("Beds", self.beds_pct),
        ];

        let bar_width = COL_WIDTH - padding * 2;
        let gap = (SECOND_ROW_HEIGHT - padding * 2 - bar_height * 4) / 3;
        let (cw, ch) = ctx.buffer.dimensions();

        for (i, (label, pct)) in rows.iter().enumerate() {
            let bx = padding;
            let by = padding + i as u32 * (bar_height + gap);
            let filled_w = (pct / 100.0 * bar_width as f64).round() as u32;
            if filled_w > 0 {
                draw_progress_bar(ctx, bx, by, filled_w, bar_height, 0, 1.0, BAR_COLOR, BAR_COLOR);
            }
            let text = MCText::new()
                .span(&format_percent(*pct)).color(NamedColor::Green)
                .then(&format!(" of {label}")).color(NamedColor::Gray)
                .build();
            let (tw, th) = ctx.renderer.measure(&text, text_font);
            ctx.renderer.draw(
                ctx.buffer.as_mut(), cw, ch,
                ctx.x as f32 + bx as f32 + (bar_width as f32 - tw) / 2.0,
                ctx.y as f32 + by as f32 + (bar_height as f32 - th) / 2.0,
                &text, text_font, true,
            );
        }
    }

    fn size(&self) -> (u32, u32) { (COL_WIDTH, SECOND_ROW_HEIGHT) }
}


pub struct ModeGames {
    pub solos: u64,
    pub doubles: u64,
    pub threes: u64,
    pub fours: u64,
    pub four_v_four: u64,
}


struct ModeEntry {
    label: &'static str,
    count: u64,
    color: NamedColor,
}


impl ModeGames {
    fn total(&self) -> u64 {
        self.solos + self.doubles + self.threes + self.fours + self.four_v_four
    }

    fn entries(&self) -> Vec<ModeEntry> {
        [("1s", self.solos), ("2s", self.doubles), ("3s", self.threes), ("4s", self.fours), ("4v4", self.four_v_four)]
            .into_iter()
            .map(|(label, count)| ModeEntry { label, count, color: NamedColor::Green })
            .collect()
    }
}


fn compute_mode_games(current: &BedwarsPlayerStats, previous: &BedwarsPlayerStats) -> ModeGames {
    let delta = |cur: &hypixel::ModeStats, prev: &hypixel::ModeStats| -> u64 {
        (cur.wins + cur.losses).saturating_sub(prev.wins + prev.losses)
    };
    ModeGames {
        solos: delta(&current.solos, &previous.solos),
        doubles: delta(&current.doubles, &previous.doubles),
        threes: delta(&current.threes, &previous.threes),
        fours: delta(&current.fours, &previous.fours),
        four_v_four: delta(&current.four_v_four, &previous.four_v_four),
    }
}


fn efficiency_box(win_rate: f64, finals_per_star: f64, clutch_rate: f64) -> TextBox {
    let fmt_pct = |v: f64| {
        let s = format!("{:.1}", v);
        format!("{}%", s.strip_suffix(".0").unwrap_or(&s))
    };
    let fmt_val = |v: f64| {
        let s = format!("{:.1}", v);
        s.strip_suffix(".0").unwrap_or(&s).to_string()
    };

    let wr_color = match win_rate {
        v if v >= 80.0 => NamedColor::DarkPurple,
        v if v >= 65.0 => NamedColor::Red,
        v if v >= 50.0 => NamedColor::Gold,
        v if v >= 35.0 => NamedColor::Green,
        _ => NamedColor::Gray,
    };
    let fps_color = match finals_per_star {
        v if v >= 100.0 => NamedColor::DarkPurple,
        v if v >= 60.0 => NamedColor::Red,
        v if v >= 35.0 => NamedColor::Gold,
        v if v >= 15.0 => NamedColor::Green,
        _ => NamedColor::Gray,
    };
    let cr_color = match clutch_rate {
        v if v >= 85.0 => NamedColor::DarkPurple,
        v if v >= 70.0 => NamedColor::Red,
        v if v >= 55.0 => NamedColor::Gold,
        v if v >= 40.0 => NamedColor::Green,
        _ => NamedColor::Gray,
    };

    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .padding(12, 12).scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(stat_line("Win Rate: ", &fmt_pct(win_rate), wr_color))
        .push(stat_line("Finals/\u{2606}: ", &fmt_val(finals_per_star), fps_color))
        .push(stat_line("Clutch: ", &fmt_pct(clutch_rate), cr_color))
}


pub struct VerticalGamesBox<'a> {
    mode_games: &'a ModeGames,
    width: u32,
    height: u32,
}


impl<'a> VerticalGamesBox<'a> {
    pub fn new(mode_games: &'a ModeGames, width: u32, height: u32) -> Self {
        Self { mode_games, width, height }
    }
}


impl Shape for VerticalGamesBox<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        let padding = 12u32;
        let scale = 1.5f32;
        let font = scale * 16.0;
        let label_scale = 1.25f32;
        let label_font = label_scale * 16.0;

        RoundedRect::new(self.width, self.height)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let total = self.mode_games.total();
        let entries = self.mode_games.entries();

        let title = MCText::new()
            .span("Games: ").color(NamedColor::Gray)
            .then(&format_number(total)).color(NamedColor::White)
            .build();
        let (_, title_h) = ctx.renderer.measure(&title, font);
        TextBlock::new()
            .push(title).scale(scale).align_x(Align::Center).max_width(self.width)
            .draw(&mut ctx.at(0, padding as i32));

        if entries.is_empty() { return; }

        let sample_label = MCText::new().span("4v4").color(NamedColor::Gray).build();
        let (_, label_h) = ctx.renderer.measure(&sample_label, label_font);

        let bar_top = padding + title_h as u32 + 8;
        let bar_bottom = self.height - padding - label_h as u32 - 4;
        let max_bar_h = bar_bottom.saturating_sub(bar_top);
        let inner_w = self.width - padding * 2;
        let max_count = entries.iter().map(|e| e.count).max().unwrap_or(1).max(1);

        let bar_count = entries.len() as u32;
        let gap = 6u32;
        let bar_w = inner_w.saturating_sub(gap * bar_count.saturating_sub(1)) / bar_count;
        let origin = (ctx.x, ctx.y);
        let (bw, bh) = ctx.buffer.dimensions();

        for (i, entry) in entries.iter().enumerate() {
            let x = padding + i as u32 * (bar_w + gap);
            let bar_h = (entry.count as f64 / max_count as f64 * max_bar_h as f64).round() as u32;
            let bar_h = bar_h.max(2);
            let bar_y = bar_bottom - bar_h;

            let bg_color = Rgba([50, 50, 55, 220]);
            for py in bar_y..bar_bottom {
                for px in x..x + bar_w {
                    let abs_x = (origin.0 + px as i32) as u32;
                    let abs_y = (origin.1 + py as i32) as u32;
                    if abs_x < bw && abs_y < bh {
                        let bg = *ctx.buffer.get_pixel(abs_x, abs_y);
                        ctx.buffer.put_pixel(abs_x, abs_y, blend(bg, bg_color));
                    }
                }
            }

            if total > 0 && entry.count > 0 {
                let pct = (entry.count as f64 / total as f64 * 100.0).round() as u32;
                let pct_text = MCText::new().span(&format!("{}%", pct)).color(NamedColor::Green).build();
                let pct_font = 1.1f32 * 16.0;
                let (pw, ph) = ctx.renderer.measure(&pct_text, pct_font);
                if (ph as u32) + 4 <= bar_h && (pw as u32) + 2 <= bar_w {
                    ctx.renderer.draw(
                        ctx.buffer.as_mut(), bw, bh,
                        origin.0 as f32 + x as f32 + (bar_w as f32 - pw) / 2.0,
                        (origin.1 + bar_y as i32 + ((bar_h as f32 - ph) / 2.0) as i32) as f32,
                        &pct_text, pct_font, true,
                    );
                }
            }

            let label = MCText::new().span(entry.label).color(entry.color).build();
            let (lw, _) = ctx.renderer.measure(&label, label_font);
            ctx.renderer.draw(
                ctx.buffer.as_mut(), bw, bh,
                origin.0 as f32 + x as f32 + (bar_w as f32 - lw) / 2.0,
                (origin.1 + (bar_bottom + 4) as i32) as f32,
                &label, label_font, true,
            );
        }
    }

    fn size(&self) -> (u32, u32) { (self.width, self.height) }
}


fn session_box(
    _session_type: &SessionType,
    started: DateTime<Utc>,
    ended: Option<DateTime<Utc>>,
) -> TextBox {
    let end_time = ended.unwrap_or_else(Utc::now);
    let duration = end_time.signed_duration_since(started);
    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    let duration_str = match (days, hours, minutes) {
        (d, h, _) if d > 0 && h > 0 => format!("{d}d {h}h"),
        (d, _, _) if d > 0 => format!("{d}d"),
        (_, h, m) if h > 0 && m > 0 => format!("{h}h {m}m"),
        (_, h, _) if h > 0 => format!("{h}h"),
        (_, _, m) => format!("{m}m"),
    };

    let start_str = started.format("%m/%d/%y %H:%M").to_string();
    let end_str = if ended.is_some() {
        end_time.format("%m/%d/%y %H:%M").to_string()
    } else {
        "Now".to_string()
    };

    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .padding(12, 12).scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(MCText::new().span("Start: ").color(NamedColor::Gray).then(&start_str).color(NamedColor::White).build())
        .push(MCText::new().span("End: ").color(NamedColor::Gray).then(&end_str).color(NamedColor::White).build())
        .push(MCText::new().span("Duration: ").color(NamedColor::Gray).then(&duration_str).color(NamedColor::White).build())
}


struct GuildBox<'a> {
    stats: &'a BedwarsPlayerStats,
}


impl Shape for GuildBox<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        RoundedRect::new(COL_WIDTH, BOTTOM_BOX_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let scale = 1.5;
        let font = scale * 16.0;
        let padding = 12u32;
        let inner_w = COL_WIDTH - padding * 2;

        let name = self.stats.guild.name.as_deref().unwrap_or("-");
        let rank = self.stats.guild.rank.as_deref().unwrap_or("N/A");
        let joined = self.stats.guild.joined.map(format_timestamp).unwrap_or_else(|| "N/A".to_string());
        let color = self.stats.guild.tag_color.as_ref()
            .and_then(|c| color_name_to_named(c))
            .unwrap_or(NamedColor::Gray);

        let lines = [
            MCText::new().span(name).color(color).build(),
            MCText::new().span("Rank: ").color(NamedColor::Gray).then(rank).color(color).build(),
            MCText::new().span("Joined: ").color(NamedColor::Gray).then(&joined).color(NamedColor::White).build(),
        ];

        let measurements: Vec<(f32, f32)> = lines.iter().map(|l| ctx.renderer.measure(l, font)).collect();
        let total_h: f32 = measurements.iter().map(|(_, h)| h).sum();
        let spacing = (BOTTOM_BOX_HEIGHT as f32 - padding as f32 * 2.0 - total_h)
            / (lines.len() - 1).max(1) as f32;

        let mut y = padding as f32;
        for (line, (tw, lh)) in lines.into_iter().zip(measurements) {
            let effective_h = if tw > inner_w as f32 {
                ctx.renderer.measure(&line, scale * (inner_w as f32 / tw) * 16.0).1
            } else {
                lh
            };
            let y_offset = (lh - effective_h) / 2.0;
            TextBlock::new()
                .push(line).scale(scale).max_width(inner_w).align_x(Align::Center)
                .draw(&mut ctx.at(padding as i32, (y + y_offset) as i32));
            y += lh + spacing;
        }
    }

    fn size(&self) -> (u32, u32) { (COL_WIDTH, BOTTOM_BOX_HEIGHT) }
}


struct HeaderSection<'a> {
    stats: &'a BedwarsPlayerStats,
    tags: &'a [TagIcon],
}


impl<'a> HeaderSection<'a> {
    fn new(stats: &'a BedwarsPlayerStats, tags: &'a [TagIcon]) -> Self { Self { stats, tags } }

    fn display_name_text(&self) -> MCText {
        let prefix = self.stats.rank_prefix.as_deref().unwrap_or("\u{00a7}7");
        let guild_tag = match (&self.stats.guild.tag, &self.stats.guild.tag_color) {
            (Some(tag), Some(color)) => format!(" {}[{}]", color_code(color), tag),
            (Some(tag), None) => format!(" \u{00a7}7[{}]", tag),
            _ => String::new(),
        };
        MCText::parse(&format!("{}{}{}", prefix, self.stats.display_name, guild_tag))
    }
}


impl Shape for HeaderSection<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        RoundedRect::new(CANVAS_WIDTH, HEADER_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let name_text = self.display_name_text();
        let name_scale = 2.75;
        let name_font = name_scale * 16.0;
        let (cw, ch) = ctx.buffer.dimensions();
        let (name_w, _) = ctx.renderer.measure(&name_text, name_font);

        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            (ctx.x + 20) as f32, (ctx.y + 13) as f32,
            &name_text, name_font, true,
        );

        if !self.tags.is_empty() {
            let icon_size = (name_scale * 12.0) as u32;
            let icon_gap = 4;
            let mut icon_x = 20.0 + name_w + 8.0;
            let icon_y = 13.0 + (name_font - icon_size as f32) / 2.0;
            for (icon_name, color) in self.tags {
                if let Some(icon) = crate::icons::tag_icon(icon_name, icon_size, *color) {
                    Image::new(&icon).draw(&mut ctx.at(icon_x as i32, icon_y as i32));
                    icon_x += icon_size as f32 + icon_gap as f32;
                }
            }
        }
    }

    fn size(&self) -> (u32, u32) { (CANVAS_WIDTH, HEADER_HEIGHT) }
}


struct LevelSection {
    current_exp: u64,
    stars_gained: f64,
}


impl LevelSection {
    fn new(current_exp: u64, stars_gained: f64) -> Self { Self { current_exp, stars_gained } }

    fn current_level_text(&self) -> MCText {
        let level = hypixel::calculate_level(self.current_exp) as u32;
        let star = prestige_star(level);
        build_prestige_text(&format!("[{}{}]", level, star), prestige_colors(level))
    }

    fn stars_gained_text(&self) -> MCText {
        let level = hypixel::calculate_level(self.current_exp) as u32;
        let star = prestige_star(level);
        let colors = prestige_colors(level);

        let s = format!("+{:.2}", self.stars_gained);
        let value = s.strip_suffix(".00").unwrap_or(&s);
        let num_color = if colors.len() > 6 { colors[1] } else { colors[0] };
        let star_color = colors[colors.len() - 2];

        MCText::parse(&format!("\u{00a7}{}{}\u{00a7}{}{}", num_color, value, star_color, star))
    }

    fn progress_bar_text(&self) -> MCText {
        let progress = level_progress(self.current_exp);
        let filled = (progress * 25.0).round() as usize;
        MCText::new()
            .span("[").color(NamedColor::DarkGray)
            .then(&"\u{25a0}".repeat(filled)).color(NamedColor::Aqua)
            .then(&"\u{25a0}".repeat(25 - filled)).color(NamedColor::Gray)
            .then("]").color(NamedColor::DarkGray)
            .build()
    }
}


impl Shape for LevelSection {
    fn draw(&self, ctx: &mut DrawContext) {
        let section_height = 53.0;
        let bottom_padding = 13.0;
        let font_size = LEVEL_SCALE * 16.0;
        let available_width = CANVAS_WIDTH - 2 * LEVEL_PADDING;

        let current_level = self.current_level_text();
        let stars_gained = self.stars_gained_text();
        let progress_bar = self.progress_bar_text();

        let (level_w, star_h) = ctx.renderer.measure(&current_level, font_size);
        let (gained_w, _) = ctx.renderer.measure(&stars_gained, font_size);
        let spacing = font_size * 0.3;
        let bar_available = available_width as f32 - level_w - gained_w - spacing * 2.0;

        let (bar_w, bar_h) = ctx.renderer.measure(&progress_bar, font_size);
        let (bar_scale, scaled_bar_w, bar_h) = if bar_w > bar_available {
            let s = LEVEL_SCALE * (bar_available / bar_w);
            let (w, h) = ctx.renderer.measure(&progress_bar, s * 16.0);
            (s, w, h)
        } else {
            (LEVEL_SCALE, bar_w, bar_h)
        };

        let total_w = level_w + spacing + scaled_bar_w + spacing + gained_w;
        let start_x = LEVEL_PADDING as f32 + (available_width as f32 - total_w) / 2.0;
        let star_y = section_height - star_h - bottom_padding;
        let star_center_y = star_y + star_h / 2.0;
        let bar_y = (star_center_y - bar_h / 2.0) as i32;
        let star_y = star_y as i32;
        let (cw, ch) = ctx.buffer.dimensions();

        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + start_x, (ctx.y + star_y) as f32,
            &current_level, font_size, true,
        );
        let bar_x = start_x + level_w + spacing;
        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + bar_x, ctx.y as f32 + bar_y as f32,
            &progress_bar, bar_scale * 16.0, true,
        );
        let gained_x = bar_x + scaled_bar_w + spacing;
        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + gained_x, (ctx.y + star_y) as f32,
            &stars_gained, font_size, true,
        );
    }

    fn size(&self) -> (u32, u32) { (CANVAS_WIDTH, 53) }
}


struct SkinSection<'a> {
    skin: Option<&'a DynamicImage>,
    mode_label: &'a str,
    network_level: f64,
}


impl<'a> SkinSection<'a> {
    fn new(skin: Option<&'a DynamicImage>, mode_label: &'a str, network_level: f64) -> Self {
        Self { skin, mode_label, network_level }
    }
}


impl Shape for SkinSection<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        RoundedRect::new(COL_WIDTH, SKIN_BOX_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let level_scale = 2.0;
        let mode_scale = 1.5;
        let level_text_height = (level_scale * 16.0) as u32;
        let mode_text_height = (mode_scale * 16.0) as u32;

        let level_text = MCText::new()
            .span("Level ").color(NamedColor::Gray)
            .then(&{
                let s = format!("{:.2}", self.network_level);
                s.strip_suffix(".00").map(String::from).unwrap_or(s)
            })
            .color(NamedColor::Yellow)
            .build();
        TextBlock::new()
            .push(level_text).scale(level_scale).align_x(Align::Center).max_width(COL_WIDTH)
            .draw(&mut ctx.at(0, SKIN_PADDING as i32));

        let mode_text = MCText::new()
            .span(&format!("({})", self.mode_label)).color(NamedColor::Gray)
            .build();
        let mode_y = SKIN_BOX_HEIGHT - SKIN_PADDING - mode_text_height;
        TextBlock::new()
            .push(mode_text).scale(mode_scale).align_x(Align::Center).max_width(COL_WIDTH)
            .draw(&mut ctx.at(0, mode_y as i32));

        if let Some(skin) = &self.skin {
            let level_bottom = SKIN_PADDING + level_text_height;
            let available_h = mode_y - level_bottom;
            let max_w = COL_WIDTH - 26;
            let (orig_w, orig_h) = (skin.width(), skin.height());
            let scale = f64::min(max_w as f64 / orig_w as f64, available_h as f64 / orig_h as f64);
            let new_w = (orig_w as f64 * scale) as u32;
            let new_h = (orig_h as f64 * scale) as u32;
            let skin_x = (COL_WIDTH - new_w) / 2;
            let skin_y = level_bottom + (available_h - new_h) / 2 + 12;
            Image::new(skin).size(new_w, new_h).draw(&mut ctx.at(skin_x as i32, skin_y as i32));
        }
    }

    fn size(&self) -> (u32, u32) { (COL_WIDTH, SKIN_BOX_HEIGHT) }
}
