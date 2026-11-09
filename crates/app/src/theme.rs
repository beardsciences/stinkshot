//! Design tokens from docs/design (Stinkshot UI mockups). Every surface
//! uses these, never ad-hoc colors. Dark is primary; light values mirror Windows 11.
//!
//! Font: Segoe UI Variable (from `Shared.fonts.regular_bytes`), registered with egui by
//! `install` along with sizes: body 13, caption 12, title 14 semibold, h1 28 semibold.
//! Radii: windows 8, controls 6, toolbars 10. Toolbar buttons 34x34, touch targets >= 32.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Stroke, TextStyle,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: Color32,      // window background (Mica fallback)
    pub layer: Color32,   // content layer / main pane
    pub card: Color32,    // cards and settings rows
    pub flyout: Color32,  // menus, toasts, toolbars
    pub control: Color32, // secondary button fill
    pub stroke: Color32,  // 1px hairline
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub overlay_dim: Color32, // area outside the selection
    pub danger: Color32,      // record dot, stop button
    pub on_accent: Color32,   // text on accent fills
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
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf3, 0xf3, 0xf3),
    layer: Color32::from_rgb(0xf9, 0xf9, 0xf9),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    flyout: Color32::from_rgb(0xf9, 0xf9, 0xf9),
    control: Color32::from_rgb(0xfb, 0xfb, 0xfb),
    // Black hairline (Windows 11 card stroke, ~9%). Premultiplied black is all-zero RGB.
    stroke: Color32::from_rgba_premultiplied(0, 0, 0, 0x17),
    text: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    text_secondary: Color32::from_rgb(0x5c, 0x5c, 0x5c),
    text_tertiary: Color32::from_rgb(0x6e, 0x6e, 0x6e),
    overlay_dim: Color32::from_rgba_premultiplied(0, 0, 0, 148),
    danger: Color32::from_rgb(0xc4, 0x2b, 0x1c),
    on_accent: Color32::from_rgb(0x0d, 0x1f, 0x08),
};

/// Text on a `danger` fill: dark ink on the light red of the dark theme, white on the deep
/// red of the light theme (both >= 4.5:1).
pub fn on_danger(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(0x1f, 0x06, 0x05)
    } else {
        Color32::WHITE
    }
}

/// 1px border of a neutral control (buttons, combo boxes, text fields). A notch stronger
/// than the card hairline so controls read as controls in both themes.
pub fn control_stroke(dark: bool) -> Color32 {
    if dark {
        Color32::from_white_alpha(22)
    } else {
        Color32::from_black_alpha(30)
    }
}

/// Subtle wash for hovered (`level` 1), pressed (2) or selected (3) transparent items such as
/// icon buttons and nav rows; white on dark, black on light.
pub fn subtle_fill(dark: bool, level: u8) -> Color32 {
    let a = match level {
        0 => 0,
        1 => 15,
        2 => 26,
        _ => 20,
    };
    if dark {
        Color32::from_white_alpha(a)
    } else {
        Color32::from_black_alpha(a.saturating_sub(4))
    }
}

/// WCAG relative luminance of an opaque sRGB colour.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio between two opaque colours (1..=21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The accent adjusted for use as text or a thin glyph on the theme's surfaces: unchanged
/// when it already reaches 4.5:1, otherwise darkened (light theme) or lightened (dark theme)
/// just enough. Keeps light green / yellow accents legible on the light theme.
pub fn accent_fg(accent: Color32, dark: bool) -> Color32 {
    let p = palette(dark);
    // Check against the lightest (light theme) / lightest-dark (dark theme) surface it sits on.
    let bg = if dark { p.control } else { p.bg };
    let target = if dark { Color32::WHITE } else { Color32::BLACK };
    let mut c = accent;
    let mut t = 0.0;
    while contrast(c, bg) < 4.5 && t < 1.0 {
        t += 0.05;
        c = accent.lerp_to_gamma(target, t);
    }
    c
}

pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(0x7f, 0xd3, 0x6a);

pub fn palette(dark: bool) -> &'static Palette {
    if let Some(p) = *HC_PALETTE.lock().unwrap_or_else(|e| e.into_inner()) {
        return p;
    }
    if dark { &DARK } else { &LIGHT }
}

/// The high-contrast palette while a Windows contrast theme is on (set by `install`).
static HC_PALETTE: std::sync::Mutex<Option<&'static Palette>> = std::sync::Mutex::new(None);

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Every token from the Windows contrast theme's system colors: one background, one text
/// color for all text levels, opaque text-colored hairlines.
pub fn high_contrast_palette(hc: &crate::a11y::HighContrast) -> Palette {
    let dark = luminance(rgb(hc.window)) < 0.5;
    Palette {
        bg: rgb(hc.window),
        layer: rgb(hc.window),
        card: rgb(hc.window),
        flyout: rgb(hc.window),
        control: rgb(hc.button),
        stroke: rgb(hc.text),
        text: rgb(hc.text),
        text_secondary: rgb(hc.text),
        text_tertiary: rgb(hc.text),
        overlay_dim: DARK.overlay_dim,
        danger: if dark { DARK.danger } else { LIGHT.danger },
        on_accent: rgb(hc.highlight_text),
    }
}

pub fn rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Name of the semibold font family registered by `install`.
pub const SEMIBOLD: &str = "StinkShot.Semibold";

/// Semibold font at `size` points (titles, h1, step numbers).
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// The 14 pt semibold "title" text style (`RichText::text_style`).
pub fn title_style() -> TextStyle {
    TextStyle::Name("Title".into())
}

/// Accent colour with the given alpha (selected backgrounds, focus rings).
pub fn accent_soft(accent: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha)
}

#[derive(Clone, Copy)]
struct ThemeState {
    dark: bool,
    accent: Color32,
    /// Windows Text size (1.0 = 100%).
    text_scale: f32,
}

fn theme_id() -> Id {
    Id::unique("stinkshot.theme.state")
}

/// The dark flag and accent last passed to `install` for this context.
pub fn current(ctx: &egui::Context) -> (bool, Color32) {
    ctx.data(|d| d.get_temp::<ThemeState>(theme_id()))
        .map(|s| (s.dark, s.accent))
        .unwrap_or((true, DEFAULT_ACCENT))
}

