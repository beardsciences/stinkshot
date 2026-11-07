//! Capture helpers shared by the shell, its workers and the headless CLI.

use anyhow::Context;
use stinkshot_core::config::{Config, parse_hex_color};
use stinkshot_core::frame::{self, WindowFx};
use stinkshot_core::{PxPoint, PxRect, Rgba};
use stinkshot_platform::capture::{self, Frozen, WindowCaptureOpts};
use stinkshot_platform::windows_enum::WinInfo;
use stinkshot_platform::{monitors, windows_enum};

use crate::state::SourceInfo;

pub fn source_of(win: &WinInfo) -> SourceInfo {
    SourceInfo {
        app: win.process.clone(),
        window_title: win.title.clone(),
    }
}

pub fn center(rect: PxRect) -> PxPoint {
    PxPoint {
        x: rect.x + rect.w / 2,
        y: rect.y + rect.h / 2,
    }
}

/// Monitor scale at the centre of `rect` (1.0 if unknown).
pub fn scale_at(rect: PxRect) -> f32 {
    monitors::at_point(center(rect))
        .map(|m| m.scale)
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

/// Rounded corners / shadow / padding for a window capture, or None if all are off.
pub fn window_fx(cfg: &Config, win: &WinInfo) -> Option<WindowFx> {
    let c = &cfg.capture;
    let scale = scale_at(win.rect);
    let corner_radius = if c.window_rounded_corners && !win.maximized {
        8.0 * scale
    } else {
        0.0
    };
    let padding = c.window_padding;
    if corner_radius <= 0.0 && !c.window_shadow && !padding {
        return None;
    }
    Some(WindowFx {
        corner_radius,
        shadow: c.window_shadow,
        shadow_blur: 24.0 * scale,
        shadow_opacity: 0.38,
        padding: if padding {
            (c.window_padding_px as f32 * scale).round() as u32
        } else {
            0
        },
        background: if padding {
            parse_hex_color(&c.window_padding_color)
        } else {
            None
        },
    })
}

/// WGC capture of `win` with the configured window effects applied.
pub fn capture_window_image(cfg: &Config, win: &WinInfo, cursor: bool) -> anyhow::Result<Rgba> {
    let img = capture::capture_window(
        win.hwnd,
        &WindowCaptureOpts {
            include_cursor: cursor,
        },
    )
    .with_context(|| format!("capture window \"{}\"", win.title))?;
    Ok(match window_fx(cfg, win) {
        Some(fx) => frame::apply_window_fx(&img, &fx),
        None => img,
    })
}

/// The window a "capture the current window" request means: the foreground window when it
/// is a real capture target, otherwise the topmost visible window. After a tray menu click
/// the foreground is usually the tray host or the taskbar, which are never targets.
pub fn target_window() -> Option<WinInfo> {
    target_from(&windows_enum::visible_windows())
}

/// `target_window` over an existing Z-ordered snapshot of `visible_windows()` (which
/// already excludes this process and cloaked windows).
pub fn target_from(windows: &[WinInfo]) -> Option<WinInfo> {
    let usable = |w: &&WinInfo| w.process != "taskbar";
    if let Some(fg) = windows_enum::foreground()
        && let Some(w) = windows.iter().filter(usable).find(|w| w.hwnd == fg.hwnd)
    {
        return Some(w.clone());
    }
    windows.iter().find(usable).cloned()
}

/// Pixels of `rect` (virtual-desktop coords) out of a frozen desktop snapshot.
pub fn crop_frozen(frozen: &Frozen, rect: PxRect) -> Rgba {
    frozen
        .image
        .crop(rect.translate(-frozen.bounds.x, -frozen.bounds.y))
}

/// Full rect of the monitor containing `p`, or the whole snapshot if unknown.
pub fn monitor_rect_at(p: PxPoint, fallback: PxRect) -> PxRect {
    monitors::at_point(p).map(|m| m.rect).unwrap_or(fallback)
}

/// Topmost window containing the centre of `rect`, as a source description.
pub fn source_for_rect(windows: &[WinInfo], rect: PxRect) -> SourceInfo {
    windows_enum::window_at(windows, center(rect))
        .map(source_of)
        .unwrap_or_default()
}
