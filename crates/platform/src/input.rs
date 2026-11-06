use std::cell::RefCell;
use std::sync::mpsc::Sender;
use std::time::Instant;

use stinkshot_core::PxPoint;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SetCursorPos, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

pub fn cursor_pos() -> PxPoint {
    let mut p = POINT::default();
    // SAFETY: out pointer is a live local.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return PxPoint::default();
    }
    PxPoint { x: p.x, y: p.y }
}

/// Scroll the window under `at` by `notches` (negative = down, like WHEEL_DELTA units of
/// 120 per notch) WITHOUT moving the user's cursor permanently: move, SendInput wheel,
/// restore position. Used by scrolling capture.
pub fn scroll_at(at: PxPoint, notches: i32) {
    if notches == 0 {
        return;
    }
    let orig = cursor_pos();
    // SAFETY: SetCursorPos/SendInput take plain values; the INPUT array outlives the call.
    unsafe {
        if let Err(e) = SetCursorPos(at.x, at.y) {
            log::warn!("scroll_at: SetCursorPos failed: {e}");
        }
        // Let the pointer settle so the wheel event is routed to the window under `at`.
        std::thread::sleep(std::time::Duration::from_millis(12));
        let delta: i32 = if notches > 0 { 120 } else { -120 };
        let one = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = vec![one; notches.unsigned_abs() as usize];
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if (sent as usize) != inputs.len() {
            log::warn!(
                "scroll_at: SendInput delivered {sent}/{} events",
                inputs.len()
            );
        }
        // Give the target time to dequeue the wheel messages before the pointer moves back.
        std::thread::sleep(std::time::Duration::from_millis(25));
        let _ = SetCursorPos(orig.x, orig.y);
    }
}

/// Global low-level mouse/keyboard event feed for the recording click/key visualizer.
/// Installs WH_MOUSE_LL / WH_KEYBOARD_LL on a dedicated thread with its own message loop;
/// dropping the guard uninstalls the hooks. Hook callbacks must return quickly (push to
/// the channel and call next hook).
pub struct InputHook {
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseDown {
        pos: PxPoint,
        button: MouseButton,
    },
    MouseUp {
        pos: PxPoint,
        button: MouseButton,
    },
    /// Key down with a display label, e.g. "Ctrl", "Shift", "P", "Enter", "F5".
    KeyDown {
        label: String,
        vk: u32,
    },
    KeyUp {
        vk: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

struct HookState {
    tx: Sender<(Instant, InputEvent)>,
    down: [bool; 256],
}

thread_local! {
    // Hook procedures run on the installing thread, so thread-local state is enough.
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

fn emit(ev: InputEvent) {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let _ = st.tx.send((Instant::now(), ev));
        }
    });
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_MOUSE_LL, lparam points to a valid MSLLHOOKSTRUCT during the callback.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let pos = PxPoint {
            x: info.pt.x,
            y: info.pt.y,
        };
        let ev = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some(InputEvent::MouseDown {
                pos,
                button: MouseButton::Left,
            }),
            WM_LBUTTONUP => Some(InputEvent::MouseUp {
                pos,
                button: MouseButton::Left,
            }),
            WM_RBUTTONDOWN => Some(InputEvent::MouseDown {
                pos,
                button: MouseButton::Right,
            }),
            WM_RBUTTONUP => Some(InputEvent::MouseUp {
                pos,
                button: MouseButton::Right,
            }),
            WM_MBUTTONDOWN => Some(InputEvent::MouseDown {
                pos,
                button: MouseButton::Middle,
            }),
            WM_MBUTTONUP => Some(InputEvent::MouseUp {
                pos,
                button: MouseButton::Middle,
            }),
            _ => None,
        };
        if let Some(ev) = ev {
            emit(ev);
        }
    }
    // SAFETY: standard hook chaining.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a valid KBDLLHOOKSTRUCT during the callback.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = info.vkCode;
        let extended = info.flags.0 & 1 != 0;
        match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // Suppress auto-repeat: only the first down since the last up is reported.
                let fresh = STATE.with(|s| {
                    s.borrow_mut().as_mut().is_some_and(|st| {
                        let slot = &mut st.down[(vk & 0xff) as usize];
                        !std::mem::replace(slot, true)
                    })
                });
                if fresh {
                    emit(InputEvent::KeyDown {
                        label: key_label(vk, info.scanCode, extended),
                        vk,
                    });
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        st.down[(vk & 0xff) as usize] = false;
                    }
                });
                emit(InputEvent::KeyUp { vk });
            }
            _ => {}
        }
    }
    // SAFETY: standard hook chaining.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Human-readable label for a virtual key: "Ctrl", "Shift", "Alt", "Win", "Enter", "F5",
