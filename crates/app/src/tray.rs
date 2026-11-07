//! Tray icon + menu (docs/design/Tray.dc.html): header "StinkShot <version> · offline",
//! capture modes with their hotkey labels, record MP4/GIF, History, Settings, Quit.
//! Left click = region capture; double click = History. Icon is the green gel orb
//! (`icon_gen`, pre-rendered at 16/20/24/32 px for DPI); it squishes and pulses after a
//! capture (`flash_capture`) and carries a red dot while recording (`set_recording`).

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::Context;
use stinkshot_core::config::{Config, parse_hex_color};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::hotkeys::display;
use crate::icon_gen;
use crate::state::Command;
use crate::update::Release;

/// A single left click waits this long for a possible double click before it counts.
const CLICK_DELAY: Duration = Duration::from_millis(300);
/// The trailing button-up of a double click must not count as a new single click.
const DOUBLE_CLICK_QUIET: Duration = Duration::from_millis(700);

enum Msg {
    Menu(String),
    LeftClick,
    DoubleClick,
}

struct Entry {
    label: &'static str,
    item: MenuItem,
    hotkey: fn(&Config) -> &str,
}

pub struct Tray {
    icon: TrayIcon,
    menu: Menu,
    header: MenuItem,
    entries: Vec<Entry>,
    update_item: Option<MenuItem>,
    update_url: Option<String>,
    rx: Receiver<Msg>,
    pending_click: Option<Instant>,
    last_double: Option<Instant>,
    accent: [u8; 3],
    dpi_size: u32,
    /// Capture animation start and the frame currently shown (`None` = rest pose).
    anim_start: Option<Instant>,
    anim_frame: Option<usize>,
    recording: bool,
}

type MenuSpec = (&'static str, &'static str, fn(&Config) -> &str);

const ID_HISTORY: &str = "history";
const ID_SETTINGS: &str = "settings";
const ID_QUIT: &str = "quit";
const ID_UPDATE: &str = "update";

fn header_text(config: &Config) -> String {
    let state = if config.privacy.check_for_updates {
        "Update Check On"
    } else {
        "Offline"
    };
    format!("StinkShot {} \u{b7} {}", env!("CARGO_PKG_VERSION"), state)
}

fn entry_text(label: &str, hotkey: &str) -> String {
    let key = display(hotkey);
    if key.is_empty() {
        label.to_string()
    } else {
        format!("{label}\t{key}")
    }
}

fn accent_of(config: &Config) -> [u8; 3] {
    let c = parse_hex_color(&config.general.accent).unwrap_or([0x7f, 0xd3, 0x6a, 0xff]);
    [c[0], c[1], c[2]]
}

fn make_icon(size: u32, accent: [u8; 3], pose: icon_gen::Pose) -> anyhow::Result<Icon> {
    Icon::from_rgba(icon_gen::render_pose(size, accent, pose), size, size)
        .context("build tray icon")
}