/// Windows Text size factor last applied by `install` (1.0 = 100%). Text sizes in `widgets`
/// are multiplied by it; `TextStyle` sizes already are.
pub fn text_scale(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<ThemeState>(theme_id()))
        .map_or(1.0, |s| s.text_scale)
}

fn hover_tint(base: Color32, dark: bool, amount: f32) -> Color32 {
    let target = if dark { Color32::WHITE } else { Color32::BLACK };
    base.lerp_to_gamma(target, amount)
}

fn widget_visuals(
    fill: Color32,
    stroke: Color32,
    fg: Color32,
    radius: u8,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(radius),
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    }
}

/// Register fonts and set egui Style/Visuals from the palette + accent for `ctx`.
/// Called by the shell once per context and whenever theme/accent changes.
pub fn install(
    ctx: &egui::Context,
    fonts: &stinkshot_core::fonts::Fonts,
    dark: bool,
    accent: Color32,
) {
    // Windows accessibility settings win over the configured theme: a contrast theme
    // replaces every token with its system colors, and Text size scales all text styles.
    let prefs = crate::a11y::prefs();
    let hc = prefs.high_contrast;
    {
        let mut cur = HC_PALETTE.lock().unwrap_or_else(|e| e.into_inner());
        let want = hc.map(|h| high_contrast_palette(&h));
        if want.as_ref() != *cur {
            // `palette` hands out `&'static`: one small leak per contrast theme change.
            *cur = want.map(|w| &*Box::leak(Box::new(w)));
        }
    }
    let (dark, accent) = match &hc {
        Some(h) => (luminance(rgb(h.window)) < 0.5, rgb(h.highlight)),
        None => (dark, accent),
    };
    let ts = prefs.text_scale;
    let fonts_id = Id::unique("stinkshot.theme.fonts");
    let have_fonts = ctx.data(|d| d.get_temp::<bool>(fonts_id)).unwrap_or(false);
    if !have_fonts {
        let mut defs = FontDefinitions::default();
        defs.font_data.insert(
            "SegoeUIVariable".into(),
            std::sync::Arc::new(FontData::from_static(fonts.regular_bytes)),
        );
        // The design's headings are weight 600: prefer the real Segoe UI Semibold face and
        // fall back to the bold face core loads for annotation text.
        let semibold_bytes = std::env::var_os("WINDIR")
            .and_then(|w| {
                stinkshot_core::fonts::map_font_file(
                    &std::path::PathBuf::from(w)
                        .join("Fonts")
                        .join("seguisb.ttf"),
                )
            })
            .unwrap_or(fonts.bold_bytes);
        defs.font_data.insert(
            "SegoeUISemibold".into(),
            std::sync::Arc::new(FontData::from_static(semibold_bytes)),
        );
        let proportional = defs.families.entry(FontFamily::Proportional).or_default();
        proportional.insert(0, "SegoeUIVariable".into());
        let mut semi = vec!["SegoeUISemibold".to_string()];
        if let Some(fallbacks) = defs.families.get(&FontFamily::Proportional) {
            semi.extend(
                fallbacks
                    .iter()
                    .filter(|n| *n != "SegoeUIVariable")
                    .cloned(),
            );
        }
        defs.families
            .insert(FontFamily::Name(SEMIBOLD.into()), semi);
        ctx.set_fonts(defs);
        // CJK, Hangul and emoji: the same system fallback faces core renders with, loaded
        // the first time egui meets a character the UI font lacks.
        let core_fonts = fonts.clone();
        ctx.add_font_provider(std::sync::Arc::new(
            move |req: &egui::FallbackRequest<'_>| -> Option<egui::FontInsert> {
                let c = req.cluster.chars().next()?;
                let fb = core_fonts.fallback_for_char(c)?;
                let data = FontData::from_blob(std::sync::Arc::new(fb.data), fb.index);
                Some(egui::FontInsert::new(fb.name, data, Vec::new()))
            },
        ));
        ctx.data_mut(|d| d.insert_temp(fonts_id, true));
    }

    let p = palette(dark);
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });

    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.window_fill = p.bg;
    visuals.panel_fill = p.layer;
    visuals.faint_bg_color = p.card;
    visuals.extreme_bg_color = if dark {
        Color32::from_rgb(0x1c, 0x1c, 0x1c)
    } else {
        Color32::WHITE
    };
    visuals.code_bg_color = p.card;
    visuals.window_stroke = Stroke::new(1.0, p.stroke);
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(100),
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    visuals.hyperlink_color = accent_fg(accent, dark);
    visuals.error_fg_color = p.danger;
    visuals.warn_fg_color = if dark {
        Color32::from_rgb(0xff, 0xb4, 0x54)
    } else {
        Color32::from_rgb(0x9d, 0x5d, 0x00)
    };
    visuals.selection.bg_fill = accent_soft(accent, 90);
    visuals.selection.stroke = Stroke::new(1.0, accent_fg(accent, dark));
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;
    visuals.text_cursor.stroke = Stroke::new(1.5, p.text);

    // Controls carry a visible 1px border in both themes (the card hairline alone vanishes
    // on the light theme's white cards).
    let cs = control_stroke(dark);
    visuals.widgets.noninteractive = widget_visuals(p.card, p.stroke, p.text_secondary, 6);
    visuals.widgets.inactive = widget_visuals(p.control, cs, p.text, 6);
    visuals.widgets.hovered = widget_visuals(hover_tint(p.control, dark, 0.07), cs, p.text, 6);
    visuals.widgets.active = widget_visuals(hover_tint(p.control, dark, 0.12), cs, p.text, 6);
    visuals.widgets.open = widget_visuals(hover_tint(p.control, dark, 0.09), cs, p.text, 6);
    if let Some(h) = &hc {
        // Contrast themes: solid borders in the text color, the system highlight for
        // selection and hover, the system link color, no soft shadows.
        let hl = rgb(h.highlight);
        visuals.widgets.noninteractive = widget_visuals(p.bg, p.text, p.text, 6);
        visuals.widgets.inactive = widget_visuals(p.control, p.text, rgb(h.button_text), 6);
        visuals.widgets.hovered = widget_visuals(p.control, hl, rgb(h.button_text), 6);
        visuals.widgets.active = widget_visuals(hl, hl, rgb(h.highlight_text), 6);
        visuals.widgets.open = widget_visuals(hl, hl, rgb(h.highlight_text), 6);
        for w in [&mut visuals.widgets.hovered, &mut visuals.widgets.active] {
            w.bg_stroke.width = 2.0;
        }
        visuals.selection.bg_fill = hl;
        visuals.selection.stroke = Stroke::new(1.0, rgb(h.highlight_text));
        visuals.hyperlink_color = rgb(h.link);
        visuals.weak_text_color = Some(rgb(h.disabled));
        visuals.window_shadow = egui::Shadow::NONE;
        visuals.popup_shadow = egui::Shadow::NONE;
        visuals.extreme_bg_color = p.bg;
        visuals.text_cursor.stroke = Stroke::new(2.0, p.text);
    }

    let mut style = egui::Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size = egui::vec2(32.0, 32.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.slider_rail_height = 4.0;
    // Hover and press fades: quick enough to never feel sluggish.
    style.animation_time = 0.1;
    style.text_styles = [
        (TextStyle::Heading, semibold(28.0 * ts)),
        (title_style(), semibold(14.0 * ts)),
        (
            TextStyle::Body,
            FontId::new(13.0 * ts, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(13.0 * ts, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0 * ts, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5 * ts, FontFamily::Monospace),
        ),
    ]
    .into();
    ctx.set_style_of(theme, style);
    ctx.data_mut(|d| {
        d.insert_temp(
            theme_id(),
            ThemeState {
                dark,
                accent,
                text_scale: ts,
            },
        )
    });
    ctx.request_repaint();
}

/// Shared widgets so every surface looks identical. Implemented by the shell agent;
/// signatures are fixed.
pub mod widgets {
    use egui::accesskit::Role;
    use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, vec2};

    use super::{accent_fg, accent_soft, control_stroke, current, hover_tint, palette, semibold};

    /// 0..1 hover amount that eases in and out over ~100 ms (0 while disabled).
    pub(crate) fn hover_fade(ui: &Ui, resp: &Response) -> f32 {
        ui.ctx().animate_bool_with_time(
            resp.id.with("stinkshot.hover"),
            resp.hovered() && ui.is_enabled(),
            0.1,
        )
    }

    /// Windows 11 keyboard focus ring: 2px, offset 1px outside the control's own corners.
    pub(crate) fn focus_ring(ui: &Ui, rect: Rect, radius: u8) {
        let (dark, _) = current(ui.ctx());
        let p = palette(dark);
        ui.painter().rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(radius.saturating_add(1)),
            Stroke::new(2.0, p.text),
            StrokeKind::Outside,
        );
    }

    struct ButtonLook {
        fill: Color32,
        fill_hover: Color32,
        fill_active: Color32,
        text: Color32,
        stroke: Stroke,
        bold: bool,
    }

    fn paint_button(ui: &mut Ui, text: &str, look: ButtonLook) -> Response {
        let size = 13.0 * super::text_scale(ui.ctx());
        let font = if look.bold {
            semibold(size)
        } else {
            egui::FontId::proportional(size)
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font, look.text);
        let size = vec2(galley.size().x + 24.0, 32.0f32.max(galley.size().y + 12.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(Role::Button, ui.is_enabled(), text));
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hover_t = hover_fade(ui, &resp);
            let fill = if !enabled {
                look.fill.gamma_multiply(0.5)
            } else if resp.is_pointer_button_down_on() {
                look.fill_active
            } else {
                look.fill.lerp_to_gamma(look.fill_hover, hover_t)
            };
            let radius = CornerRadius::same(6);
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter()
                .rect_stroke(rect, radius, look.stroke, StrokeKind::Inside);
            if resp.has_focus() {
                focus_ring(ui, rect, 6);
            }
            let pos = Pos2::new(
                rect.center().x - galley.size().x / 2.0,
                rect.center().y - galley.size().y / 2.0,
            );
            let color = if enabled {
                look.text
            } else {
                look.text.gamma_multiply(0.5)
            };
            ui.painter().galley(pos, galley, color);
        }
        if resp.hovered() && ui.is_enabled() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp
    }

    /// Filled accent button (primary action).
    pub fn accent_button(ui: &mut Ui, text: &str) -> Response {
        let (dark, accent) = current(ui.ctx());
        let p = palette(dark);
        paint_button(
            ui,
            text,
            ButtonLook {
                fill: accent,
                fill_hover: accent.lerp_to_gamma(Color32::WHITE, 0.14),
                fill_active: accent.lerp_to_gamma(Color32::BLACK, 0.12),
                text: p.on_accent,
                stroke: Stroke::new(1.0, accent.lerp_to_gamma(Color32::BLACK, 0.2)),
                bold: true,
            },
        )
    }

    /// Neutral control-filled button.
    pub fn button(ui: &mut Ui, text: &str) -> Response {
        let (dark, _) = current(ui.ctx());
        let p = palette(dark);
        paint_button(
            ui,
            text,
            ButtonLook {
                fill: p.control,
                fill_hover: hover_tint(p.control, dark, 0.07),
                fill_active: hover_tint(p.control, dark, 0.12),
                text: p.text,
                stroke: Stroke::new(1.0, control_stroke(dark)),
                bold: false,
            },
        )
    }

    /// 34x34 icon button for toolbars; `selected` paints the accent-soft background.
    pub fn icon_button(ui: &mut Ui, icon: super::Icon, tooltip: &str, selected: bool) -> Response {
        let (dark, accent) = current(ui.ctx());
        let p = palette(dark);
        let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
        // Named by its tooltip; `selected` reads as a pressed toggle.
        resp.widget_info(|| {
            egui::WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, tooltip)
        });
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hover_t = hover_fade(ui, &resp);
            let pressed = enabled && resp.is_pointer_button_down_on();
            let bg = if selected {
                accent_soft(accent, 56 + (24.0 * hover_t) as u8)
            } else if pressed {
                super::subtle_fill(dark, 2)
            } else {
                super::subtle_fill(dark, 1).gamma_multiply(hover_t)
            };
            ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
            if selected {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(6),
                    Stroke::new(1.0, accent_soft(accent, 140)),
                    StrokeKind::Inside,
                );
            }
            if resp.has_focus() {
                focus_ring(ui, rect, 6);
            }
            let color = if !enabled {
                // Disabled glyphs stay recognisable but clearly inert.
                p.text_tertiary.gamma_multiply(0.55)
            } else if selected {
                accent_fg(accent, dark)
            } else {
                p.text
            };
            // Pressed glyphs shrink a hair, like Fluent's pressed state.
            let side = if pressed { 17.0 } else { 18.0 };
            paint_icon(
                ui.painter(),
                Rect::from_center_size(rect.center(), vec2(side, side)),
                icon,
                color,
            );
        }
        if resp.hovered() && ui.is_enabled() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp.on_hover_text(tooltip)
    }

    /// Windows 11 style toggle switch with label text to its left.
    pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
        let (dark, accent) = current(ui.ctx());
        let p = palette(dark);
        let label = if *on {
            crate::l10n::tr!("shell.toggle.on")
        } else {
            crate::l10n::tr!("shell.toggle.off")
        };
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(13.0 * super::text_scale(ui.ctx())),
            p.text_secondary,
        );
        let switch = vec2(40.0, 20.0);
        let gap = 10.0;
        let size = vec2(galley.size().x.max(22.0) + gap + switch.x, 24.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        // No name of its own: `setting_row` names it after the row title.
        let state = *on;
        resp.widget_info(|| egui::WidgetInfo::selected(Role::Switch, ui.is_enabled(), state, ""));
        if ui.is_rect_visible(rect) {
            let t = ui.ctx().animate_bool_responsive(resp.id, *on);
            let sw = Rect::from_min_size(
                Pos2::new(rect.right() - switch.x, rect.center().y - switch.y / 2.0),
                switch,
            );
            let pill = CornerRadius::same(10);
            // Off: Windows 11 draws the outline and thumb in the secondary text colour, which
            // reads on both the dark and the white cards (the old fixed greys vanished on
            // white). A faint wash on hover.
            let off_color = p.text_secondary;
            let hover_t = hover_fade(ui, &resp);
            if t < 1.0 && hover_t > 0.0 {
                ui.painter().rect_filled(
                    sw,
                    pill,
                    super::subtle_fill(dark, 1).gamma_multiply(hover_t * (1.0 - t)),
                );
            }
            if t > 0.0 {
                let on = accent.lerp_to_gamma(
                    if dark { Color32::WHITE } else { Color32::BLACK },
                    0.08 * hover_t,
                );
                ui.painter().rect_filled(sw, pill, on.gamma_multiply(t));
            }
            if t < 1.0 {
                ui.painter().rect_stroke(
                    sw,
                    pill,
                    Stroke::new(1.0, off_color.gamma_multiply(1.0 - t)),
                    StrokeKind::Inside,
                );
            }
            let x0 = sw.left() + 10.0;
            let x1 = sw.right() - 10.0;
            let thumb_x = x0 + (x1 - x0) * t;
            let thumb_r = 6.0 + hover_t;
            let thumb_color = off_color.lerp_to_gamma(p.on_accent, t);
            ui.painter()
                .circle_filled(Pos2::new(thumb_x, sw.center().y), thumb_r, thumb_color);
            if resp.has_focus() {
                focus_ring(ui, sw.expand(1.0), 11);
            }
            ui.painter().galley(
                Pos2::new(
                    sw.left() - gap - galley.size().x,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                p.text_secondary,
            );
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp
    }

    /// Settings-style card row: title + description on the left, `add_right` on the right.
    pub fn setting_row(ui: &mut Ui, title: &str, desc: &str, add_right: impl FnOnce(&mut Ui)) {
        setting_row_alert(ui, title, desc, None, add_right);
    }

    /// `setting_row` with an optional problem line under the description, in the danger
    /// color, and a danger-tinted border while it shows.
    pub fn setting_row_alert(
        ui: &mut Ui,
        title: &str,
        desc: &str,
        alert: Option<&str>,
        add_right: impl FnOnce(&mut Ui),
    ) {
        let (dark, _) = current(ui.ctx());
        let p = palette(dark);
        let stroke = if alert.is_some() {
            Stroke::new(1.0, p.danger.gamma_multiply(0.6))
        } else {
            Stroke::new(1.0, p.stroke)
        };
        egui::Frame::new()
            .fill(p.card)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The text wraps beside the controls, using their width from the last frame,
                // so long (translated) text never runs under them. When that would leave the
                // text too narrow, the controls move to their own line below it.
                let id = ui.scope_id().with(("setting_row", title));
                let right_w = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
                let text_w = ui.available_width() - right_w - 16.0;
                let stacked = text_w < 200.0;
                let ts = super::text_scale(ui.ctx());
                let text = |ui: &mut Ui, max_w: f32| {
                    ui.vertical(|ui| {
                        ui.set_max_width(max_w);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        let t = ui.add(
                            egui::Label::new(
                                egui::RichText::new(title).size(14.0 * ts).color(p.text),
                            )
                            .wrap(),
                        );
                        if !desc.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(desc)
                                        .size(12.0 * ts)
                                        .color(p.text_tertiary),
                                )
                                .wrap(),
                            );
                        }
                        if let Some(alert) = alert {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(alert).size(12.0 * ts).color(p.danger),
                                )
                                .wrap(),
                            );
                        }
                        t.id
                    })
                    .inner
                };
                let right = egui::Layout::right_to_left(egui::Align::Center);
                // Screen readers name the row's unnamed controls after its title.
                let used = if stacked {
                    let title_id = text(ui, ui.available_width());
                    ui.add_space(6.0);
                    let h = ui.spacing().interact_size.y;
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), h), right, |ui| {
                        ui.label_inputs_by(title_id, add_right);
                        ui.min_rect().width()
                    })
                    .inner
                } else {
                    ui.horizontal(|ui| {
                        let title_id = text(ui, text_w);
                        ui.with_layout(right, |ui| {
                            ui.label_inputs_by(title_id, add_right);
                            ui.min_rect().width()
                        })
                        .inner
                    })
                    .inner
                };
                if (used - right_w).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(id, used));
                    ui.ctx().request_repaint();
                }
            });
    }

    /// ComboBox icon painter: a Fluent chevron instead of egui's filled triangle. Use as
    /// `ComboBox::icon(widgets::combo_chevron)`.
    pub fn combo_chevron(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
        let (dark, _) = current(ui.ctx());
        let side = rect.height().min(14.0);
        let color = if ui.is_enabled() {
            palette(dark).text_secondary
        } else {
            visuals.fg_stroke.color
        };
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(side, side)),
            super::Icon::ChevronDown,
            color,
        );
    }

    /// Paint an icon centered in `rect` with stroke color.
    pub fn paint_icon(
        painter: &egui::Painter,
        rect: egui::Rect,
        icon: super::Icon,
        color: egui::Color32,
    ) {
        super::icons::paint(painter, rect, icon, color);
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
    Pause,
    Play,
    Mic,
    Speaker,
    Webcam,
    Clicks,
    Search,
    Star,
    StarFilled,
    Folder,
    Trash,
    History,
    Shield,
    ChevronDown,
    Settings,
    Eyedropper,
    /// Two regions side by side (multi-region capture).
    Regions,
    /// Measure tool.
    Ruler,
    /// Scan Codes and Text panel.
    Qr,
    /// Beautify panel.
    Frame,
    /// Edit in Window: a window with an arrow out of it.
    OpenWindow,
}

