use image::{DynamicImage, RgbaImage};
use mctext::{MCText, NamedColor};

use hypixel::{
    BedwarsPlayerStats, Mode, ModeStats, SlumberInfo, StreakSource, WinstreakHistory,
    color_code, combined_mode_name, level_progress,
};

use crate::canvas::{
    Align, BOX_BACKGROUND, CANVAS_BACKGROUND, Canvas, DrawContext, Image, RoundedRect, Shape,
    TextBlock, TextBox,
};

use super::common::{
    BAR_COLOR, color_name_to_named, colors, draw_progress_bar, format_number, format_percent,
    format_ratio, format_timestamp,
};
use super::prestiges::{build_prestige_text, prestige_colors, prestige_star};
use super::session::{ModeGames, VerticalGamesBox};

const CANVAS_WIDTH: u32 = 800;
const CANVAS_HEIGHT: u32 = 600;
const COL_WIDTH: u32 = 256;
const HEADER_Y: u32 = 0;
const HEADER_HEIGHT: u32 = 100;
const LEVEL_Y: u32 = 57;
const MAIN_ROW_Y: u32 = 116;
const BOTTOM_ROW_Y: u32 = 500;
const BOTTOM_BOX_HEIGHT: u32 = 100;
const STATS_BOX_HEIGHT: u32 = 176;
const SKIN_BOX_HEIGHT: u32 = 368;
const SECOND_ROW_Y: u32 = MAIN_ROW_Y + STATS_BOX_HEIGHT + 16;
const SECOND_ROW_HEIGHT: u32 = 176;
const MAX_DISPLAYED_STREAKS: usize = 5;
const BOX_CORNER_RADIUS: u32 = 18;
const STATS_BOX_WIDTH: u32 = 528;
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


pub type TagIcon = (String, u32);


pub fn render_bedwars(
    stats: &BedwarsPlayerStats,
    modes: &[Mode],
    skin: Option<&DynamicImage>,
    winstreaks: &WinstreakHistory,
    tags: &[TagIcon],
) -> RgbaImage {
    let mode_stats = stats.get_combined_mode_stats(modes);
    let mode_label = combined_mode_name(modes);
    let mode_games = ModeGames {
        solos: stats.solos.wins + stats.solos.losses,
        doubles: stats.doubles.wins + stats.doubles.losses,
        threes: stats.threes.wins + stats.threes.losses,
        fours: stats.fours.wins + stats.fours.losses,
        four_v_four: stats.four_v_four.wins + stats.four_v_four.losses,
    };

    let canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT)
        .background(CANVAS_BACKGROUND)
        .draw(0, HEADER_Y as i32, &HeaderSection::new(stats, tags))
        .draw(0, LEVEL_Y as i32, &LevelSection::new(stats))
        .draw(col_x(0) as i32, MAIN_ROW_Y as i32, &SkinSection::new(skin, &mode_label, stats.network_level))
        .draw(col_x(1) as i32, MAIN_ROW_Y as i32, &StatsSection::new(&mode_stats));

    let is_single_non_overall = modes.len() == 1 && modes[0] != Mode::Overall;
    let canvas = if is_single_non_overall {
        canvas.draw(
            col_x(1) as i32, SECOND_ROW_Y as i32,
            &ModeShareBox::new(&mode_stats, &stats.overall),
        )
    } else {
        canvas.draw(
            col_x(1) as i32, SECOND_ROW_Y as i32,
            &VerticalGamesBox::new(&mode_games, COL_WIDTH, SECOND_ROW_HEIGHT),
        )
    };

    canvas
        .draw(col_x(2) as i32, SECOND_ROW_Y as i32, &WinstreaksBox { winstreaks, current_ws: mode_stats.winstreak })
        .draw(col_x(0) as i32, BOTTOM_ROW_Y as i32, &status_box(stats))
        .draw(col_x(1) as i32, BOTTOM_ROW_Y as i32, &GuildBox { stats })
        .draw(col_x(2) as i32, BOTTOM_ROW_Y as i32, &slumber_box(&stats.slumber))
        .build()
}


struct StatsSection<'a> {
    stats: &'a ModeStats,
}


impl<'a> StatsSection<'a> {
    fn new(stats: &'a ModeStats) -> Self { Self { stats } }
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
            ("WLR:", "Wins:", self.stats.wlr(), self.stats.wins, self.stats.losses, colors::wlr(self.stats.wlr()), colors::wins(self.stats.wins)),
            ("FKDR:", "Finals:", self.stats.fkdr(), self.stats.final_kills, self.stats.final_deaths, colors::fkdr(self.stats.fkdr()), colors::final_kills(self.stats.final_kills)),
            ("KDR:", "Kills:", self.stats.kdr(), self.stats.kills, self.stats.deaths, colors::kdr(self.stats.kdr()), colors::kills(self.stats.kills)),
            ("BBLR:", "Beds:", self.stats.bblr(), self.stats.beds_broken, self.stats.beds_lost, colors::bblr(self.stats.bblr()), colors::beds_broken(self.stats.beds_broken)),
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


struct ModeShareBox<'a> {
    mode: &'a ModeStats,
    overall: &'a ModeStats,
}


impl<'a> ModeShareBox<'a> {
    fn new(mode: &'a ModeStats, overall: &'a ModeStats) -> Self { Self { mode, overall } }

