use std::path::Path;

/// HKCU\Software\Microsoft\Windows\CurrentVersion\Run\StinkShot = "<exe>" --tray
pub fn set_autostart(enabled: bool, exe: &Path) -> anyhow::Result<()> {
    let _ = (enabled, exe);
    todo!("platform::system::set_autostart")
}

pub fn autostart_enabled() -> bool {
    todo!("platform::system::autostart_enabled")
}

/// Windows 11 routes PrintScreen to Snipping Tool when
/// HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled = 1, which steals the key
/// from RegisterHotKey. `take_over(true)` sets it to 0 (remembering the previous value in
/// HKCU\Software\StinkShot\PrevPrintScreenSnipping); `take_over(false)` restores it.
/// Takes effect without sign-out on current builds; returns whether a change was made.
pub fn take_over_printscreen(take: bool) -> anyhow::Result<bool> {
    let _ = take;
    todo!("platform::system::take_over_printscreen")
}

/// Explorer window with the file selected.
pub fn reveal_in_explorer(path: &Path) -> anyhow::Result<()> {
    let _ = path;
    todo!("platform::system::reveal_in_explorer")
}

/// ShellExecute "open".
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    let _ = path;
    todo!("platform::system::open_path")
}

/// True if Windows apps use dark mode (AppsUseLightTheme == 0).
pub fn system_dark_mode() -> bool {
    todo!("platform::system::system_dark_mode")
}

/// Windows accent color, if available.
pub fn system_accent() -> Option<[u8; 4]> {
    todo!("platform::system::system_accent")
}

/// Single-instance guard: named mutex "Local\StinkShot.Instance". Returns None if another
/// instance owns it. Keep the guard alive for the process lifetime.
pub struct InstanceGuard {
    #[allow(dead_code)]
    handle: isize,
}

pub fn acquire_single_instance() -> Option<InstanceGuard> {
    todo!("platform::system::acquire_single_instance")
}

/// Forward CLI args to the running instance via WM_COPYDATA to the hidden window titled
/// "StinkShot.IPC". Returns false if no instance window was found.
pub fn send_to_running_instance(args: &[String]) -> bool {
    let _ = args;
    todo!("platform::system::send_to_running_instance")
}

/// Create the hidden IPC message-only window on a dedicated thread; every WM_COPYDATA
/// payload (UTF-8 args joined by '\0') is sent to `tx`.
pub fn start_ipc_listener(tx: std::sync::mpsc::Sender<Vec<String>>) -> anyhow::Result<()> {
    let _ = tx;
    todo!("platform::system::start_ipc_listener")
}

/// Short beep-free shutter sound via PlaySound on a system sound alias, async.
pub fn play_capture_sound() {
    todo!("platform::system::play_capture_sound")
}
