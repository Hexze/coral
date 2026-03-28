mod column;
mod image;
mod rect;
mod row;
mod spacer;
mod text;
mod text_box;

pub use column::Column;
pub use image::Image;
pub use rect::RoundedRect;
pub use row::Row;
pub use spacer::Spacer;
pub use text::{Text, TextBlock};
pub use text_box::{Align, TextBox};


pub(crate) fn is_outside_rounded_rect(px: u32, py: u32, w: u32, h: u32, r: u32) -> bool {
    let r = r.min(w / 2).min(h / 2);
    if r == 0 { return false; }
    let in_corner = |dx: u32, dy: u32| dx * dx + dy * dy > r * r;
    let wr = w - r;
    let hr = h - r;
    if px < r && py < r { return in_corner(r - px, r - py); }
    if px >= wr && py < r { return in_corner(px - wr + 1, r - py); }
    if px < r && py >= hr { return in_corner(r - px, py - hr + 1); }
    if px >= wr && py >= hr { return in_corner(px - wr + 1, py - hr + 1); }
    false
}