impl Tray {
    pub fn new(config: &Config) -> anyhow::Result<Tray> {
        let menu = Menu::new();
        let header = MenuItem::new(header_text(config), false, None);
        menu.append(&header).context("tray menu")?;

        let spec: [MenuSpec; 8] = [
            ("region", "Region", |c| &c.hotkeys.region),
            ("window", "Window", |c| &c.hotkeys.window),
            ("full", "Full Screen", |c| &c.hotkeys.fullscreen),
            ("scroll", "Scrolling", |c| &c.hotkeys.scrolling),
            ("ocr", "Copy Text (OCR)", |c| &c.hotkeys.ocr),
            ("repeat", "Repeat Last Region", |c| {
                &c.hotkeys.repeat_last_region
            }),
            ("rec_mp4", "Record MP4", |c| &c.hotkeys.record_mp4),
            ("rec_gif", "Record GIF", |c| &c.hotkeys.record_gif),
        ];
        let mut entries = Vec::new();
        for (i, (id, label, hotkey)) in spec.into_iter().enumerate() {
            if i == 6 {
                menu.append(&PredefinedMenuItem::separator())
                    .context("tray menu")?;
            }
            let item = MenuItem::with_id(id, entry_text(label, hotkey(config)), true, None);
            menu.append(&item).context("tray menu")?;
            entries.push(Entry {
                label,
                item,
                hotkey,
            });
        }
        menu.append(&PredefinedMenuItem::separator())
            .context("tray menu")?;
        menu.append(&MenuItem::with_id(ID_HISTORY, "History", true, None))
            .context("tray menu")?;
        menu.append(&MenuItem::with_id(ID_SETTINGS, "Settings", true, None))
            .context("tray menu")?;
        menu.append(&MenuItem::with_id(ID_QUIT, "Quit", true, None))
            .context("tray menu")?;

        let accent = accent_of(config);
        let dpi_size = icon_gen::tray_size_for_dpi(crate::sysutil::system_dpi());
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .with_tooltip("StinkShot")
            .with_icon(make_icon(dpi_size, accent, icon_gen::Pose::REST)?)
            .build()
            .context("create tray icon")?;

        let (tx, rx) = channel();
        let tray_events = TrayIconEvent::receiver().clone();
        let menu_events = MenuEvent::receiver().clone();
        let tx_tray = tx.clone();
        std::thread::Builder::new()
            .name("tray-bridge".into())
            .spawn(move || {
                while let Ok(ev) = tray_events.recv() {
                    let msg = match ev {
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } => Some(Msg::LeftClick),
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => Some(Msg::DoubleClick),
                        _ => None,
                    };
                    if let Some(m) = msg {
                        if tx_tray.send(m).is_err() {
                            break;
                        }
                        crate::wake::wake();
                    }
                }
            })
            .context("start tray bridge")?;
        std::thread::Builder::new()
            .name("tray-menu-bridge".into())
            .spawn(move || {
                while let Ok(ev) = menu_events.recv() {
                    if tx.send(Msg::Menu(ev.id.0.clone())).is_err() {
                        break;
                    }
                    crate::wake::wake();
                }
            })
            .context("start tray menu bridge")?;

