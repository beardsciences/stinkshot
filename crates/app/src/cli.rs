//! Command line. `stinkshot` with no args starts the tray app.
//!
//!   stinkshot [--tray]                      start (tray only, no window)
//!   stinkshot region|window|full|scroll|ocr|repeat|record|gif
//!                                           trigger in the running instance (starts it if needed)
//!   stinkshot history|settings              open windows
//!   stinkshot settings --page help          open Settings on a page (help, about, ...)
//!   stinkshot full --out <file> [--monitor N] [--cursor]
//!   stinkshot window --out <file>           (foreground window)  headless: no UI, exits
//!   stinkshot ocr --file <image>            print recognized text to stdout, exits
//!   stinkshot quit                          ask the running instance to exit and wait (installers)
//!   stinkshot --version

use std::path::PathBuf;

use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use stinkshot_core::Rgba;
use stinkshot_core::image::ImageFormat;

use crate::state::Command;

#[derive(Debug)]
pub enum CliAction {
    /// Start (or forward to) the app; optionally run a command right away.
    RunApp {
        initial: Option<Command>,
        forward_args: Vec<String>,
    },
    Headless(HeadlessJob),
    /// Ask the running instance (if any) to quit and wait until it has. Never starts one.
    QuitRunning,
}

#[derive(Debug)]
pub enum HeadlessJob {
    Full {
        out: PathBuf,
        monitor: Option<usize>,
        cursor: bool,
    },
    Window {
        out: PathBuf,
        cursor: bool,
    },
    OcrFile {
        file: PathBuf,
    },
}

