//! English strings for the shell group: shell.rs, shell/, tray.rs, toast.rs, pin.rs,
//! record_ui.rs, errors.rs, actions.rs, cli.rs output and the rest of crates/app.
//! Keys start with `shell.`.

pub fn get(key: &str) -> Option<&'static str> {
    Some(match key {
        "shell.tray.history" => "History",
        "shell.tray.settings" => "Settings",
        "shell.tray.quit" => "Quit",
        _ => return None,
    })
}

/// Every key above, for the key test.
#[cfg(test)]
pub const KEYS: &[&str] = &[
    "shell.tray.history",
    "shell.tray.settings",
    "shell.tray.quit",
];
