//! Design tokens from docs/design (Claude Design canvas "StinkShot UI"). Every surface
//! uses these, never ad-hoc colors. Dark is primary; light values mirror Windows 11.
//!
//! Font: Segoe UI Variable (from `Shared.fonts.regular_bytes`), registered with egui by
//! `install` along with sizes: body 13, caption 12, title 14 semibold, h1 28 semibold.
//! Radii: windows 8, controls 6, toolbars 10. Toolbar buttons 34x34, touch targets >= 32.

use egui::Color32;

pub struct Palette {
    pub bg: Color32,          // window background (Mica fallback)
    pub layer: Color32,       // content layer / main pane
    pub card: Color32,        // cards and settings rows
    pub flyout: Color32,      // menus, toasts, toolbars
    pub control: Color32,     // secondary button fill
    pub stroke: Color32,      // 1px hairline
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub overlay_dim: Color32, // area outside the selection
    pub danger: Color32,      // record dot, stop button
    pub on_accent: Color32,   // text on accent fills
    pub annotation_default: Color32,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x20, 0x20, 0x20),
    layer: Color32::from_rgb(0x27, 0x27, 0x27),
    card: Color32::from_rgb(0x2b, 0x2b, 0x2b),
    flyout: Color32::from_rgb(0x2c, 0x2c, 0x2c),
    control: Color32::from_rgb(0x37, 0x37, 0x37),
    stroke: Color32::from_rgba_premultiplied(0x14, 0x14, 0x14, 0x14),
    text: Color32::from_rgb(0xff, 0xff, 0xff),
    text_secondary: Color32::from_rgb(0xc8, 0xc8, 0xc8),
    text_tertiary: Color32::from_rgb(0xa8, 0xa8, 0xa8),
    overlay_dim: Color32::from_rgba_premultiplied(0, 0, 0, 148),
    danger: Color32::from_rgb(0xff, 0x5a, 0x52),
    on_accent: Color32::from_rgb(0x0d, 0x1f, 0x08),
    annotation_default: Color32::from_rgb(0xff, 0x8a, 0x3d),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf3, 0xf3, 0xf3),
    layer: Color32::from_rgb(0xf9, 0xf9, 0xf9),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    flyout: Color32::from_rgb(0xf9, 0xf9, 0xf9),
    control: Color32::from_rgb(0xfb, 0xfb, 0xfb),
    stroke: Color32::from_rgba_premultiplied(0x0f, 0x0f, 0x0f, 0x0f),
    text: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    text_secondary: Color32::from_rgb(0x5c, 0x5c, 0x5c),
    text_tertiary: Color32::from_rgb(0x6e, 0x6e, 0x6e),
    overlay_dim: Color32::from_rgba_premultiplied(0, 0, 0, 148),
    danger: Color32::from_rgb(0xc4, 0x2b, 0x1c),
    on_accent: Color32::from_rgb(0x0d, 0x1f, 0x08),
    annotation_default: Color32::from_rgb(0xff, 0x8a, 0x3d),
};

pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(0x7f, 0xd3, 0x6a);

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

pub fn rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Register fonts and set egui Style/Visuals from the palette + accent for `ctx`.
/// Called by the shell once per context and whenever theme/accent changes.
pub fn install(ctx: &egui::Context, fonts: &stinkshot_core::fonts::Fonts, dark: bool, accent: Color32) {
    let _ = (ctx, fonts, dark, accent);
    todo!("app::theme::install")
}

/// Shared widgets so every surface looks identical. Implemented by the shell agent;
/// signatures are fixed.
pub mod widgets {
    use egui::{Response, Ui};

    /// Filled accent button (primary action).
    pub fn accent_button(ui: &mut Ui, text: &str) -> Response {
        let _ = (ui, text);
        todo!("theme::widgets::accent_button")
    }
    /// Neutral control-filled button.
    pub fn button(ui: &mut Ui, text: &str) -> Response {
        let _ = (ui, text);
        todo!("theme::widgets::button")
    }
    /// 34x34 icon button for toolbars; `selected` paints the accent-soft background.
    pub fn icon_button(ui: &mut Ui, icon: super::Icon, tooltip: &str, selected: bool) -> Response {
        let _ = (ui, icon, tooltip, selected);
        todo!("theme::widgets::icon_button")
    }
    /// Windows 11 style toggle switch with label text to its left.
    pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
        let _ = (ui, on);
        todo!("theme::widgets::toggle")
    }
    /// Settings-style card row: title + description on the left, `add_right` on the right.
    pub fn setting_row(ui: &mut Ui, title: &str, desc: &str, add_right: impl FnOnce(&mut Ui)) {
        let _ = (ui, title, desc, add_right);
        todo!("theme::widgets::setting_row")
    }
    /// Paint an icon centered in `rect` with stroke color.
    pub fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: super::Icon, color: egui::Color32) {
        let _ = (painter, rect, icon, color);
        todo!("theme::widgets::paint_icon")
    }
}

/// Stroke icons drawn with the egui painter (no icon font, no emoji). Shapes follow the
/// inline SVGs in docs/design/Main.dc.html and Recording.dc.html.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Blur,
    Pixelate,
    Redact,
    Magnifier,
    Callout,
    Spotlight,
    Ocr,
    Pin,
    Save,
    Close,
    Copy,
    Undo,
    Redo,
    Pause,
    Play,
    Stop,
    Mic,
    Speaker,
    Webcam,
    Clicks,
    Search,
    Star,
    StarFilled,
    Folder,
    Trash,
    Settings,
    History,
    Shield,
}
