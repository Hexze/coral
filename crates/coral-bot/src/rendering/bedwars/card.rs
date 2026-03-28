use std::time::Instant;

use hypixel::player::color_code;
use hypixel::stats::bedwars::{self, Mode, ModeStats, Stats, combined_mode_name};
use image::DynamicImage;
use render::canvas::{
    BOX_BACKGROUND, CANVAS_BACKGROUND, Canvas, DrawContext, MCText, NamedColor, RgbaImage, Shape,
    shapes::{Align, Image, RoundedRect, TextBlock, TextBox},
};
use tracing::info;

use super::layout::*;
use crate::rendering::common::{self, *};


pub fn render(stats: &Stats, modes: &[Mode], skin: Option<&DynamicImage>) -> RgbaImage {
    let mode_stats = stats.get_combined_mode_stats(modes);

    let t0 = Instant::now();
    let canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT).background(CANVAS_BACKGROUND);
    let t_init = t0.elapsed();

    let t0 = Instant::now();
    let canvas = canvas.draw(0, HEADER_Y as i32, &HeaderSection::new(stats));
    let t_header = t0.elapsed();

    let t0 = Instant::now();
    let canvas = canvas.draw(0, LEVEL_Y as i32, &LevelSection::new(stats));
    let t_level = t0.elapsed();

    let t0 = Instant::now();
    let canvas = canvas.draw(col_x(0) as i32, MAIN_ROW_Y as i32, &SkinSection::new(skin, &combined_mode_name(modes)));
    let t_skin = t0.elapsed();

    let t0 = Instant::now();
    let canvas = canvas
        .draw(col_x(1) as i32, MAIN_ROW_Y as i32, &ratios_box(&mode_stats))
        .draw(col_x(1) as i32, SECOND_ROW_Y as i32, &general_box(stats))
        .draw(col_x(2) as i32, MAIN_ROW_Y as i32, &positives_box(&mode_stats))
        .draw(col_x(2) as i32, SECOND_ROW_Y as i32, &winstreaks_box())
        .draw(col_x(0) as i32, BOTTOM_ROW_Y as i32, &guild_box(stats))
        .draw(col_x(1) as i32, BOTTOM_ROW_Y as i32, &status_box(stats, &mode_stats))
        .draw(col_x(2) as i32, BOTTOM_ROW_Y as i32, &negatives_box(&mode_stats));
    let t_boxes = t0.elapsed();

    let t0 = Instant::now();
    let result = canvas.build();
    let t_build = t0.elapsed();

    info!(
        "render breakdown: init={:?} header={:?} level={:?} skin={:?} boxes={:?} build={:?}",
        t_init, t_header, t_level, t_skin, t_boxes, t_build
    );

    result
}


fn ratios_box(stats: &ModeStats) -> TextBox {
    let (wlr, fkdr, kdr, bblr) = (stats.wlr(), stats.fkdr(), stats.kdr(), stats.bblr());

    TextBox::new()
        .width(COL_WIDTH).height(MAIN_BOX_HEIGHT)
        .padding(12, 16).scale(2.25).line_spacing(0.0)
        .align_y(Align::Spread)
        .push(stat_line("WLR: ", &format_ratio(wlr), common::wlr(wlr)))
        .push(stat_line("FKDR: ", &format_ratio(fkdr), common::fkdr(fkdr)))
        .push(stat_line("KDR: ", &format_ratio(kdr), common::kdr(kdr)))
        .push(stat_line("BBLR: ", &format_ratio(bblr), common::bblr(bblr)))
}


fn general_box(stats: &Stats) -> TextBox {
    let gexp = stats.guild.weekly_gexp
        .map(format_number)
        .unwrap_or_else(|| "N/A".to_string());

    TextBox::new()
        .width(COL_WIDTH).height(MAIN_BOX_HEIGHT)
        .padding(12, 16).scale(2.25).line_spacing(0.0)
        .align_y(Align::Spread)
        .push(stat_line("Level: ", &format!("{:.2}", stats.network_level), NamedColor::Yellow))
        .push(stat_line("AP: ", &format_number(stats.achievement_points), NamedColor::White))
        .push(stat_line("Ranks: ", &format_number(stats.ranks_gifted), NamedColor::Gold))
        .push(stat_line("GEXP: ", &gexp, NamedColor::DarkGreen))
}