/// "P", "1", "Num 7", ... Falls back to the layout's key name.
pub fn key_label(vk: u32, scan: u32, extended: bool) -> String {
    let fixed = match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D => "Enter",
        0x10 | 0xA0 | 0xA1 => "Shift",
        0x11 | 0xA2 | 0xA3 => "Ctrl",
        0x12 | 0xA4 | 0xA5 => "Alt",
        0x13 => "Pause",
        0x14 => "Caps Lock",
        0x1B => "Esc",
        0x20 => "Space",
        0x21 => "Page Up",
        0x22 => "Page Down",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x2C => "Print Screen",
        0x2D => "Insert",
        0x2E => "Delete",
        0x5B | 0x5C => "Win",
        0x5D => "Menu",
        0x6A => "Num *",
        0x6B => "Num +",
        0x6D => "Num -",
        0x6E => "Num .",
        0x6F => "Num /",
        0x90 => "Num Lock",
        0x91 => "Scroll Lock",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => {
            return char::from_u32(vk).map(String::from).unwrap_or_default();
        }
        0x60..=0x69 => return format!("Num {}", vk - 0x60),
        0x70..=0x87 => return format!("F{}", vk - 0x6F),
        _ => {}
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyNameTextW;
    let lparam = ((scan & 0xff) << 16) | if extended { 1 << 24 } else { 0 };
    let mut buf = [0u16; 64];
    // SAFETY: buffer is a live local.
    let n = unsafe { GetKeyNameTextW(lparam as i32, &mut buf) };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        format!("VK{vk:02X}")
    }
}

impl InputHook {
    pub fn install(tx: Sender<(Instant, InputEvent)>) -> anyhow::Result<InputHook> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let join = std::thread::Builder::new()
            .name("stinkshot-input-hook".into())
            .spawn(move || hook_thread(tx, ready_tx))
            .map_err(|e| anyhow::anyhow!("spawn input hook thread: {e}"))?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => Ok(InputHook {
                thread_id,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(e) => Err(anyhow::anyhow!("input hook thread did not start: {e}")),
        }
    }
}

fn hook_thread(tx: Sender<(Instant, InputEvent)>, ready: Sender<anyhow::Result<u32>>) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(HookState {
            tx,
            down: [false; 256],
        })
    });
    // SAFETY: hooks are installed and removed on this same thread, which pumps messages below.
    unsafe {
        let hmod = GetModuleHandleW(None).ok().map(|m| m.into());
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0);
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0);
        let (mouse, kb): (HHOOK, HHOOK) = match (mouse, kb) {
            (Ok(m), Ok(k)) => (m, k),
            (m, k) => {
                if let Ok(m) = m {
                    let _ = UnhookWindowsHookEx(m);
                }
                if let Ok(k) = k {
                    let _ = UnhookWindowsHookEx(k);
                }
                let _ = ready.send(Err(anyhow::anyhow!("SetWindowsHookExW failed")));
                return;
            }
        };
        let _ = ready.send(Ok(GetCurrentThreadId()));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(mouse);
        let _ = UnhookWindowsHookEx(kb);
    }
    STATE.with(|s| *s.borrow_mut() = None);
}

impl Drop for InputHook {
    fn drop(&mut self) {
        // SAFETY: posting WM_QUIT to our own hook thread.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(key_label(0xA2, 0, false), "Ctrl");
        assert_eq!(key_label(0xA1, 0, false), "Shift");
        assert_eq!(key_label(0x50, 0, false), "P");
        assert_eq!(key_label(0x0D, 0, false), "Enter");
        assert_eq!(key_label(0x74, 0, false), "F5");
        assert_eq!(key_label(0x31, 0, false), "1");
        assert_eq!(key_label(0x67, 0, false), "Num 7");
        assert_eq!(key_label(0x5B, 0, false), "Win");
    }
}
