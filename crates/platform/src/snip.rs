//! Replace Snipping Tool: a `WH_KEYBOARD_LL` hook that swallows exactly the two Snipping Tool
//! chords, Win+Shift+S (region snip) and Win+Shift+R (screen recording), and reports them so
//! the app can run its own region capture and recording instead.
//!
//! Rules the hook keeps (see `ChordFilter`):
//! - Only the letter key of a chord is swallowed: its first down, its auto-repeats and its
//!   matching up, whatever order the keys are released in. Win, Shift, Ctrl and Alt always
//!   pass through, so no key can be left logically stuck down.
//! - Win+Shift with Ctrl or Alt also held is not a Snipping Tool chord and passes through.
//! - Before deciding on a letter down, the tracked modifier state is checked against
//!   `GetAsyncKeyState`, so a key-up the hook never saw (lock screen, UAC prompt, a hook that
//!   was removed for a while) cannot turn a plain Shift+S into a capture.
//! - A swallowed letter whose up the hook never saw (the secure desktop took input while it
//!   was held, or the hook was removed for a while) is not stuck: a down counts as an
//!   auto-repeat only within `REPEAT_WINDOW_MS` of the previous swallowed event for that
//!   letter, otherwise it is judged as a fresh press. `GetAsyncKeyState` cannot help here,
//!   since a swallowed down never reaches the async key state.
//! - While pass-through is on (`SnipHook::set_pass_through`, used while Settings records a
//!   shortcut), no new chord is swallowed or reported; state is still tracked.
//! - Events injected by this hook (the Start-menu mask key) carry `SNIP_EXTRA_INFO` and are
//!   ignored. Other injected input (AutoHotkey, Stream Deck, tests) is handled like typing.
//! - After swallowing a chord, an unassigned key (VK 0xE8) is injected while Win is still
//!   down, so releasing Win does not open the Start menu (the AutoHotkey "menu mask" trick).
//! - The callback does no more than the state machine, the mask `SendInput` and a
//!   `PostThreadMessageW` to its own thread; the channel send happens in the message loop.
//!   Windows silently removes a low-level hook whose callback times out, so the hook thread
//!   re-installs the hook every `REFRESH_MS`.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, KillTimer, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN, WM_TIMER,
    WM_USER,
};

/// A Snipping Tool shortcut the hook took over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnipChord {
    /// Win+Shift+S: Snipping Tool region snip.
    Region,
    /// Win+Shift+R: Snipping Tool screen recording.
    Record,
}

impl SnipChord {
    pub const ALL: [SnipChord; 2] = [SnipChord::Region, SnipChord::Record];

    /// Display form of the chord, e.g. "Win+Shift+S".
    pub fn display(self) -> &'static str {
        match self {
            SnipChord::Region => "Win+Shift+S",
            SnipChord::Record => "Win+Shift+R",
        }
    }

    /// The same chord in the `global-hotkey` string format, so the app can tell when one
    /// of its own bindings is the chord the hook already handles.
    pub fn hotkey_string(self) -> &'static str {
        match self {
            SnipChord::Region => "Super+Shift+KeyS",
            SnipChord::Record => "Super+Shift+KeyR",
        }
    }

    fn vk(self) -> u32 {
        match self {
            SnipChord::Region => VK_S,
            SnipChord::Record => VK_R,
        }
    }

    fn from_vk(vk: u32) -> Option<SnipChord> {
        SnipChord::ALL.into_iter().find(|c| c.vk() == vk)
    }

    fn index(self) -> usize {
        match self {
            SnipChord::Region => 0,
            SnipChord::Record => 1,
        }
    }
}

const VK_S: u32 = 0x53;
const VK_R: u32 = 0x52;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
/// Unassigned virtual key injected to keep Win's release from opening Start.
pub const MASK_VK: u32 = 0xE8;
/// `dwExtraInfo` tag on everything this hook injects ("SNIP").
pub const SNIP_EXTRA_INFO: usize = 0x534E_4950;
/// Re-install interval: undoes a silent removal after a callback timeout.
const REFRESH_MS: u32 = 30_000;
/// Longest gap between a swallowed letter event and its next auto-repeat. The longest
/// keyboard repeat delay Windows offers is 1 s; repeats after that come every 33-400 ms.
pub const REPEAT_WINDOW_MS: u32 = 1_100;
/// Thread message carrying a chord (`wParam` = `SnipChord::index`).
const WM_SNIP_CHORD: u32 = WM_APP + 0x51;
const LLKHF_INJECTED: u32 = 0x10;