fn positives_box(stats: &ModeStats) -> TextBox {
    TextBox::new()
        .width(COL_WIDTH).height(MAIN_BOX_HEIGHT)
        .padding(12, 16).scale(2.25).line_spacing(0.0)
        .align_y(Align::Spread)
        .push(stat_line("Wins: ", &format_number(stats.wins), common::wins(stats.wins)))
        .push(stat_line("Finals: ", &format_number(stats.final_kills), common::final_kills(stats.final_kills)))
        .push(stat_line("Kills: ", &format_number(stats.kills), common::kills(stats.kills)))
        .push(stat_line("Beds: ", &format_number(stats.beds_broken), common::beds_broken(stats.beds_broken)))
}


fn winstreaks_box() -> TextBox {
    TextBox::new()
        .width(COL_WIDTH).height(MAIN_BOX_HEIGHT)
        .padding(12, 12).scale(1.25).line_spacing(3.3)
        .align_y(Align::Spread)
        .push(MCText::new().span("Winstreaks").color(NamedColor::Gray).build())
}


fn guild_box(stats: &Stats) -> TextBox {
    let name = stats.guild.name.as_deref().unwrap_or("-");
    let rank = stats.guild.rank.as_deref().unwrap_or("N/A");
    let joined = stats.guild.joined
        .map(format_timestamp)
        .unwrap_or_else(|| "N/A".to_string());
    let color = stats.guild.tag_color.as_ref()
        .and_then(|c| color_name_to_named(c))
        .unwrap_or(NamedColor::Gray);

    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .padding(12, 12).scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(MCText::new().span(name).color(color).build())
        .push(MCText::new().span("Rank: ").color(NamedColor::Gray).then(rank).color(color).build())
        .push(MCText::new().span("Joined: ").color(NamedColor::Gray).then(&joined).color(NamedColor::White).build())
}


fn status_box(stats: &Stats, mode_stats: &ModeStats) -> TextBox {
    let ws_text = match mode_stats.winstreak {
        Some(ws) => MCText::new()
            .span("Winstreak: ").color(NamedColor::Gray)
            .then(&format_number(ws)).color(common::winstreak(ws))
            .build(),
        None => MCText::new()
            .span("Winstreak: ").color(NamedColor::Gray)
            .then("?").color(NamedColor::Red)
            .build(),
    };

    let status = MCText::new()
        .span("Status: ").color(NamedColor::Gray)
        .then("N/A").color(NamedColor::Gray)
        .build();

    let login = stats.first_login
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
        .padding(12, 12).scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(ws_text)
        .push(status)
        .push(login)
}


fn negatives_box(stats: &ModeStats) -> TextBox {
    TextBox::new()
        .width(COL_WIDTH).height(BOTTOM_BOX_HEIGHT)
        .padding(12, 10).scale(1.5).line_spacing(0.0)
        .align_x(Align::Center).align_y(Align::Spread)
        .push(MCText::new().span("Losses: ").color(NamedColor::Gray).then(&format_number(stats.losses)).color(NamedColor::Gray).build())
        .push(MCText::new().span("Final Deaths: ").color(NamedColor::Gray).then(&format_number(stats.final_deaths)).color(NamedColor::Gray).build())
        .push(MCText::new().span("Beds Lost: ").color(NamedColor::Gray).then(&format_number(stats.beds_lost)).color(NamedColor::Gray).build())
}


struct HeaderSection<'a> {
    stats: &'a Stats,
}


impl<'a> HeaderSection<'a> {
    fn new(stats: &'a Stats) -> Self {
        Self { stats }
    }

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
            .corner_radius(20)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        let mut name_ctx = ctx.at(20, 13);
        TextBlock::new()
            .push(self.display_name_text())
            .scale(2.75)
            .draw(&mut name_ctx);
    }

    fn size(&self) -> (u32, u32) {
        (CANVAS_WIDTH, HEADER_HEIGHT)
    }
}


struct LevelSection<'a> {
    stats: &'a Stats,
}

const LEVEL_SCALE: f32 = 2.75;
const LEVEL_PADDING: u32 = 20;


impl<'a> LevelSection<'a> {
    fn new(stats: &'a Stats) -> Self {
        Self { stats }
    }