        Ok(Tray {
            icon,
            menu,
            header,
            entries,
            update_item: None,
            update_url: None,
            rx,
            pending_click: None,
            last_double: None,
            accent,
            dpi_size,
            anim_start: None,
            anim_frame: None,
            recording: false,
        })
    }

    /// Play the short squish-and-pulse animation (a capture just completed). `poll` advances it.
    pub fn flash_capture(&mut self) {
        self.anim_start = Some(Instant::now());
        self.anim_frame = None;
        crate::wake::wake();
    }

    /// Show or clear the red recording dot. Cheap to call every frame.
    pub fn set_recording(&mut self, on: bool) {
        if on != self.recording {
            self.recording = on;
            self.refresh_icon();
        }
    }

    fn pose(&self) -> icon_gen::Pose {
        let base = self
            .anim_frame
            .map(|f| icon_gen::ANIM[f])
            .unwrap_or(icon_gen::Pose::REST);
        icon_gen::Pose {
            recording: self.recording,
            ..base
        }
    }

    fn refresh_icon(&mut self) {
        match make_icon(self.dpi_size, self.accent, self.pose()) {
            Ok(icon) => {
                if let Err(e) = self.icon.set_icon(Some(icon)) {
                    log::warn!("tray icon update failed: {e}");
                }
            }
            Err(e) => log::warn!("tray icon rebuild failed: {e:#}"),
        }
    }

    fn tick_animation(&mut self) {
        let Some(start) = self.anim_start else {
            return;
        };
        let frame_len = Duration::from_millis(icon_gen::ANIM_FRAME_MS);
        let elapsed = start.elapsed();
        let idx = (elapsed.as_millis() / frame_len.as_millis()) as usize;
        let next = if idx < icon_gen::ANIM.len() {
            Some(idx)
        } else {
            self.anim_start = None;
            None
        };
        if next != self.anim_frame {
            self.anim_frame = next;
            self.refresh_icon();
        }
        if self.anim_start.is_some() {
            let due = frame_len * (idx as u32 + 1);
            crate::wake::wake_after(due.saturating_sub(elapsed) + Duration::from_millis(2));
        }
    }

    /// Refresh hotkey labels (and accent / header) after settings change.
    pub fn update_labels(&mut self, config: &Config) {
        self.header.set_text(header_text(config));
        for e in &self.entries {
            e.item.set_text(entry_text(e.label, (e.hotkey)(config)));
        }
        let accent = accent_of(config);
        if accent != self.accent {
            self.accent = accent;
            self.refresh_icon();
        }
    }

    /// Show (or clear) an "Update available" entry at the top of the menu.
    pub fn set_update(&mut self, release: Option<&Release>) {
        if let Some(old) = self.update_item.take()
            && let Err(e) = self.menu.remove(&old)
        {
            log::debug!("remove update item: {e}");
        }
        self.update_url = None;
        if let Some(r) = release {
            let item = MenuItem::with_id(
                ID_UPDATE,
                format!("Update Available: {}", r.tag),
                !r.url.is_empty(),
                None,
            );
            if let Err(e) = self.menu.insert(&item, 1) {
                log::warn!("tray update item: {e}");
                return;
            }
            self.update_url = Some(r.url.clone());
            self.update_item = Some(item);
        }
    }

    /// Drain menu/icon events into commands. Call every frame.
    pub fn poll(&mut self) -> Vec<Command> {
        self.tick_animation();
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Menu(id) => {
                    if let Some(c) = self.menu_command(&id) {
                        out.push(c);
                    }
                }
                Msg::LeftClick => {
                    let recent_double = self
                        .last_double
                        .is_some_and(|t| t.elapsed() < DOUBLE_CLICK_QUIET);
                    if !recent_double {
                        self.pending_click = Some(Instant::now());
                        crate::wake::wake_after(CLICK_DELAY + Duration::from_millis(30));
                    }
                }
                Msg::DoubleClick => {
                    self.pending_click = None;
                    self.last_double = Some(Instant::now());
                    out.push(Command::OpenHistory);
                }
            }
        }
        if let Some(t) = self.pending_click {
            if t.elapsed() >= CLICK_DELAY {
                self.pending_click = None;
                out.push(Command::CaptureRegion);
            } else {
                crate::wake::wake_after(CLICK_DELAY - t.elapsed() + Duration::from_millis(10));
            }
        }
        out
    }

    fn menu_command(&self, id: &str) -> Option<Command> {
        Some(match id {
            "region" => Command::CaptureRegion,
            "window" => Command::CaptureWindow,
            "full" => Command::CaptureFullscreen,
            "scroll" => Command::CaptureScrolling,
            "ocr" => Command::OcrRegion,
            "repeat" => Command::RepeatLastRegion,
            "rec_mp4" => Command::RecordMp4,
            "rec_gif" => Command::RecordGif,
            ID_HISTORY => Command::OpenHistory,
            ID_SETTINGS => Command::OpenSettings,
            ID_QUIT => Command::Quit,
            ID_UPDATE => return self.update_url.clone().map(Command::OpenUrl),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_text_has_tab_separated_hotkey() {
        assert_eq!(entry_text("Region", "PrintScreen"), "Region\tPrtSc");
        assert_eq!(
            entry_text("Scrolling", "Control+Shift+KeyS"),
            "Scrolling\tCtrl+Shift+S"
        );
        assert_eq!(entry_text("Window", ""), "Window");
    }

    #[test]
    fn header_mentions_offline_by_default() {
        let cfg = Config::default();
        assert!(header_text(&cfg).ends_with("Offline"));
        assert!(header_text(&cfg).starts_with("StinkShot "));
    }
}