/// Modifier state, as tracked from events or read from `GetAsyncKeyState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub win: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// One keyboard event as the hook sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u32,
    pub down: bool,
    /// Event time in milliseconds (`KBDLLHOOKSTRUCT::time`, wraps around).
    pub time: u32,
}

impl KeyEvent {
    pub fn down(vk: u32) -> KeyEvent {
        KeyEvent {
            vk,
            down: true,
            time: 0,
        }
    }
    pub fn up(vk: u32) -> KeyEvent {
        KeyEvent {
            vk,
            down: false,
            time: 0,
        }
    }
    /// The same event at time `ms`.
    pub fn at(self, ms: u32) -> KeyEvent {
        KeyEvent { time: ms, ..self }
    }
}

/// What to do with one event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Stop the event here (return 1 from the hook without calling the next hook).
    pub swallow: bool,
    /// A chord completed: route it to the app.
    pub fire: Option<SnipChord>,
    /// Inject the mask key now so the pending Win release does not open Start.
    pub mask_win: bool,
}

/// The pure chord state machine behind the hook. Feed it every keyboard event in order.
#[derive(Clone, Debug, Default)]
pub struct ChordFilter {
    lwin: bool,
    rwin: bool,
    lshift: bool,
    rshift: bool,
    lctrl: bool,
    rctrl: bool,
    lalt: bool,
    ralt: bool,
    /// Letter keys whose down was swallowed and whose up has not arrived yet.
    swallowed: [bool; 2],
    /// Time of the last swallowed event for each letter (repeat detection).
    last: [u32; 2],
    /// Swallow and report no new chord (see `set_pass_through`).
    pass_through: bool,
}

impl ChordFilter {
    pub fn new() -> ChordFilter {
        ChordFilter::default()
    }

    /// Modifier state as tracked from the events seen so far.
    pub fn mods(&self) -> Mods {
        Mods {
            win: self.lwin || self.rwin,
            shift: self.lshift || self.rshift,
            ctrl: self.lctrl || self.rctrl,
            alt: self.lalt || self.ralt,
        }
    }

    /// Whether a chord letter is held after its down was swallowed.
    pub fn holding(&self) -> bool {
        self.swallowed.iter().any(|s| *s)
    }

    /// While on, no new chord is swallowed or reported: a letter down passes through even
    /// with Win+Shift held. Ups and auto-repeats of a letter already swallowed stay
    /// swallowed, so no lone half of a key press leaks out.
    pub fn set_pass_through(&mut self, on: bool) {
        self.pass_through = on;
    }

    /// Correct the tracked modifiers with the real key state. A modifier that is up for
    /// real is cleared on both sides; one that is down for real but was never seen going
    /// down is attributed to its left key.
    pub fn sync(&mut self, actual: Mods) {
        fn fix(l: &mut bool, r: &mut bool, actual: bool) {
            if !actual {
                *l = false;
                *r = false;
            } else if !*l && !*r {
                *l = true;
            }
        }
        fix(&mut self.lwin, &mut self.rwin, actual.win);
        fix(&mut self.lshift, &mut self.rshift, actual.shift);
        fix(&mut self.lctrl, &mut self.rctrl, actual.ctrl);
        fix(&mut self.lalt, &mut self.ralt, actual.alt);
    }

