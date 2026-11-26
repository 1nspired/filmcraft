//! Poema product identity. The caesura is original code-drawn geometry; no upstream brand
//! artwork is embedded. See ATTRIBUTION.md. Protocol, crate and project-format names stay stable.

pub const NAME: &str = "Poema NLE";
pub const APP_ID: &str = "local.poema.nle";
pub const ABOUT: &str = "About Poema NLE";
const INK: egui::Color32 = egui::Color32::from_rgb(14, 14, 18);
const PAPER: egui::Color32 = egui::Color32::from_rgb(237, 232, 220);
const CUT: egui::Color32 = egui::Color32::from_rgb(212, 69, 47);

/// A fixed-size original caesura icon, shared by the native window and package asset generator.
/// All geometry and allocation bounds are constants, independent of media or user input.
pub fn icon() -> egui::IconData {
    const SIDE: u32 = 256;
    let mut rgba = Vec::with_capacity(256 * 256 * 4);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let corner_x = if x < 48 { 48 - x } else { x.saturating_sub(207) };
            let corner_y = if y < 48 { 48 - y } else { y.saturating_sub(207) };
            let outside = corner_x * corner_x + corner_y * corner_y > 48 * 48;
            let bar = (88..108).contains(&x) || (148..168).contains(&x);
            let color = if outside {
                egui::Color32::TRANSPARENT
            } else if bar && (60..196).contains(&y) {
                CUT
            } else {
                INK
            };
            rgba.extend_from_slice(&color.to_array());
        }
    }
    egui::IconData { rgba, width: SIDE, height: SIDE }
}

/// Draw the Poema wordmark, returning its actual painted bounds for the adjacent layout.
/// Refuse invalid geometry before passing it to the painter.
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    if !left_center.x.is_finite() || !left_center.y.is_finite() || !height.is_finite() || !(1.0..=256.0).contains(&height) {
        return None;
    }
    let painter = ui.painter();
    let top = left_center.y - height / 2.0;
    for offset in [0.0, 0.27] {
        let bar = egui::Rect::from_min_size(egui::pos2(left_center.x + height * offset, top), egui::vec2(height * 0.12, height));
        painter.rect_filled(bar, height * 0.025, CUT);
    }
    let text = painter.text(
        egui::pos2(left_center.x + height * 0.68, left_center.y),
        egui::Align2::LEFT_CENTER,
        "P O E M A",
        crate::theme::Tokens::semibold(height * 0.82),
        if dark { PAPER } else { INK },
    );
    Some(egui::Rect::from_min_max(egui::pos2(left_center.x, top.min(text.min.y)), egui::pos2(text.max.x, (top + height).max(text.max.y))))
}