    fn pct(mode_val: u64, overall_val: u64) -> f64 {
        if overall_val == 0 { 0.0 } else { mode_val as f64 / overall_val as f64 * 100.0 }
    }
}


impl Shape for ModeShareBox<'_> {
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
            ("Wins", Self::pct(self.mode.wins, self.overall.wins)),
            ("Finals", Self::pct(self.mode.final_kills, self.overall.final_kills)),
            ("Kills", Self::pct(self.mode.kills, self.overall.kills)),
            ("Beds", Self::pct(self.mode.beds_broken, self.overall.beds_broken)),
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


fn status_box(stats: &BedwarsPlayerStats) -> TextBox {
    let status = MCText::new()
        .span("Status: ").color(NamedColor::Gray)
        .then("N/A").color(NamedColor::Gray)
        .build();
    let last_login = MCText::new()
        .span("Last Login: ").color(NamedColor::Gray)
        .then("N/A").color(NamedColor::Gray)
        .build();
    let first_login = stats
        .first_login
        .map(|ts| {
            MCText::new()
                .span("First Login: ").color(NamedColor::Gray)
                .then(&format_timestamp(ts)).color(NamedColor::White)
                .build()
        })
        .unwrap_or_else(|| {
            MCText::new()
                .span("First Login: ").color(NamedColor::Gray)
                .then("N/A").color(NamedColor::Gray)
                .build()
        });

    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .corner_radius(BOX_CORNER_RADIUS).padding(12, 12)
        .scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(status).push(last_login).push(first_login)
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


fn slumber_box(slumber: &SlumberInfo) -> TextBox {
    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .corner_radius(BOX_CORNER_RADIUS).padding(12, 12)
        .scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(
            MCText::new()
                .span("Tickets: ").color(NamedColor::Gray)
                .then(&format_number(slumber.tickets)).color(NamedColor::Aqua)
                .build(),
        )
        .push(
            MCText::new()
                .span("Lifetime: ").color(NamedColor::Gray)
                .then(&format_number(slumber.total_tickets_earned)).color(NamedColor::DarkAqua)
                .build(),
        )
        .push(
            MCText::new()
                .span("XP Doublers: ").color(NamedColor::Gray)
                .then(&format_number(slumber.doublers)).color(NamedColor::Green)
                .build(),
        )
}


struct WinstreaksBox<'a> {
    winstreaks: &'a WinstreakHistory,
    current_ws: Option<u64>,
}


impl Shape for WinstreaksBox<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        let padding = 12u32;
        let scale = 1.5f32;
        let font = scale * 16.0;
        let inner_w = COL_WIDTH - padding * 2;

        RoundedRect::new(COL_WIDTH, SECOND_ROW_HEIGHT)
            .corner_radius(BOX_CORNER_RADIUS)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let current_line = match self.current_ws {
            Some(ws) => MCText::new()
                .span("Winstreak: ").color(NamedColor::Gray)
                .then(&format_number(ws)).color(colors::winstreak(ws))
                .build(),
            None => MCText::new()
                .span("Winstreak: ").color(NamedColor::Gray)
                .then("?").color(NamedColor::Red)
                .build(),
        };

        let (_, line_h) = ctx.renderer.measure(&current_line, font);
        let mut y = padding as f32;
        TextBlock::new().push(current_line).scale(scale).draw(&mut ctx.at(padding as i32, y as i32));
        y += line_h;

        let display_count = self.winstreaks.streaks.len().min(MAX_DISPLAYED_STREAKS);
        let icon_size = 20u32;
        let icon_radius = 8u32;
        let icon_gap = 4u32;
        let urchin_icon = crate::icons::urchin(icon_size, icon_radius);
        let antisniper_icon = crate::icons::antisniper(icon_size, icon_radius);