    /// Process one event. Modifiers are tracked and always pass through; only a chord
    /// letter can be swallowed.
    pub fn on_key(&mut self, ev: KeyEvent) -> Verdict {
        let pass = Verdict::default();
        let slot = match ev.vk {
            VK_LWIN => Some(&mut self.lwin),
            VK_RWIN => Some(&mut self.rwin),
            VK_LSHIFT | VK_SHIFT => Some(&mut self.lshift),
            VK_RSHIFT => Some(&mut self.rshift),
            VK_LCONTROL | VK_CONTROL => Some(&mut self.lctrl),
            VK_RCONTROL => Some(&mut self.rctrl),
            VK_LMENU | VK_MENU => Some(&mut self.lalt),
            VK_RMENU => Some(&mut self.ralt),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = ev.down;
            if !ev.down && matches!(ev.vk, VK_SHIFT | VK_CONTROL | VK_MENU) {
                // A generic modifier up (injected input) releases both sides.
                match ev.vk {
                    VK_SHIFT => self.rshift = false,
                    VK_CONTROL => self.rctrl = false,
                    _ => self.ralt = false,
                }
            }
            return pass;
        }
        let Some(chord) = SnipChord::from_vk(ev.vk) else {
            return pass;
        };
        let idx = chord.index();
        if !ev.down {
            // The up of a swallowed down is swallowed too, whatever the modifiers are now.
            let swallow = std::mem::replace(&mut self.swallowed[idx], false);
            return Verdict { swallow, ..pass };
        }
        if self.swallowed[idx] {
            if ev.time.wrapping_sub(self.last[idx]) <= REPEAT_WINDOW_MS {
                // Auto-repeat of a letter whose first down was swallowed.
                self.last[idx] = ev.time;
                return Verdict {
                    swallow: true,
                    ..pass
                };
            }
            // Too long since the last swallowed event to be a repeat: the up was missed.
            // Judge this down as a fresh press.
            self.swallowed[idx] = false;
        }
        if self.pass_through {
            return pass;
        }
        let m = Mods {
            win: self.lwin || self.rwin,
            shift: self.lshift || self.rshift,
            ctrl: self.lctrl || self.rctrl,
            alt: self.lalt || self.ralt,
        };
        if m.win && m.shift && !m.ctrl && !m.alt {
            self.swallowed[idx] = true;
            self.last[idx] = ev.time;
            return Verdict {
                swallow: true,
                fire: Some(chord),
                mask_win: true,
            };
        }
        pass
    }
}

// ------------------------------------------------------------------------- the real hook

struct HookCtx {
    filter: ChordFilter,
    thread_id: u32,
    pass_through: Arc<AtomicBool>,
}

thread_local! {
    // The hook procedure runs on the installing thread, so thread-local state is enough.
    static CTX: RefCell<Option<HookCtx>> = const { RefCell::new(None) };
}

fn key_down(vk: i32) -> bool {
    // SAFETY: trivial query.
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

fn async_mods() -> Mods {
    Mods {
        win: key_down(VK_LWIN as i32) || key_down(VK_RWIN as i32),
        shift: key_down(VK_SHIFT as i32),
        ctrl: key_down(VK_CONTROL as i32),
        alt: key_down(VK_MENU as i32),
    }
}

fn mask_input(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(MASK_VK as u16),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: SNIP_EXTRA_INFO,
            },
        },
    }
}

unsafe extern "system" fn snip_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a valid KBDLLHOOKSTRUCT during the call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let ours = info.flags.0 & LLKHF_INJECTED != 0 && info.dwExtraInfo == SNIP_EXTRA_INFO;
        if !ours {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let ev = KeyEvent {
                vk: info.vkCode,
                down,
                time: info.time,
            };
            let verdict = CTX.with(|c| {
                let mut c = c.borrow_mut();
                let c = c.as_mut()?;
                if down && SnipChord::from_vk(ev.vk).is_some() {
                    c.filter.sync(async_mods());
                    let through = c.pass_through.load(Ordering::Relaxed);
                    c.filter.set_pass_through(through);
                }
                Some((c.filter.on_key(ev), c.thread_id))
            });
            if let Some((v, tid)) = verdict {
                if v.mask_win {
                    // SAFETY: the INPUT array outlives the call; injection is asynchronous.
                    unsafe {
                        SendInput(
                            &[mask_input(false), mask_input(true)],
                            std::mem::size_of::<INPUT>() as i32,
                        );
                    }
                }
                if let Some(chord) = v.fire {
                    // SAFETY: posting to this hook thread's own queue.
                    let _ = unsafe {
                        PostThreadMessageW(tid, WM_SNIP_CHORD, WPARAM(chord.index()), LPARAM(0))
                    };
                }
                if v.swallow {
                    return LRESULT(1);
                }
            }
        }
    }
    // SAFETY: standard hook chaining.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The installed Snipping Tool hook. Dropping it removes the hook and joins its thread.
pub struct SnipHook {
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
    pass_through: Arc<AtomicBool>,
}

