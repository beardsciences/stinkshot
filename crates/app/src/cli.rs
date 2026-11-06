//! Command line. `stinkshot` with no args starts the tray app.
//!
//!   stinkshot [--tray]                      start (tray only, no window)
//!   stinkshot region|window|full|scroll|ocr|repeat|record|gif
//!                                           trigger in the running instance (starts it if needed)
//!   stinkshot history|settings              open windows
//!   stinkshot full --out <file> [--monitor N] [--cursor]
//!   stinkshot window --out <file>           (foreground window)  headless: no UI, exits
//!   stinkshot ocr --file <image>            print recognized text to stdout, exits
//!   stinkshot --version

use crate::state::Command;

#[derive(Debug)]
pub enum CliAction {
    /// Start (or forward to) the app; optionally run a command right away.
    RunApp { initial: Option<Command>, forward_args: Vec<String> },
    Headless(HeadlessJob),
}

#[derive(Debug)]
pub enum HeadlessJob {
    Full { out: std::path::PathBuf, monitor: Option<usize>, cursor: bool },
    Window { out: std::path::PathBuf, cursor: bool },
    OcrFile { file: std::path::PathBuf },
}

pub fn parse() -> CliAction {
    todo!("cli::parse")
}

/// Map forwarded IPC args (same grammar as `parse`) to a command.
pub fn command_from_args(args: &[String]) -> Option<Command> {
    let _ = args;
    todo!("cli::command_from_args")
}

pub fn run_headless(job: HeadlessJob) -> anyhow::Result<()> {
    let _ = job;
    todo!("cli::run_headless")
}