    fn current_star_text(&self) -> MCText {
        let star = prestige_star(self.stats.level);
        build_prestige_text(
            &format!("[{}{}]", self.stats.level, star),
            &prestige_colors(self.stats.level),
        )
    }

    fn next_star_text(&self) -> MCText {
        let star = prestige_star(self.stats.level);
        build_prestige_text(
            &format!("[{}{}]", self.stats.level + 1, star),
            &prestige_colors(self.stats.level + 1),
        )
    }

    fn progress_bar_text(&self) -> MCText {
        let filled = (bedwars::level_progress(self.stats.experience) * 25.0).round() as usize;
        MCText::new()
            .span("[").color(NamedColor::DarkGray)
            .then(&"■".repeat(filled)).color(NamedColor::Aqua)
            .then(&"■".repeat(25 - filled)).color(NamedColor::Gray)
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
        let (bar_w, _) = ctx.renderer.measure(&progress_bar, font_size);
        let bar_scale = if bar_w > bar_available {
            LEVEL_SCALE * (bar_available / bar_w)
        } else {
            LEVEL_SCALE
        };
        let (scaled_bar_w, bar_h) = ctx.renderer.measure(&progress_bar, bar_scale * 16.0);

        let total_w = current_w + spacing + scaled_bar_w + spacing + next_w;
        let start_x = LEVEL_PADDING as f32 + (available_width as f32 - total_w) / 2.0;
        let star_y = section_height - star_h - bottom_padding;
        let star_center_y = star_y + star_h / 2.0;
        let bar_y = (star_center_y - bar_h / 2.0) as i32;
        let star_y = star_y as i32;

        let mut current_ctx = ctx.at(start_x as i32, star_y);
        TextBlock::new().push(current_star).scale(LEVEL_SCALE).draw(&mut current_ctx);

        let bar_x = start_x + current_w + spacing;
        let mut bar_ctx = ctx.at(bar_x as i32, bar_y);
        TextBlock::new().push(progress_bar).scale(bar_scale).draw(&mut bar_ctx);

        let next_x = bar_x + scaled_bar_w + spacing;
        let mut next_ctx = ctx.at(next_x as i32, star_y);
        TextBlock::new().push(next_star).scale(LEVEL_SCALE).draw(&mut next_ctx);
    }

    fn size(&self) -> (u32, u32) {
        (CANVAS_WIDTH, 53)
    }
}


struct SkinSection<'a> {
    skin: Option<&'a DynamicImage>,
    mode_label: String,
}


impl<'a> SkinSection<'a> {
    fn new(skin: Option<&'a DynamicImage>, mode_label: &str) -> Self {
        Self { skin, mode_label: mode_label.to_string() }
    }
}


impl Shape for SkinSection<'_> {
    fn draw(&self, ctx: &mut DrawContext) {
        RoundedRect::new(COL_WIDTH, SKIN_BOX_HEIGHT)
            .corner_radius(20)
            .padding(13, 13)
            .background(BOX_BACKGROUND)
            .draw(ctx);

        if let Some(skin) = &self.skin {
            let max_w = COL_WIDTH - 26;
            let max_h = SKIN_BOX_HEIGHT - 53;
            let scale = f64::min(max_w as f64 / skin.width() as f64, max_h as f64 / skin.height() as f64);
            let new_w = (skin.width() as f64 * scale) as u32;
            let new_h = (skin.height() as f64 * scale) as u32;
            let skin_x = 13 + (max_w - new_w) / 2;
            let skin_y = 13 + (max_h - new_h) / 2;

            let mut skin_ctx = ctx.at(skin_x as i32, skin_y as i32);
            Image::new((*skin).clone()).size(new_w, new_h).draw(&mut skin_ctx);
        }

        let mode_text = MCText::new()
            .span("(").color(NamedColor::Gray)
            .then(&self.mode_label).then(")")
            .build();

        let mut mode_ctx = ctx.at(0, (SKIN_BOX_HEIGHT - 27) as i32);
        TextBlock::new()
            .push(mode_text)
            .scale(1.5)
            .align_x(Align::Center)
            .max_width(COL_WIDTH)
            .draw(&mut mode_ctx);
    }

    fn size(&self) -> (u32, u32) {
        (COL_WIDTH, SKIN_BOX_HEIGHT)
    }
}