        for (i, streak) in self.winstreaks.streaks[..display_count].iter().enumerate() {
            let suffix = if streak.approximate { "+" } else { "" };
            let date = format_timestamp(streak.timestamp.timestamp_millis());
            let color = colors::winstreak(streak.value);
            let rank = format!("{}.", i + 1);

            let icon = match streak.source {
                StreakSource::Urchin => &urchin_icon,
                StreakSource::Antisniper => &antisniper_icon,
            };
            Image::new(icon).draw(&mut ctx.at(padding as i32, (y + (line_h - icon_size as f32) / 2.0) as i32));

            let text_x = padding + icon_size + icon_gap;
            let left = MCText::new()
                .span(&rank).color(NamedColor::DarkGray)
                .then(" ").then(&format!("{}{}", format_number(streak.value), suffix)).color(color)
                .build();
            let right = MCText::new()
                .span("- ").color(NamedColor::DarkGray)
                .then(&date).color(NamedColor::Gray)
                .build();

            TextBlock::new().push(left).scale(scale).draw(&mut ctx.at(text_x as i32, y as i32));
            let (rw, _) = ctx.renderer.measure(&right, font);
            TextBlock::new().push(right).scale(scale)
                .draw(&mut ctx.at((padding as f32 + inner_w as f32 - rw) as i32, y as i32));
            y += line_h;
        }
    }

    fn size(&self) -> (u32, u32) { (COL_WIDTH, SECOND_ROW_HEIGHT) }
}


struct HeaderSection<'a> {
    stats: &'a BedwarsPlayerStats,
    tags: &'a [TagIcon],
}


impl<'a> HeaderSection<'a> {
    fn new(stats: &'a BedwarsPlayerStats, tags: &'a [TagIcon]) -> Self { Self { stats, tags } }

    fn display_name_text(&self) -> MCText {
        let prefix = self.stats.rank_prefix.as_deref().unwrap_or("§7");
        let guild_tag = match (&self.stats.guild.tag, &self.stats.guild.tag_color) {
            (Some(tag), Some(color)) => format!(" {}[{}]", color_code(color), tag),
            (Some(tag), None) => format!(" §7[{}]", tag),
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


struct LevelSection<'a> {
    stats: &'a BedwarsPlayerStats,
}


impl<'a> LevelSection<'a> {
    fn new(stats: &'a BedwarsPlayerStats) -> Self { Self { stats } }

    fn current_star_text(&self) -> MCText {
        let star = prestige_star(self.stats.level);
        build_prestige_text(&format!("[{}{}]", self.stats.level, star), prestige_colors(self.stats.level))
    }

    fn next_star_text(&self) -> MCText {
        let star = prestige_star(self.stats.level);
        build_prestige_text(&format!("[{}{}]", self.stats.level + 1, star), prestige_colors(self.stats.level + 1))
    }

    fn progress_bar_text(&self) -> MCText {
        let progress = level_progress(self.stats.experience);
        let filled = (progress * 25.0).round() as usize;
        MCText::new()
            .span("[").color(NamedColor::DarkGray)
            .then(&"\u{25a0}".repeat(filled)).color(NamedColor::Aqua)
            .then(&"\u{25a0}".repeat(25 - filled)).color(NamedColor::Gray)
            .then("]").color(NamedColor::DarkGray)
            .build()
    }
}


impl Shape for LevelSection<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        let section_height = 53.0;
        let bottom_padding = 13.0;
        let font_size = LEVEL_SCALE * 16.0;
        let available_width = CANVAS_WIDTH - 2 * LEVEL_PADDING;

        let current_star = self.current_star_text();
        let next_star = self.next_star_text();
        let progress_bar = self.progress_bar_text();

        let (current_w, star_h) = ctx.renderer.measure(&current_star, font_size);
        let (next_w, _) = ctx.renderer.measure(&next_star, font_size);
        let spacing = font_size * 0.3;
        let bar_available = available_width as f32 - current_w - next_w - spacing * 2.0;

        let (bar_w, bar_h) = ctx.renderer.measure(&progress_bar, font_size);
        let (bar_scale, scaled_bar_w, bar_h) = if bar_w > bar_available {
            let s = LEVEL_SCALE * (bar_available / bar_w);
            let (w, h) = ctx.renderer.measure(&progress_bar, s * 16.0);
            (s, w, h)
        } else {
            (LEVEL_SCALE, bar_w, bar_h)
        };

        let total_w = current_w + spacing + scaled_bar_w + spacing + next_w;
        let start_x = LEVEL_PADDING as f32 + (available_width as f32 - total_w) / 2.0;
        let star_y = section_height - star_h - bottom_padding;
        let star_center_y = star_y + star_h / 2.0;
        let bar_y = (star_center_y - bar_h / 2.0) as i32;
        let star_y = star_y as i32;
        let (cw, ch) = ctx.buffer.dimensions();

        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + start_x, (ctx.y + star_y) as f32,
            &current_star, font_size, true,
        );
        let bar_x = start_x + current_w + spacing;
        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + bar_x, ctx.y as f32 + bar_y as f32,
            &progress_bar, bar_scale * 16.0, true,
        );
        let next_x = bar_x + scaled_bar_w + spacing;
        ctx.renderer.draw(
            ctx.buffer.as_mut(), cw, ch,
            ctx.x as f32 + next_x, (ctx.y + star_y) as f32,
            &next_star, font_size, true,
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