/// Icon vector data (24x24 design grid, stroke 1.8) and the painter that draws it.
mod icons {
    use egui::epaint::{Mesh, Vertex};
    use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

    use super::Icon;

    enum Prim {
        /// Stroked SVG path.
        Path(&'static str),
        /// Filled SVG path (no stroke besides a hairline to smooth edges).
        Fill(&'static str),
        Circle(f32, f32, f32),
        Ellipse(f32, f32, f32, f32),
        Rect(f32, f32, f32, f32, f32),
        FillRect(f32, f32, f32, f32, f32),
        /// Rect with a translucent fill plus outline.
        SoftRect(f32, f32, f32, f32, f32),
        DashCircle(f32, f32, f32),
    }

    use Prim::*;

    fn prims(icon: Icon) -> &'static [Prim] {
        match icon {
            Icon::Select => &[Path("M5 3l14 8-6 2-3 6z")],
            Icon::Arrow => &[Path("M5 19L19 5M10 5h9v9")],
            Icon::Line => &[Path("M5 19L19 5")],
            Icon::Rect => &[Rect(4.0, 6.0, 16.0, 12.0, 1.5)],
            Icon::Ellipse => &[Ellipse(12.0, 12.0, 8.0, 6.0)],
            Icon::Pen => &[Path("M4 20l4-1 11-11-3-3L5 16z")],
            Icon::Highlighter => &[Path("M9 14l7-7 3 3-7 7H9z M4 20h8")],
            Icon::Text => &[Path("M5 6h14M12 6v13")],
            Icon::Step => &[Circle(12.0, 12.0, 8.0), Path("M11 9l2-1v8")],
            Icon::Blur => &[DashCircle(12.0, 12.0, 8.0)],
            Icon::Pixelate => &[
                FillRect(4.0, 4.0, 5.0, 5.0, 0.0),
                FillRect(15.0, 4.0, 5.0, 5.0, 0.0),
                FillRect(9.5, 9.5, 5.0, 5.0, 0.0),
                FillRect(4.0, 15.0, 5.0, 5.0, 0.0),
                FillRect(15.0, 15.0, 5.0, 5.0, 0.0),
            ],
            Icon::Redact => &[FillRect(4.0, 8.0, 16.0, 8.0, 1.0)],
            Icon::Magnifier => &[Circle(10.0, 10.0, 6.0), Path("M15 15l5 5M8 10h4M10 8v4")],
            Icon::Callout => &[Path("M4 5h16v11H10l-4 4v-4H4z")],
            Icon::Spotlight => &[SoftRect(3.0, 3.0, 18.0, 18.0, 2.0), Circle(12.0, 12.0, 5.0)],
            Icon::Ocr => &[Path("M4 8V5h3M17 5h3v3M20 16v3h-3M7 19H4v-3M8 9h8M12 9v7")],
            Icon::Pin => &[Path("M9 4h6l-1 6 3 3H7l3-3zM12 13v7")],
            Icon::Save => &[Path("M5 4h11l3 3v13H5z M8 4v5h7V4 M8 20v-6h8v6")],
            Icon::Close => &[Path("M6 6l12 12M18 6L6 18")],
            Icon::Copy => &[Rect(8.0, 8.0, 12.0, 12.0, 2.0), Path("M4 16V4h12")],
            Icon::Pause => &[
                FillRect(6.0, 5.0, 4.0, 14.0, 1.0),
                FillRect(14.0, 5.0, 4.0, 14.0, 1.0),
            ],
            Icon::Play => &[Fill("M7 4l13 8-13 8z")],
            Icon::Mic => &[
                Rect(9.0, 3.0, 6.0, 11.0, 3.0),
                Path("M5 11a7 7 0 0014 0M12 18v3"),
            ],
            Icon::Speaker => &[Path("M4 9h4l5-4v14l-5-4H4z M17 9a4 4 0 010 6")],
            Icon::Webcam => &[Rect(3.0, 7.0, 13.0, 10.0, 2.0), Path("M16 11l5-3v8l-5-3z")],
            Icon::Clicks => &[Circle(12.0, 12.0, 3.0), Circle(12.0, 12.0, 8.0)],
            Icon::Search => &[Circle(11.0, 11.0, 6.0), Path("M16 16l4 4")],
            Icon::Star => &[Path(
                "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z",
            )],
            Icon::StarFilled => &[Fill(
                "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z",
            )],
            Icon::Folder => &[Path(
                "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z",
            )],
            Icon::Trash => &[Path("M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13M10 11v6M14 11v6")],
            Icon::History => &[Circle(12.0, 12.0, 8.0), Path("M12 7v5l3 2")],
            Icon::Shield => &[
                Path("M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z"),
                Path("M8.5 12l2.5 2.5 4.5-5"),
            ],
            Icon::ChevronDown => &[Path("M6 9l6 6 6-6")],
            Icon::Regions => &[
                Rect(3.0, 4.0, 10.0, 8.0, 1.5),
                Rect(11.0, 12.0, 10.0, 8.0, 1.5),
            ],
            Icon::Eyedropper => &[
                Path("M13 8l3 3-8 8H5v-3z"),
                Path("M12 7l5 5M15 4.5a2.1 2.1 0 013 0l1.5 1.5a2.1 2.1 0 010 3L17 11.5 12.5 7z"),
            ],
            Icon::Settings => &[
                Path(
                    "M18.9 13.0L21.2 14.4L20.2 16.8L17.6 16.2L16.2 17.6L16.8 20.2L14.4 21.2L13.0 18.9\
                     L11.0 18.9L9.6 21.2L7.2 20.2L7.8 17.6L6.4 16.2L3.8 16.8L2.8 14.4L5.1 13.0\
                     L5.1 11.0L2.8 9.6L3.8 7.2L6.4 7.8L7.8 6.4L7.2 3.8L9.6 2.8L11.0 5.1L13.0 5.1\
                     L14.4 2.8L16.8 3.8L16.2 6.4L17.6 7.8L20.2 7.2L21.2 9.6L18.9 11.0z",
                ),
                Circle(12.0, 12.0, 3.0),
            ],
            Icon::Ruler => &[Path(
                "M3 16L16 3L21 8L8 21z M7 12L9 14 M10 9L12 11 M13 6L15 8",
            )],
            Icon::Qr => &[
                Rect(4.0, 4.0, 6.0, 6.0, 1.0),
                Rect(14.0, 4.0, 6.0, 6.0, 1.0),
                Rect(4.0, 14.0, 6.0, 6.0, 1.0),
                FillRect(14.0, 14.0, 2.5, 2.5, 0.0),
                FillRect(17.5, 17.5, 2.5, 2.5, 0.0),
                FillRect(17.5, 14.0, 2.5, 2.5, 0.0),
                FillRect(14.0, 17.5, 2.5, 2.5, 0.0),
            ],
            Icon::Frame => &[
                SoftRect(3.0, 3.0, 18.0, 18.0, 3.0),
                Rect(7.5, 7.5, 9.0, 9.0, 1.5),
            ],
            Icon::OpenWindow => &[
                Path(
                    "M11 4H5.5A2.5 2.5 0 0 0 3 6.5V18.5A2.5 2.5 0 0 0 5.5 21H17.5A2.5 2.5 0 0 0 20 18.5V13",
                ),
                Path("M3 9h8"),
                Path("M14 3h7v7M21 3l-8 8"),
            ],
        }
    }

    pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
        let side = rect.width().min(rect.height());
        let scale = side / 24.0;
        let origin = rect.center() - vec2(side / 2.0, side / 2.0);
        let t = move |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);
        let sw = (1.8 * scale).max(1.2);
        let stroke = Stroke::new(sw, color);
        for prim in prims(icon) {
            match *prim {
                Path(d) => {
                    for sp in parse_path(d) {
                        let pts: Vec<Pos2> = sp.points.iter().map(|p| t(p.0, p.1)).collect();
                        if pts.len() < 2 {
                            continue;
                        }
                        if sp.closed {
                            painter.add(Shape::closed_line(pts, stroke));
                        } else {
                            let (a, b) = (pts[0], pts[pts.len() - 1]);
                            painter.add(Shape::line(pts, stroke));
                            // Round caps.
                            painter.circle_filled(a, sw / 2.0, color);
                            painter.circle_filled(b, sw / 2.0, color);
                        }
                    }
                }
                Fill(d) => {
                    for sp in parse_path(d) {
                        let pts: Vec<Pos2> = sp.points.iter().map(|p| t(p.0, p.1)).collect();
                        fill_fan(painter, &pts, color);
                        if pts.len() > 2 {
                            painter.add(Shape::closed_line(
                                pts,
                                Stroke::new((sw * 0.6).max(1.0), color),
                            ));
                        }
                    }
                }
                Circle(cx, cy, r) => {
                    painter.circle_stroke(t(cx, cy), r * scale, stroke);
                }
                Ellipse(cx, cy, rx, ry) => {
                    let pts: Vec<Pos2> = (0..48)
                        .map(|i| {
                            let a = i as f32 / 48.0 * std::f32::consts::TAU;
                            t(cx + rx * a.cos(), cy + ry * a.sin())
                        })
                        .collect();
                    painter.add(Shape::closed_line(pts, stroke));
                }
                Rect(x, y, w, h, r) => {
                    let rr = Rect::from_min_max(t(x, y), t(x + w, y + h));
                    painter.rect_stroke(
                        rr,
                        CornerRadius::same((r * scale).round().clamp(0.0, 255.0) as u8),
                        stroke,
                        StrokeKind::Middle,
                    );
                }
                FillRect(x, y, w, h, r) => {
                    let rr = Rect::from_min_max(t(x, y), t(x + w, y + h));
                    painter.rect_filled(
                        rr,
                        CornerRadius::same((r * scale).round().clamp(0.0, 255.0) as u8),
                        color,
                    );
                }
                SoftRect(x, y, w, h, r) => {
                    let rr = Rect::from_min_max(t(x, y), t(x + w, y + h));
                    let radius = CornerRadius::same((r * scale).round().clamp(0.0, 255.0) as u8);
                    painter.rect_filled(rr, radius, color.gamma_multiply(0.25));
                    painter.rect_stroke(rr, radius, stroke, StrokeKind::Middle);
                }
                DashCircle(cx, cy, r) => {
                    let pts: Vec<Pos2> = (0..=64)
                        .map(|i| {
                            let a = i as f32 / 64.0 * std::f32::consts::TAU;
                            t(cx + r * a.cos(), cy + r * a.sin())
                        })
                        .collect();
                    painter.extend(Shape::dashed_line(&pts, stroke, 2.0 * scale, 3.0 * scale));
                }
            }
        }
    }

    /// Triangle fan from the centroid: correct for the star-shaped outlines used here.
    fn fill_fan(painter: &Painter, pts: &[Pos2], color: Color32) {
        if pts.len() < 3 {
            return;
        }
        let n = pts.len() as f32;
        let c = pos2(
            pts.iter().map(|p| p.x).sum::<f32>() / n,
            pts.iter().map(|p| p.y).sum::<f32>() / n,
        );
        let mut mesh = Mesh::default();
        let uv = egui::epaint::WHITE_UV;
        mesh.vertices.push(Vertex { pos: c, uv, color });
        for p in pts {
            mesh.vertices.push(Vertex { pos: *p, uv, color });
        }
        for i in 0..pts.len() {
            let a = 1 + i as u32;
            let b = 1 + ((i + 1) % pts.len()) as u32;
            mesh.indices.extend_from_slice(&[0, a, b]);
        }
        painter.add(Shape::mesh(mesh));
    }

    // ------------------------------------------------------------------ SVG path parsing

    pub struct SubPath {
        pub points: Vec<(f32, f32)>,
        pub closed: bool,
    }

    struct Lexer<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl Lexer<'_> {
        fn skip_sep(&mut self) {
            while self.i < self.s.len()
                && (self.s[self.i] == b' '
                    || self.s[self.i] == b','
                    || self.s[self.i] == b'\n'
                    || self.s[self.i] == b'\t')
            {
                self.i += 1;
            }
        }
        fn peek_is_number(&mut self) -> bool {
            self.skip_sep();
            self.i < self.s.len()
                && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'-' | b'+' | b'.'))
        }
        fn number(&mut self) -> Option<f32> {
            self.skip_sep();
            let start = self.i;
            if self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+') {
                self.i += 1;
            }
            let mut seen_dot = false;
            while self.i < self.s.len() {
                let c = self.s[self.i];
                if c.is_ascii_digit() {
                    self.i += 1;
                } else if c == b'.' && !seen_dot {
                    seen_dot = true;
                    self.i += 1;
                } else {
                    break;
                }
            }
            std::str::from_utf8(&self.s[start..self.i])
                .ok()?
                .parse()
                .ok()
        }
        fn flag(&mut self) -> Option<bool> {
            self.skip_sep();
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'0' => Some(false),
                b'1' => Some(true),
                _ => None,
            }
        }
    }

    pub fn parse_path(d: &str) -> Vec<SubPath> {
        let mut lx = Lexer {
            s: d.as_bytes(),
            i: 0,
        };
        let mut out: Vec<SubPath> = Vec::new();
        let mut cur = (0.0f32, 0.0f32);
        let mut start = (0.0f32, 0.0f32);
        let mut cmd = b'M';
        loop {
            lx.skip_sep();
            if lx.i >= lx.s.len() {
                break;
            }
            let c = lx.s[lx.i];
            if c.is_ascii_alphabetic() {
                cmd = c;
                lx.i += 1;
                if cmd == b'Z' || cmd == b'z' {
                    if let Some(sp) = out.last_mut() {
                        sp.closed = true;
                    }
                    cur = start;
                    continue;
                }
            } else if !lx.peek_is_number() {
                break;
            }
            let rel = cmd.is_ascii_lowercase();
            let (ox, oy) = if rel { cur } else { (0.0, 0.0) };
            let ok = (|| -> Option<()> {
                match cmd.to_ascii_uppercase() {
                    b'M' => {
                        let (x, y) = (lx.number()? + ox, lx.number()? + oy);
                        cur = (x, y);
                        start = cur;
                        out.push(SubPath {
                            points: vec![cur],
                            closed: false,
                        });
                        // Subsequent pairs are implicit line-tos.
                        cmd = if rel { b'l' } else { b'L' };
                    }
                    b'L' => {
                        let (x, y) = (lx.number()? + ox, lx.number()? + oy);
                        cur = (x, y);
                        push(&mut out, cur, start);
                    }
                    b'H' => {
                        let x = lx.number()? + ox;
                        cur.0 = x;
                        push(&mut out, cur, start);
                    }
                    b'V' => {
                        let y = lx.number()? + oy;
                        cur.1 = y;
                        push(&mut out, cur, start);
                    }
                    b'C' => {
                        let p1 = (lx.number()? + ox, lx.number()? + oy);
                        let p2 = (lx.number()? + ox, lx.number()? + oy);
                        let p3 = (lx.number()? + ox, lx.number()? + oy);
                        for i in 1..=14 {
                            let t = i as f32 / 14.0;
                            let u = 1.0 - t;
                            let x = u * u * u * cur.0
                                + 3.0 * u * u * t * p1.0
                                + 3.0 * u * t * t * p2.0
                                + t * t * t * p3.0;
                            let y = u * u * u * cur.1
                                + 3.0 * u * u * t * p1.1
                                + 3.0 * u * t * t * p2.1
                                + t * t * t * p3.1;
                            push(&mut out, (x, y), start);
                        }
                        cur = p3;
                    }
                    b'A' => {
                        let rx = lx.number()?;
                        let ry = lx.number()?;
                        let rot = lx.number()?;
                        let large = lx.flag()?;
                        let sweep = lx.flag()?;
                        let end = (lx.number()? + ox, lx.number()? + oy);
                        for p in arc_points(cur, rx, ry, rot, large, sweep, end) {
                            push(&mut out, p, start);
                        }
                        cur = end;
                    }
                    _ => return None,
                }
                Some(())
            })();
            if ok.is_none() {
                break;
            }
        }
        out
    }

    fn push(out: &mut Vec<SubPath>, p: (f32, f32), start: (f32, f32)) {
        if out.is_empty() {
            out.push(SubPath {
                points: vec![start],
                closed: false,
            });
        }
        if let Some(sp) = out.last_mut() {
            sp.points.push(p);
        }
    }

    /// SVG endpoint arc -> polyline (spec F.6.5).
    fn arc_points(
        from: (f32, f32),
        rx: f32,
        ry: f32,
        rot_deg: f32,
        large: bool,
        sweep: bool,
        to: (f32, f32),
    ) -> Vec<(f32, f32)> {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 || from == to {
            return vec![to];
        }
        let phi = rot_deg.to_radians();
        let (cos_p, sin_p) = (phi.cos(), phi.sin());
        let dx = (from.0 - to.0) / 2.0;
        let dy = (from.1 - to.1) / 2.0;
        let x1p = cos_p * dx + sin_p * dy;
        let y1p = -sin_p * dx + cos_p * dy;
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            rx *= s;
            ry *= s;
        }
        let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let mut coef = if den == 0.0 {
            0.0
        } else {
            (num / den).max(0.0).sqrt()
        };
        if large == sweep {
            coef = -coef;
        }
        let cxp = coef * (rx * y1p / ry);
        let cyp = coef * (-(ry * x1p) / rx);
        let cx = cos_p * cxp - sin_p * cyp + (from.0 + to.0) / 2.0;
        let cy = sin_p * cxp + cos_p * cyp + (from.1 + to.1) / 2.0;
        let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
            let dot = ux * vx + uy * vy;
            let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
            let mut a = (dot / len).clamp(-1.0, 1.0).acos();
            if ux * vy - uy * vx < 0.0 {
                a = -a;
            }
            a
        };
        let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
        let mut dtheta = angle(
            (x1p - cxp) / rx,
            (y1p - cyp) / ry,
            (-x1p - cxp) / rx,
            (-y1p - cyp) / ry,
        );
        if !sweep && dtheta > 0.0 {
            dtheta -= std::f32::consts::TAU;
        } else if sweep && dtheta < 0.0 {
            dtheta += std::f32::consts::TAU;
        }
        let steps = ((dtheta.abs() / (std::f32::consts::PI / 12.0)).ceil() as usize).max(2);
        let mut pts = Vec::with_capacity(steps);
        for i in 1..=steps {
            let a = theta1 + dtheta * i as f32 / steps as f32;
            let (ca, sa) = (a.cos(), a.sin());
            pts.push((
                cos_p * rx * ca - sin_p * ry * sa + cx,
                sin_p * rx * ca + cos_p * ry * sa + cy,
            ));
        }
        if let Some(last) = pts.last_mut() {
            *last = to;
        }
        pts
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_relative_lines_and_close() {
            let sp = parse_path("M5 3l14 8-6 2-3 6z");
            assert_eq!(sp.len(), 1);
            assert!(sp[0].closed);
            assert_eq!(
                sp[0].points,
                vec![(5.0, 3.0), (19.0, 11.0), (13.0, 13.0), (10.0, 19.0)]
            );
        }

        #[test]
        fn parses_multiple_subpaths_and_h_v() {
            let sp = parse_path("M5 6h14M12 6v13");
            assert_eq!(sp.len(), 2);
            assert_eq!(sp[0].points, vec![(5.0, 6.0), (19.0, 6.0)]);
            assert_eq!(sp[1].points, vec![(12.0, 6.0), (12.0, 19.0)]);
        }

        #[test]
        fn arc_with_packed_flags_ends_at_target() {
            // Mic cradle: "a7 7 0 0014 0" == flags 0,0 then dx=14 dy=0.
            let sp = parse_path("M5 11a7 7 0 0014 0");
            let last = sp[0].points.last().copied().unwrap_or_default();
            assert!((last.0 - 19.0).abs() < 1e-3 && (last.1 - 11.0).abs() < 1e-3);
            // sweep=0 from left to right bulges downward (y grows) on screen? it must stay on the circle
            for p in &sp[0].points {
                let d = ((p.0 - 12.0).powi(2) + (p.1 - 11.0).powi(2)).sqrt();
                assert!((d - 7.0).abs() < 0.05, "point {p:?} off circle: {d}");
            }
        }

        #[test]
        fn cubic_relative_reaches_end() {
            let sp = parse_path("M12 3l8 3v6c0 4.5-3.4 8-8 9");
            let last = sp[0].points.last().copied().unwrap_or_default();
            assert!((last.0 - 12.0).abs() < 1e-3 && (last.1 - 21.0).abs() < 1e-3);
        }

        #[test]
        fn every_icon_has_geometry() {
            use Icon::*;
            for icon in [
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
                Pause,
                Play,
                Mic,
                Speaker,
                Webcam,
                Clicks,
                Search,
                Star,
                StarFilled,
                Folder,
                Trash,
                History,
                Shield,
                ChevronDown,
                Settings,
            ] {
                assert!(!prims(icon).is_empty(), "{icon:?}");
                for p in prims(icon) {
                    if let Path(d) | Fill(d) = p {
                        let sp = parse_path(d);
                        assert!(
                            !sp.is_empty() && sp.iter().all(|s| s.points.len() >= 2),
                            "{icon:?}: {d}"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod hc_tests {
    use super::*;

    #[test]
    fn high_contrast_palette_uses_the_system_colors() {
        let hc = crate::a11y::HighContrast {
            window: [0, 0, 0],
            text: [255, 255, 255],
            button: [0, 0, 0],
            button_text: [255, 255, 255],
            highlight: [26, 235, 255],
            highlight_text: [0, 0, 0],
            disabled: [63, 242, 63],
            link: [255, 255, 0],
        };
        let p = high_contrast_palette(&hc);
        assert_eq!(p.bg, Color32::BLACK);
        assert_eq!(p.text_tertiary, Color32::WHITE);
        assert_eq!(p.stroke, Color32::WHITE);
        assert!(contrast(p.text, p.bg) >= 7.0);
        assert!(contrast(p.on_accent, rgb(hc.highlight)) >= 4.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_matches_wcag_extremes() {
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn accent_fg_is_legible_on_both_themes() {
        for accent in [
            DEFAULT_ACCENT,
            Color32::from_rgb(0xff, 0xb4, 0x54),
            Color32::from_rgb(0x4c, 0xc2, 0xff),
            Color32::from_rgb(0xc4, 0x9c, 0xff),
        ] {
            assert!(contrast(accent_fg(accent, false), LIGHT.bg) >= 4.5);
            assert!(contrast(accent_fg(accent, true), DARK.control) >= 4.5);
        }
        // Already legible accents are left alone.
        assert_eq!(accent_fg(DEFAULT_ACCENT, true), DEFAULT_ACCENT);
    }

    #[test]
    fn secondary_text_meets_aa_on_every_surface() {
        for p in [&DARK, &LIGHT] {
            for bg in [p.bg, p.layer, p.card, p.flyout, p.control] {
                assert!(contrast(p.text_secondary, bg) >= 4.5, "{bg:?}");
                assert!(contrast(p.text_tertiary, bg) >= 4.5, "{bg:?}");
            }
        }
        assert!(contrast(on_danger(true), DARK.danger) >= 4.5);
        assert!(contrast(on_danger(false), LIGHT.danger) >= 4.5);
    }
}

/// Rows for every popup list (context menus and dropdowns), Windows 11 menu proportions:
/// 34 pt rows with a 2 pt gap (hover highlights never touch), text 16 pt in from the popup
/// edge (6 pt `menu_margin` + 10 pt padding), highlight 6 pt in from it. The global
/// spacing (32 pt controls plus 8 pt gaps) is sized for forms and made lists look loose.
pub fn popup_rows(ui: &mut egui::Ui) {
    let s = ui.spacing_mut();
    s.item_spacing.y = 2.0;
    s.interact_size.y = 34.0;
    s.button_padding = egui::vec2(10.0, 7.0);
}

/// `popup_rows` plus a minimum width, for right-click menus.
pub fn menu_rows(ui: &mut egui::Ui) {
    popup_rows(ui);
    ui.set_min_width(200.0);
}