impl SnipHook {
    /// Install the hook on a dedicated thread. Every chord it swallows is sent to `tx`
    /// from that thread's message loop (never from inside the hook callback).
    pub fn install(tx: Sender<SnipChord>) -> anyhow::Result<SnipHook> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let pass_through = Arc::new(AtomicBool::new(false));
        let flag = pass_through.clone();
        let join = std::thread::Builder::new()
            .name("stinkshot-snip-hook".into())
            .spawn(move || hook_thread(tx, ready_tx, flag))
            .map_err(|e| anyhow::anyhow!("spawn Snipping Tool hook thread: {e}"))?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => Ok(SnipHook {
                thread_id,
                join: Some(join),
                pass_through,
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            // The thread unhooks and exits on its own when its ready send fails.
            Err(e) => Err(anyhow::anyhow!(
                "Snipping Tool hook thread did not start: {e}"
            )),
        }
    }
}

impl SnipHook {
    /// Let the chords through untouched (Win+Shift+S opens the Snipping Tool again) and
    /// report none, e.g. while Settings records a shortcut. The hook stays installed.
    pub fn set_pass_through(&self, on: bool) {
        self.pass_through.store(on, Ordering::Relaxed);
    }
}

fn install_hook() -> windows::core::Result<HHOOK> {
    // SAFETY: installs a hook whose procedure lives in this module for the process lifetime.
    unsafe {
        let hmod = GetModuleHandleW(None).ok().map(|m| m.into());
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(snip_proc), hmod, 0)
    }
}

