//! First-run tour: four short, skippable steps (hotkeys, tray, privacy, where captures go).
//! The shell opens it once, when `general.tour_seen` is false; closing it any way (Done,
//! Skip, Esc, the close box) sets the flag. Settings > About > Show Tour opens it again.

use std::time::Duration;

use egui::{Align2, CornerRadius, RichText, Sense, UiBuilder, pos2, vec2};
use stinkshot_core::config::Config;
use stinkshot_platform::Hwnd;

use crate::actions::Action;
use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon, widgets};
use crate::toast::ui_common as uc;

/// Every user-visible string of the tour (one place, for the localization lookup later).
mod text {
    pub const WINDOW_TITLE: &str = "Welcome to Stinkshot";
    pub const SKIP: &str = "Skip Tour";
    pub const BACK: &str = "Back";
    pub const NEXT: &str = "Next";
    pub const DONE: &str = "Done";
    pub const OPEN_HELP: &str = "Open Help";

    pub const HOTKEYS_TITLE: &str = "Capture With a Hotkey";
    /// `{key}` is the region hotkey.
    pub const HOTKEYS_BODY: &str = "Press {key} to capture a region. Then draw, copy or save.";
    pub const HOTKEYS_BODY_UNSET: &str =
        "Set a region hotkey in Settings > Hotkeys, or use the tray icon.";
    pub const NOT_SET: &str = "Not Set";

    pub const TRAY_TITLE: &str = "It Lives in the Tray";
    pub const TRAY_BODY: &str = "Click the tray icon to capture, double-click for History.";

    pub const PRIVACY_TITLE: &str = "Nothing Ever Leaves Your PC";
    pub const PRIVACY_BODY: &str = "No accounts, uploads or telemetry. Update checks are opt-in.";

    pub const FILES_TITLE: &str = "Where Captures Go";
    pub const FILES_BODY: &str =
        "Saved here and listed in History. Change it in Settings > History.";
}

/// Which global hotkeys the first step lists (the rest are in Help).
const STEP_ACTIONS: [Action; 4] = [
    Action::Region,
    Action::Window,
    Action::Fullscreen,
    Action::RecordMp4,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Hotkeys,
    Tray,
    Privacy,
    Files,
}

const STEPS: [Step; 4] = [Step::Hotkeys, Step::Tray, Step::Privacy, Step::Files];

impl Step {
    fn icon(self) -> Icon {
        match self {
            Step::Hotkeys => Icon::Regions,
            Step::Tray => Icon::Settings,
            Step::Privacy => Icon::Shield,
            Step::Files => Icon::Folder,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Step::Hotkeys => text::HOTKEYS_TITLE,
            Step::Tray => text::TRAY_TITLE,
            Step::Privacy => text::PRIVACY_TITLE,
            Step::Files => text::FILES_TITLE,
        }
    }
}

/// The step body for this config.
fn body(step: Step, cfg: &Config) -> String {
    match step {
        Step::Hotkeys => {
            let key = crate::hotkeys::display(&cfg.hotkeys.region);
            if key.is_empty() {
                text::HOTKEYS_BODY_UNSET.into()
            } else {
                text::HOTKEYS_BODY.replace("{key}", &key)
            }
        }
        Step::Tray => text::TRAY_BODY.into(),
        Step::Privacy => text::PRIVACY_BODY.into(),
        Step::Files => text::FILES_BODY.into(),
    }
}

/// Extra (key, label) rows under the body: the main hotkeys, or the captures folder.
fn detail_rows(step: Step, cfg: &Config) -> Vec<(String, String)> {
    match step {
        Step::Hotkeys => STEP_ACTIONS
            .iter()
            .map(|a| {
                let k = crate::hotkeys::display(a.hotkey(&cfg.hotkeys));
                let k = if k.is_empty() {
                    text::NOT_SET.to_string()
                } else {
                    k
                };
                (k, a.label().to_string())
            })
            .collect(),
        Step::Files => vec![(cfg.history_folder().display().to_string(), String::new())],
        Step::Tray | Step::Privacy => Vec::new(),
    }
}

pub struct TourWindow {
    step: usize,
    done: bool,
}

impl TourWindow {
    pub fn new() -> Self {
        Self {
            step: 0,
            done: false,
        }
    }

    /// The window title (the shell focuses an open tour by it).
    pub const TITLE: &'static str = text::WINDOW_TITLE;
}

