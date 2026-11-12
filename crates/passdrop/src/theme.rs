//! Look and feel: system fonts, a calm palette for light and dark, and a few small
//! drawing helpers (cards, chips, callouts, a dashed drop zone).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Stroke, TextStyle,
    Theme, Visuals, vec2,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub card: Color32,
    pub input: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub warn: Color32,
    pub warn_bg: Color32,
    pub danger: Color32,
    pub danger_bg: Color32,
    pub chip: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(0x1c, 0x1c, 0x1e),
            card: Color32::from_rgb(0x26, 0x26, 0x29),
            input: Color32::from_rgb(0x16, 0x16, 0x18),
            border: Color32::from_rgb(0x38, 0x38, 0x3c),
            text: Color32::from_rgb(0xf2, 0xf2, 0xf5),
            muted: Color32::from_rgb(0x9a, 0x9a, 0xa2),
            accent: Color32::from_rgb(0x0a, 0x84, 0xff),
            warn: Color32::from_rgb(0xff, 0xb3, 0x40),
            warn_bg: Color32::from_rgb(0x3a, 0x2e, 0x1a),
            danger: Color32::from_rgb(0xff, 0x6b, 0x60),
            danger_bg: Color32::from_rgb(0x3d, 0x1f, 0x1d),
            chip: Color32::from_rgb(0x32, 0x32, 0x36),
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0xf3, 0xf3, 0xf6),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            input: Color32::from_rgb(0xf6, 0xf6, 0xf8),
            border: Color32::from_rgb(0xdc, 0xdc, 0xe2),
            text: Color32::from_rgb(0x1c, 0x1c, 0x1e),
            muted: Color32::from_rgb(0x6c, 0x6c, 0x74),
            accent: Color32::from_rgb(0x00, 0x6e, 0xe6),
            warn: Color32::from_rgb(0x9a, 0x5b, 0x00),
            warn_bg: Color32::from_rgb(0xfd, 0xf3, 0xe1),
            danger: Color32::from_rgb(0xc4, 0x1d, 0x12),
            danger_bg: Color32::from_rgb(0xfd, 0xe8, 0xe6),
            chip: Color32::from_rgb(0xec, 0xec, 0xf0),
        }
    }
}

pub fn current(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

/// Does this text need a wide-coverage font? SF Pro covers Latin, Greek and Cyrillic, and
/// Apple Symbols covers punctuation, arrows and symbols; anything else (CJK, Arabic,
/// Hebrew, Indic, Thai, …) needs the large fallback, which costs memory, so it is only
/// loaded when the spec actually contains such text.
pub fn needs_wide_font(text: &str) -> bool {
    text.chars().any(|c| {
        let u = c as u32;
        !(u <= 0x052F || (0x1D00..=0x1FFF).contains(&u) || (0x2000..=0x2BFF).contains(&u) || (0xFE00..=0xFE0F).contains(&u))
    })
}

pub fn install(ctx: &egui::Context, wide: bool) {
    install_fonts(ctx, wide);
    for theme in [Theme::Dark, Theme::Light] {
        let p = palette(theme == Theme::Dark);
        ctx.style_mut_of(theme, |s| {
            s.text_styles = [
                (TextStyle::Heading, FontId::proportional(20.0)),
                (TextStyle::Body, FontId::proportional(13.5)),
                (TextStyle::Button, FontId::proportional(13.5)),
                (TextStyle::Small, FontId::proportional(11.5)),
                (TextStyle::Monospace, FontId::monospace(11.0)),
            ]
            .into();
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(12.0, 5.0);
            s.spacing.interact_size.y = 26.0;
            s.visuals = visuals(theme == Theme::Dark, p);
        });
    }
}

fn visuals(dark: bool, p: Palette) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.faint_bg_color = p.card;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.weak_text_color = Some(p.muted);
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.5, p.accent);
    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = radius;
        s.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = p.chip;
    w.inactive.weak_bg_fill = p.chip;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_fill = p.chip;
    w.hovered.weak_bg_fill = p.chip;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.7));
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.active.bg_fill = p.chip;
    w.active.weak_bg_fill = p.chip;
    w.active.bg_stroke = Stroke::new(1.5, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    v
}

/// System fonts first (SF Pro / SF Mono on macOS), egui's bundled fonts after them,
/// then broad-coverage fallbacks: egui's own fonts lack symbols like → and most scripts,
/// and agent-written text can contain anything.
fn install_fonts(ctx: &egui::Context, wide: bool) {
    let mut fonts = FontDefinitions::default();
    // Each file is read once and registered once; families only hold names.
    let mut add = |name: &str, path: &str, families: &[FontFamily], first: bool| {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
        for family in families {
            let list = fonts.families.entry(family.clone()).or_default();
            if first {
                list.insert(0, name.to_string());
            } else {
                list.push(name.to_string());
            }
        }
    };
    add("sf", "/System/Library/Fonts/SFNS.ttf", &[FontFamily::Proportional], true);
    add("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", &[FontFamily::Monospace], true);
    let both = [FontFamily::Proportional, FontFamily::Monospace];
    add("symbols", "/System/Library/Fonts/Apple Symbols.ttf", &both, false);
    add("dejavu", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", &both, false);
    if wide {
        add("wide", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf", &both, false);
        add("noto", "/usr/share/fonts/noto/NotoSans-Regular.ttf", &both, false);
    }
    ctx.set_fonts(fonts);
}

pub fn card(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 14))
}

/// A small rounded label, e.g. "Secret" or "Visible to agent".
pub fn chip(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(RichText::new(text).small().color(fg));
    });
}

/// A tinted box for a notice, with optional content under the text.
pub fn callout(ui: &mut egui::Ui, fg: Color32, bg: Color32, text: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).color(fg));
        add(ui);
    });
}

pub fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: Stroke) {
    let (a, b, c, d) = (rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom());
    for (from, to) in [(a, b), (b, c), (c, d), (d, a)] {
        painter.extend(egui::Shape::dashed_line(&[from, to], stroke, 6.0, 4.0));
    }
}

#[cfg(test)]
mod tests {
    use super::needs_wide_font;

    #[test]
    fn wide_font_only_for_scripts_sf_pro_lacks() {
        for s in ["App Store → Team Keys", "Latin Extended letters: ğ ş ı İ ç ö ü", "Ключ API", "Κλειδί", "“quoted” — dash ⚠ ↗"] {
            assert!(!needs_wide_font(s), "{s}");
        }
        for s in ["APIキー", "مفتاح", "מפתח", "कुंजी", "密钥"] {
            assert!(needs_wide_font(s), "{s}");
        }
    }
}