fn hook_thread(
    tx: Sender<SnipChord>,
    ready: Sender<anyhow::Result<u32>>,
    pass_through: Arc<AtomicBool>,
) {
    // SAFETY: trivial query.
    let thread_id = unsafe { GetCurrentThreadId() };
    CTX.with(|c| {
        *c.borrow_mut() = Some(HookCtx {
            filter: ChordFilter::new(),
            thread_id,
            pass_through,
        })
    });
    let mut hook = match install_hook() {
        Ok(h) => h,
        Err(e) => {
            CTX.with(|c| *c.borrow_mut() = None);
            let _ = ready.send(Err(anyhow::anyhow!("SetWindowsHookExW: {e}")));
            return;
        }
    };
    // SAFETY: the hook, timer and message queue all belong to this thread.
    unsafe {
        // Create the message queue before announcing the thread id, so a WM_QUIT posted
        // right after install cannot fail with ERROR_INVALID_THREAD_ID.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        if ready.send(Ok(thread_id)).is_err() {
            let _ = UnhookWindowsHookEx(hook);
            CTX.with(|c| *c.borrow_mut() = None);
            return;
        }
        let timer = SetTimer(None, 0, REFRESH_MS, None);
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_SNIP_CHORD {
                if let Some(chord) = SnipChord::ALL.get(msg.wParam.0) {
                    let _ = tx.send(*chord);
                }
                continue;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_TIMER && msg.wParam.0 == timer {
                // Hook callbacks only run while this thread waits for messages, so no
                // event can be seen twice between installing the new hook and removing the
                // old one.
                match install_hook() {
                    Ok(fresh) => {
                        let _ = UnhookWindowsHookEx(hook);
                        hook = fresh;
                    }
                    Err(e) => log::warn!("Snipping Tool hook refresh failed: {e}"),
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if timer != 0 {
            let _ = KillTimer(None, timer);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    CTX.with(|c| *c.borrow_mut() = None);
}

impl Drop for SnipHook {
    fn drop(&mut self) {
        // SAFETY: posting WM_QUIT to our own hook thread.
        let posted =
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok();
        if let Some(j) = self.join.take() {
            if posted {
                let _ = j.join();
            } else {
                log::warn!("Snipping Tool hook thread could not be told to quit");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: u32 = 0x41;

    fn run(f: &mut ChordFilter, evs: &[KeyEvent]) -> Vec<Verdict> {
        evs.iter().map(|e| f.on_key(*e)).collect()
    }

    fn fired(vs: &[Verdict]) -> Vec<SnipChord> {
        vs.iter().filter_map(|v| v.fire).collect()
    }

    fn swallowed(evs: &[KeyEvent], vs: &[Verdict]) -> Vec<KeyEvent> {
        evs.iter()
            .zip(vs)
            .filter(|(_, v)| v.swallow)
            .map(|(e, _)| *e)
            .collect()
    }

    use KeyEvent as K;

    #[test]
    fn win_shift_s_fires_region_and_swallows_only_s() {
        let mut f = ChordFilter::new();
        let evs = [
            K::down(VK_LWIN),
            K::down(VK_LSHIFT),
            K::down(VK_S),
            K::up(VK_S),
            K::up(VK_LSHIFT),
            K::up(VK_LWIN),
        ];
        let vs = run(&mut f, &evs);
        assert_eq!(fired(&vs), vec![SnipChord::Region]);
        assert_eq!(swallowed(&evs, &vs), vec![K::down(VK_S), K::up(VK_S)]);
        assert!(vs[2].mask_win);
        assert_eq!(vs.iter().filter(|v| v.mask_win).count(), 1);
        assert!(!f.holding());
        assert_eq!(f.mods(), Mods::default());
    }

    #[test]
    fn win_shift_r_fires_record() {
        let mut f = ChordFilter::new();
        let evs = [
            K::down(VK_RWIN),
            K::down(VK_RSHIFT),
            K::down(VK_R),
            K::up(VK_R),
            K::up(VK_RSHIFT),
            K::up(VK_RWIN),
        ];
        let vs = run(&mut f, &evs);
        assert_eq!(fired(&vs), vec![SnipChord::Record]);
        assert_eq!(swallowed(&evs, &vs), vec![K::down(VK_R), K::up(VK_R)]);
    }

    #[test]
    fn modifier_press_order_does_not_matter() {
        let mut f = ChordFilter::new();
        let evs = [K::down(VK_LSHIFT), K::down(VK_LWIN), K::down(VK_S)];
        assert_eq!(fired(&run(&mut f, &evs)), vec![SnipChord::Region]);
    }

    #[test]
    fn release_order_is_safe_and_up_is_swallowed_after_modifiers_go() {
        // Win and Shift released before S: S up must still be swallowed (its down was), and
        // the modifiers' ups must pass through.
        let mut f = ChordFilter::new();
        let evs = [
            K::down(VK_LWIN),
            K::down(VK_LSHIFT),
            K::down(VK_S),
            K::up(VK_LWIN),
            K::up(VK_LSHIFT),
            K::up(VK_S),
        ];
        let vs = run(&mut f, &evs);
        assert_eq!(swallowed(&evs, &vs), vec![K::down(VK_S), K::up(VK_S)]);
        assert!(!f.holding());
        // Afterwards a plain s is typed normally.
        assert_eq!(f.on_key(K::down(VK_S)), Verdict::default());
        assert_eq!(f.on_key(K::up(VK_S)), Verdict::default());
    }

    #[test]
    fn auto_repeat_is_swallowed_and_fires_once() {
        let mut f = ChordFilter::new();
        let mut evs = vec![K::down(VK_LWIN), K::down(VK_LSHIFT)];
        evs.extend(std::iter::repeat_n(K::down(VK_S), 5));
        evs.push(K::up(VK_S));
        let vs = run(&mut f, &evs);
        assert_eq!(fired(&vs), vec![SnipChord::Region]);
        assert_eq!(swallowed(&evs, &vs).len(), 6);
    }

    #[test]
    fn auto_repeat_after_win_release_stays_swallowed() {
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_S)],
        );
        f.on_key(K::up(VK_LWIN));
        f.on_key(K::up(VK_LSHIFT));
        let v = f.on_key(K::down(VK_S));
        assert!(v.swallow && v.fire.is_none());
        assert!(f.on_key(K::up(VK_S)).swallow);
    }

    #[test]
    fn repeated_chords_while_modifiers_stay_down() {
        let mut f = ChordFilter::new();
        let evs = [
            K::down(VK_LWIN),
            K::down(VK_LSHIFT),
            K::down(VK_S),
            K::up(VK_S),
            K::down(VK_R),
            K::up(VK_R),
            K::down(VK_S),
            K::up(VK_S),
        ];
        let vs = run(&mut f, &evs);
        assert_eq!(
            fired(&vs),
            vec![SnipChord::Region, SnipChord::Record, SnipChord::Region]
        );
    }

    #[test]
    fn nothing_else_is_swallowed() {
        let combos: &[&[u32]] = &[
            &[],
            &[VK_LSHIFT],
            &[VK_LWIN],
            &[VK_LCONTROL, VK_LSHIFT],
            &[VK_LWIN, VK_LCONTROL],
            &[VK_LWIN, VK_LSHIFT, VK_LCONTROL],
            &[VK_LWIN, VK_LSHIFT, VK_LMENU],
            &[VK_LWIN, VK_RSHIFT, VK_RCONTROL],
            &[VK_LWIN, VK_LSHIFT, VK_RMENU],
        ];
        for mods in combos {
            for key in [VK_S, VK_R, KEY_A, 0x2C, 0x0D] {
                let mut f = ChordFilter::new();
                let mut evs: Vec<KeyEvent> = mods.iter().map(|m| K::down(*m)).collect();
                evs.push(K::down(key));
                evs.push(K::up(key));
                evs.extend(mods.iter().rev().map(|m| K::up(*m)));
                let vs = run(&mut f, &evs);
                let chord = matches!(mods, [VK_LWIN, VK_LSHIFT] | [VK_LWIN, VK_RSHIFT]);
                assert!(
                    swallowed(&evs, &vs).is_empty() || chord,
                    "mods {mods:x?} key {key:x}"
                );
                assert!(fired(&vs).is_empty(), "mods {mods:x?} key {key:x}");
            }
        }
        // Win+Shift with any other letter passes.
        let mut f = ChordFilter::new();
        let evs = [
            K::down(VK_LWIN),
            K::down(VK_LSHIFT),
            K::down(KEY_A),
            K::up(KEY_A),
        ];
        let vs = run(&mut f, &evs);
        assert!(swallowed(&evs, &vs).is_empty());
    }

    #[test]
    fn modifiers_are_never_swallowed() {
        let mut f = ChordFilter::new();
        for vk in [
            VK_LWIN,
            VK_RWIN,
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
        ] {
            assert_eq!(f.on_key(K::down(vk)), Verdict::default());
        }
        // Even while a chord letter is held.
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_S)],
        );
        assert!(!f.on_key(K::up(VK_LSHIFT)).swallow);
        assert!(!f.on_key(K::up(VK_LWIN)).swallow);
    }

    #[test]
    fn left_and_right_modifiers_are_tracked_separately() {
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_RWIN), K::down(VK_LSHIFT)],
        );
        // Releasing one Win key leaves the other down.
        f.on_key(K::up(VK_LWIN));
        assert!(f.on_key(K::down(VK_S)).fire.is_some());
        f.on_key(K::up(VK_S));
        f.on_key(K::up(VK_RWIN));
        assert!(f.on_key(K::down(VK_S)).fire.is_none());
    }

    #[test]
    fn generic_shift_from_injected_input_counts() {
        let mut f = ChordFilter::new();
        run(&mut f, &[K::down(VK_LWIN), K::down(VK_SHIFT)]);
        assert!(f.on_key(K::down(VK_S)).fire.is_some());
        f.on_key(K::up(VK_S));
        f.on_key(K::up(VK_SHIFT));
        assert!(!f.mods().shift);
    }

    #[test]
    fn stale_win_from_a_missed_key_up_is_corrected_by_sync() {
        // Win+L locks the session: the hook sees Win go down but never sees it come up.
        let mut f = ChordFilter::new();
        run(&mut f, &[K::down(VK_LWIN), K::down(0x4C), K::up(0x4C)]);
        assert!(f.mods().win);
        // Back at the desktop, the user types a capital S.
        f.on_key(K::down(VK_LSHIFT));
        f.sync(Mods {
            shift: true,
            ..Mods::default()
        });
        let v = f.on_key(K::down(VK_S));
        assert_eq!(v, Verdict::default(), "plain Shift+S must reach the app");
        assert!(!f.on_key(K::up(VK_S)).swallow);
    }

    #[test]
    fn sync_picks_up_modifiers_pressed_before_install() {
        let mut f = ChordFilter::new();
        f.sync(Mods {
            win: true,
            shift: true,
            ..Mods::default()
        });
        assert!(f.on_key(K::down(VK_S)).fire.is_some());
        // The sync attributed them to the left keys, whose ups clear them.
        f.on_key(K::up(VK_S));
        f.on_key(K::up(VK_LWIN));
        f.on_key(K::up(VK_LSHIFT));
        assert_eq!(f.mods(), Mods::default());
    }

    #[test]
    fn sync_keeps_the_side_that_was_seen() {
        let mut f = ChordFilter::new();
        f.on_key(K::down(VK_RWIN));
        f.sync(Mods {
            win: true,
            ..Mods::default()
        });
        f.on_key(K::up(VK_RWIN));
        assert!(!f.mods().win);
    }

    #[test]
    fn swallowed_letter_up_survives_sync() {
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_R)],
        );
        f.sync(Mods::default());
        assert!(f.on_key(K::up(VK_R)).swallow);
        assert!(!f.holding());
    }

    #[test]
    fn stray_letter_up_passes() {
        // An S up whose down happened before the hook existed is not ours to swallow.
        let mut f = ChordFilter::new();
        run(&mut f, &[K::down(VK_LWIN), K::down(VK_LSHIFT)]);
        assert!(!f.on_key(K::up(VK_S)).swallow);
    }

    #[test]
    fn missed_letter_up_does_not_leave_the_letter_stuck() {
        // Win+Shift+S, then the secure desktop takes the S up (Ctrl+Alt+Del, UAC, Win+L).
        let mut f = ChordFilter::new();
        let v = run(
            &mut f,
            &[
                K::down(VK_LWIN).at(1_000),
                K::down(VK_LSHIFT).at(1_010),
                K::down(VK_S).at(1_020),
            ],
        );
        assert_eq!(fired(&v), vec![SnipChord::Region]);
        // Repeats inside the window are still swallowed, and extend it.
        assert!(f.on_key(K::down(VK_S).at(1_600)).swallow);
        assert!(f.on_key(K::down(VK_S).at(2_650)).swallow);
        f.on_key(K::up(VK_LSHIFT).at(2_700));
        f.on_key(K::up(VK_LWIN).at(2_710));
        f.sync(Mods::default());
        // Much later a plain s is typed: it reaches the app.
        let late = 2_650 + REPEAT_WINDOW_MS + 1;
        assert_eq!(f.on_key(K::down(VK_S).at(late)), Verdict::default());
        assert!(!f.on_key(K::up(VK_S).at(late + 50)).swallow);
        // And a new Win+Shift+S fires again.
        f.on_key(K::down(VK_LWIN).at(late + 100));
        f.on_key(K::down(VK_LSHIFT).at(late + 110));
        assert_eq!(
            f.on_key(K::down(VK_S).at(late + 120)).fire,
            Some(SnipChord::Region)
        );
    }

    #[test]
    fn missed_letter_up_then_chord_fires_after_the_window() {
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_S).at(5)],
        );
        // Win+Shift still down, no S up seen: the next press 1.2 s later fires.
        let v = f.on_key(K::down(VK_S).at(5 + 1_200));
        assert_eq!(v.fire, Some(SnipChord::Region));
        assert!(v.swallow);
    }

    #[test]
    fn repeat_window_survives_tick_wraparound() {
        let mut f = ChordFilter::new();
        let t0 = u32::MAX - 100;
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_S).at(t0)],
        );
        let v = f.on_key(K::down(VK_S).at(t0.wrapping_add(500)));
        assert!(v.swallow && v.fire.is_none());
    }

    #[test]
    fn pass_through_lets_chords_reach_windows() {
        let mut f = ChordFilter::new();
        f.set_pass_through(true);
        let evs = [
            K::down(VK_LWIN),
            K::down(VK_LSHIFT),
            K::down(VK_S),
            K::up(VK_S),
            K::down(VK_R),
            K::up(VK_R),
        ];
        let vs = run(&mut f, &evs);
        assert!(fired(&vs).is_empty());
        assert!(swallowed(&evs, &vs).is_empty());
        assert!(vs.iter().all(|v| !v.mask_win));
        // Modifiers were still tracked: turning it off makes the next press a chord.
        f.set_pass_through(false);
        assert_eq!(f.on_key(K::down(VK_S)).fire, Some(SnipChord::Region));
    }

    #[test]
    fn pass_through_keeps_swallowing_a_letter_already_held() {
        let mut f = ChordFilter::new();
        run(
            &mut f,
            &[K::down(VK_LWIN), K::down(VK_LSHIFT), K::down(VK_S)],
        );
        f.set_pass_through(true);
        let v = f.on_key(K::down(VK_S));
        assert!(v.swallow && v.fire.is_none());
        assert!(f.on_key(K::up(VK_S)).swallow);
        assert!(!f.holding());
    }

    #[test]
    fn chord_strings() {
        assert_eq!(SnipChord::Region.display(), "Win+Shift+S");
        assert_eq!(SnipChord::Record.display(), "Win+Shift+R");
        for c in SnipChord::ALL {
            assert_eq!(SnipChord::ALL[c.index()], c);
            assert_eq!(SnipChord::from_vk(c.vk()), Some(c));
        }
    }
}