impl Surface for TourWindow {
    fn title(&self) -> String {
        Self::TITLE.into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(560.0, 400.0))
            .with_resizable(false)
            .with_maximize_button(false)
            .with_minimize_button(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        stinkshot_platform::window_util::force_foreground(hwnd);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let step = STEPS[self.step.min(STEPS.len() - 1)];
        let last = self.step + 1 >= STEPS.len();
        let mut close = ctx.input(|i| i.viewport().close_requested())
            || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let (mut next, mut back, mut help) = (false, false, false);
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::Enter)) {
            next = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            back = true;
        }

        uc::root_ui(ctx, p.bg, |ui, rect| {
            let area = rect.shrink2(vec2(32.0, 28.0));
            // Step dots, top right.
            for i in 0..STEPS.len() {
                let c = pos2(
                    area.right() - (STEPS.len() - 1 - i) as f32 * 14.0 - 4.0,
                    area.top() + 10.0,
                );
                let col = if i == self.step {
                    accent
                } else {
                    theme::subtle_fill(shared.dark, 3)
                };
                ui.painter().circle_filled(c, 4.0, col);
            }
            widgets::paint_icon(
                ui.painter(),
                egui::Rect::from_min_size(area.min, vec2(36.0, 36.0)),
                step.icon(),
                accent,
            );
            let content = egui::Rect::from_min_max(area.min + vec2(0.0, 52.0), area.max);
            ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(
                    RichText::new(step.title())
                        .font(theme::semibold(22.0))
                        .color(p.text),
                );
                ui.label(
                    RichText::new(body(step, &shared.config))
                        .size(14.0)
                        .color(p.text_secondary),
                );
                let rows = detail_rows(step, &shared.config);
                if !rows.is_empty() {
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1.0, p.stroke))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for (k, v) in &rows {
                                ui.horizontal(|ui| {
                                    if v.is_empty() {
                                        ui.label(RichText::new(k).size(13.0).color(p.text));
                                    } else {
                                        let (r, _) = ui
                                            .allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
                                        ui.painter().text(
                                            r.left_center(),
                                            Align2::LEFT_CENTER,
                                            k,
                                            theme::semibold(13.0),
                                            p.text,
                                        );
                                        ui.label(
                                            RichText::new(v).size(13.0).color(p.text_secondary),
                                        );
                                    }
                                });
                            }
                        });
                }
            });

            // Buttons, bottom row.
            let bar = egui::Rect::from_min_max(pos2(area.left(), area.bottom() - 34.0), area.max);
            ui.scope_builder(UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_centered(|ui| {
                    if !last && widgets::button(ui, text::SKIP).clicked() {
                        close = true;
                    }
                    if last && widgets::button(ui, text::OPEN_HELP).clicked() {
                        help = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let primary = if last { text::DONE } else { text::NEXT };
                        if widgets::accent_button(ui, primary).clicked() {
                            next = true;
                        }
                        if self.step > 0 && widgets::button(ui, text::BACK).clicked() {
                            back = true;
                        }
                    });
                });
            });
        });

        if back && self.step > 0 {
            self.step -= 1;
        } else if next {
            if last {
                close = true;
            } else {
                self.step += 1;
            }
        }
        if help {
            shared
                .outbox
                .push(Command::OpenSettingsAt(crate::settings_ui::Section::Help));
            close = true;
        }
        if close && !self.done {
            self.done = true;
            if !shared.config.general.tour_seen {
                shared.config.general.tour_seen = true;
                shared.config_dirty = true;
            }
        }
        // Keep the dots and hover states fresh without spinning.
        ctx.request_repaint_after(Duration::from_secs(1));
        if close {
            SurfaceStatus::Close
        } else {
            SurfaceStatus::Open
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_steps_with_the_privacy_promise() {
        assert_eq!(STEPS.len(), 4);
        assert!(
            STEPS
                .iter()
                .any(|s| s.title() == "Nothing Ever Leaves Your PC")
        );
    }

    #[test]
    fn hotkey_step_names_the_region_key_or_says_how_to_set_one() {
        let mut cfg = Config::default();
        cfg.hotkeys.region = "PrintScreen".into();
        assert!(body(Step::Hotkeys, &cfg).contains("Press PrtSc"));
        cfg.hotkeys.region.clear();
        assert_eq!(body(Step::Hotkeys, &cfg), text::HOTKEYS_BODY_UNSET);
        let rows = detail_rows(Step::Hotkeys, &cfg);
        assert_eq!(rows.len(), STEP_ACTIONS.len());
        assert_eq!(
            rows[0],
            (text::NOT_SET.into(), Action::Region.label().into())
        );
    }

    #[test]
    fn files_step_shows_the_captures_folder() {
        let cfg = Config::default();
        let rows = detail_rows(Step::Files, &cfg);
        assert_eq!(rows[0].0, cfg.history_folder().display().to_string());
    }

    #[test]
    fn bodies_are_short() {
        let cfg = Config::default();
        for s in STEPS {
            assert!(body(s, &cfg).len() <= 70, "{s:?}");
        }
    }

    #[test]
    fn tour_is_unseen_by_default() {
        assert!(!Config::default().general.tour_seen);
    }
}