#[derive(Parser, Debug)]
#[command(
    name = "stinkshot",
    version,
    about = "Local-first screenshot and screen recording for Windows",
    long_about = "Local-first screenshot and screen recording for Windows.\n\nWith no arguments Stinkshot starts in the tray. Subcommands trigger an action in the running instance (starting it if needed); `full --out`, `window --out` and `ocr --file` run headless and exit."
)]
struct Cli {
    /// Start in the tray only (used by start-with-Windows).
    #[arg(long)]
    tray: bool,
    #[command(subcommand)]
    cmd: Option<Sub>,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Select a region (overlay editor).
    Region,
    /// Capture the foreground window; with --out, headless.
    Window {
        /// Write the capture to this file (png, jpg or webp by extension) and exit.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Include the mouse cursor.
        #[arg(long)]
        cursor: bool,
    },
    /// Capture a full monitor; with --out, headless.
    Full {
        /// Write the capture to this file (png, jpg or webp by extension) and exit.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Monitor number, 1-based, in system order. Default: the monitor under the cursor.
        #[arg(short, long)]
        monitor: Option<usize>,
        /// Include the mouse cursor.
        #[arg(long)]
        cursor: bool,
    },
    /// Scrolling capture.
    Scroll,
    /// Select a region and copy its text; with --file, print the text of an image and exit.
    Ocr {
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
    /// Capture the last region again.
    Repeat,
    /// Pick a color anywhere on screen and copy it (hex, RGB or HSL).
    PickColor,
    /// Record the screen as MP4 (toggles: stops a running recording).
    Record,
    /// Record the screen as GIF (toggles: stops a running recording).
    Gif,
    /// Open the history window.
    History,
    /// Open the settings window.
    Settings {
        /// Page to open, e.g. help or about.
        #[arg(long)]
        page: Option<String>,
    },
    /// Ask the running instance to exit and wait for it (used by the installer).
    Quit,
    /// Soak test driver commands (`soak` feature builds only).
    #[cfg(feature = "soak")]
    #[command(hide = true)]
    Soak {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn command_of(sub: &Sub) -> Command {
    match sub {
        Sub::Region => Command::CaptureRegion,
        Sub::Window { .. } => Command::CaptureWindow,
        Sub::Full { .. } => Command::CaptureFullscreen,
        Sub::Scroll => Command::CaptureScrolling,
        Sub::Ocr { .. } => Command::OcrRegion,
        Sub::Repeat => Command::RepeatLastRegion,
        Sub::PickColor => Command::PickColor,
        Sub::Record => Command::RecordMp4,
        Sub::Gif => Command::RecordGif,
        Sub::History => Command::OpenHistory,
        Sub::Settings { page } => match page
            .as_deref()
            .and_then(crate::settings_ui::Section::from_name)
        {
            Some(s) => Command::OpenSettingsAt(s),
            None => Command::OpenSettings,
        },
        Sub::Quit => Command::Quit,
        #[cfg(feature = "soak")]
        Sub::Soak { args } => Command::Soak(args.clone()),
    }
}

fn headless_of(sub: &Sub) -> Option<HeadlessJob> {
    match sub {
        Sub::Window {
            out: Some(out),
            cursor,
        } => Some(HeadlessJob::Window {
            out: out.clone(),
            cursor: *cursor,
        }),
        Sub::Full {
            out: Some(out),
            monitor,
            cursor,
        } => Some(HeadlessJob::Full {
            out: out.clone(),
            monitor: monitor.map(|m| m.saturating_sub(1)),
            cursor: *cursor,
        }),
        Sub::Ocr { file: Some(file) } => Some(HeadlessJob::OcrFile { file: file.clone() }),
        _ => None,
    }
}

fn into_action(cli: Cli, raw_args: Vec<String>) -> CliAction {
    if let Some(sub) = &cli.cmd {
        if matches!(sub, Sub::Quit) {
            return CliAction::QuitRunning;
        }
        if let Some(job) = headless_of(sub) {
            return CliAction::Headless(job);
        }
        return CliAction::RunApp {
            initial: Some(command_of(sub)),
            forward_args: raw_args,
        };
    }
    // Plain `stinkshot` (not `--tray`) launched again by the user: open History in the
    // running instance instead of silently doing nothing.
    let forward_args = if cli.tray {
        Vec::new()
    } else {
        vec!["history".to_string()]
    };
    CliAction::RunApp {
        initial: None,
        forward_args,
    }
}

/// Parse the process arguments. Prints help/version/usage errors and exits as clap does.
pub fn parse() -> CliAction {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cli = Cli::parse();
    into_action(cli, raw)
}

/// Map forwarded IPC args (same grammar as `parse`) to a command.
pub fn command_from_args(args: &[String]) -> Option<Command> {
    let argv = std::iter::once("stinkshot".to_string()).chain(args.iter().cloned());
    let cli = Cli::try_parse_from(argv).ok()?;
    cli.cmd.as_ref().map(command_of)
}

fn format_for(path: &std::path::Path) -> ImageFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => ImageFormat::Jpeg,
        Some("webp") => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

fn save_to(img: &Rgba, out: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    img.save(out, format_for(out), 90)
        .with_context(|| format!("write {}", out.display()))
}

pub fn run_headless(job: HeadlessJob) -> anyhow::Result<()> {
    use stinkshot_platform::{capture, input, monitors, windows_enum};
    match job {
        HeadlessJob::Full {
            out,
            monitor,
            cursor,
        } => {
            let mons = monitors::list();
            let rect = match monitor {
                Some(i) => {
                    mons.get(i)
                        .ok_or_else(|| {
                            anyhow!("monitor {} not found ({} connected)", i + 1, mons.len())
                        })?
                        .rect
                }
                None => monitors::at_point(input::cursor_pos())
                    .map(|m| m.rect)
                    .unwrap_or_else(monitors::virtual_bounds),
            };
            // One shot: GDI for just this monitor. A duplication session (what the running
            // app keeps warm) would cost far more to build than it saves here.
            let img = capture::capture_rect_once(rect, cursor)?;
            if img.width == 0 || img.height == 0 {
                return Err(anyhow!("monitor rectangle is outside the captured desktop"));
            }
            save_to(&img, &out)
        }
        HeadlessJob::Window { out, cursor } => {
            let cfg = stinkshot_core::config::Config::load();
            let win = windows_enum::foreground().ok_or_else(|| anyhow!("no foreground window"))?;
            let img = crate::capture_ops::capture_window_image(&cfg, &win, cursor)?;
            save_to(&img, &out)
        }
        HeadlessJob::OcrFile { file } => {
            let img = Rgba::load(&file).with_context(|| format!("read {}", file.display()))?;
            let engine =
                stinkshot_platform::ocr::WindowsOcr::new().context("no OCR language installed")?;
            use stinkshot_platform::ocr::OcrEngine;
            let result = engine.recognize(&img)?;
            println!("{}", result.text);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn cli(a: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("stinkshot").chain(a.iter().copied())).unwrap()
    }

    /// The soak test driver's commands exist only in `--features soak` builds.
    #[test]
    #[cfg(not(feature = "soak"))]
    fn soak_commands_are_not_in_normal_builds() {
        assert!(command_from_args(&args(&["soak", "window", "1"])).is_none());
        assert!(
            Cli::try_parse_from(["stinkshot", "soak", "search", "x"]).is_err(),
            "soak must not parse"
        );
    }

    #[test]
    fn maps_simple_commands() {
        assert!(matches!(
            command_from_args(&args(&["region"])),
            Some(Command::CaptureRegion)
        ));
        assert!(matches!(
            command_from_args(&args(&["scroll"])),
            Some(Command::CaptureScrolling)
        ));
        assert!(matches!(
            command_from_args(&args(&["repeat"])),
            Some(Command::RepeatLastRegion)
        ));
        assert!(matches!(
            command_from_args(&args(&["pick-color"])),
            Some(Command::PickColor)
        ));
        assert!(matches!(
            command_from_args(&args(&["record"])),
            Some(Command::RecordMp4)
        ));
        assert!(matches!(
            command_from_args(&args(&["gif"])),
            Some(Command::RecordGif)
        ));
        assert!(matches!(
            command_from_args(&args(&["history"])),
            Some(Command::OpenHistory)
        ));
        assert!(matches!(
            command_from_args(&args(&["settings"])),
            Some(Command::OpenSettings)
        ));
        assert!(matches!(
            command_from_args(&args(&["settings", "--page", "help"])),
            Some(Command::OpenSettingsAt(crate::settings_ui::Section::Help))
        ));
        assert!(matches!(
            command_from_args(&args(&["ocr"])),
            Some(Command::OcrRegion)
        ));
        assert!(matches!(
            command_from_args(&args(&["full"])),
            Some(Command::CaptureFullscreen)
        ));
        assert!(matches!(
            command_from_args(&args(&["window"])),
            Some(Command::CaptureWindow)
        ));
    }

    #[test]
    fn no_command_for_tray_or_garbage() {
        assert!(command_from_args(&[]).is_none());
        assert!(command_from_args(&args(&["--tray"])).is_none());
        assert!(command_from_args(&args(&["frobnicate"])).is_none());
    }

    #[test]
    fn headless_jobs() {
        match into_action(
            cli(&["full", "--out", "a.png", "--monitor", "2", "--cursor"]),
            vec![],
        ) {
            CliAction::Headless(HeadlessJob::Full {
                out,
                monitor,
                cursor,
            }) => {
                assert_eq!(out, PathBuf::from("a.png"));
                assert_eq!(monitor, Some(1));
                assert!(cursor);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            into_action(cli(&["window", "-o", "w.jpg"]), vec![]),
            CliAction::Headless(HeadlessJob::Window { cursor: false, .. })
        ));
        assert!(matches!(
            into_action(cli(&["ocr", "--file", "x.png"]), vec![]),
            CliAction::Headless(HeadlessJob::OcrFile { .. })
        ));
    }

    #[test]
    fn trigger_without_out_runs_app_and_forwards() {
        match into_action(cli(&["full"]), args(&["full"])) {
            CliAction::RunApp {
                initial: Some(Command::CaptureFullscreen),
                forward_args,
            } => {
                assert_eq!(forward_args, args(&["full"]));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bare_launch_forwards_history_but_tray_does_not() {
        match into_action(cli(&[]), vec![]) {
            CliAction::RunApp {
                initial: None,
                forward_args,
            } => assert_eq!(forward_args, args(&["history"])),
            other => panic!("unexpected {other:?}"),
        }
        match into_action(cli(&["--tray"]), args(&["--tray"])) {
            CliAction::RunApp {
                initial: None,
                forward_args,
            } => assert!(forward_args.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn quit_never_starts_an_instance() {
        assert!(matches!(
            into_action(cli(&["quit"]), args(&["quit"])),
            CliAction::QuitRunning
        ));
        // What the running instance does with the forwarded args.
        assert!(matches!(
            command_from_args(&args(&["quit"])),
            Some(Command::Quit)
        ));
    }

    #[test]
    fn extension_picks_format() {
        assert_eq!(format_for(std::path::Path::new("a.JPG")), ImageFormat::Jpeg);
        assert_eq!(
            format_for(std::path::Path::new("a.webp")),
            ImageFormat::Webp
        );
        assert_eq!(format_for(std::path::Path::new("a")), ImageFormat::Png);
    }
}
