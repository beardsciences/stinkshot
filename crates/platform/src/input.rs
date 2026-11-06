use stinkshot_core::PxPoint;

pub fn cursor_pos() -> PxPoint {
    todo!("platform::input::cursor_pos")
}

/// Scroll the window under `at` by `notches` (negative = down, like WHEEL_DELTA units of
/// 120 per notch) WITHOUT moving the user's cursor permanently: move, SendInput wheel,
/// restore position. Used by scrolling capture.
pub fn scroll_at(at: PxPoint, notches: i32) {
    let _ = (at, notches);
    todo!("platform::input::scroll_at")
}

/// Global low-level mouse/keyboard event feed for the recording click/key visualizer.
/// Installs WH_MOUSE_LL / WH_KEYBOARD_LL on a dedicated thread with its own message loop;
/// dropping the guard uninstalls the hooks. Hook callbacks must return quickly (push to
/// the channel and call next hook).
pub struct InputHook {
    #[allow(dead_code)]
    inner: (),
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseDown { pos: PxPoint, button: MouseButton },
    MouseUp { pos: PxPoint, button: MouseButton },
    /// Key down with a display label, e.g. "Ctrl", "Shift", "P", "Enter", "F5".
    KeyDown { label: String, vk: u32 },
    KeyUp { vk: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl InputHook {
    pub fn install(tx: std::sync::mpsc::Sender<(std::time::Instant, InputEvent)>) -> anyhow::Result<InputHook> {
        let _ = tx;
        todo!("platform::input::InputHook::install")
    }
}
