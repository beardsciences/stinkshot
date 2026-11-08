//! Console display power state (on, off, dimmed), tracked through a power-setting callback.
//!
//! Desktop Duplication delivers no frames while the displays are off (DWM does not feed
//! duplications then, measured Nov 7 2026), which looks exactly like a static screen: a
//! cached duplication session would keep returning the picture from before the displays went
//! off. `capture` asks this module whether the displays are off and whether the state changed
//! since an output's picture was last validated.
//!
//! The state comes from `PowerSettingRegisterNotification(GUID_CONSOLE_DISPLAY_STATE)` with a
//! callback (no window or message loop needed); Windows reports the current state right after
//! registering and then every change. Until the first report, or if registering fails, the
//! state is `Unknown` and treated as on (the behavior before this module existed).

use std::sync::Once;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use windows::Win32::Foundation::{HANDLE, WIN32_ERROR};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, POWERBROADCAST_SETTING, PowerSettingRegisterNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PBT_POWERSETTINGCHANGE, REGISTER_NOTIFICATION_FLAGS,
};
use windows::core::GUID;

/// GUID_CONSOLE_DISPLAY_STATE: 0 off, 1 on, 2 dimmed.
const CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);
/// `DEVICE_NOTIFY_CALLBACK`: the recipient is a `DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS`.
const DEVICE_NOTIFY_CALLBACK: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayPower {
    /// Not reported (yet), or the notification could not be registered.
    Unknown,
    Off,
    On,
    Dimmed,
}

impl DisplayPower {
    fn from_raw(v: i32) -> Self {
        match v {
            0 => DisplayPower::Off,
            1 => DisplayPower::On,
            2 => DisplayPower::Dimmed,
            _ => DisplayPower::Unknown,
        }
    }
}

static STATE: AtomicI32 = AtomicI32::new(-1);
/// Incremented when the displays go off or come back (not on dimming: a dimmed display still
/// gets every frame, so a picture validated before dimming is still good).
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// Called (on a new thread) when the displays come back on after being off.
static ON_WAKE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
static START: Once = Once::new();

/// Apply one reported raw state. Returns true when the displays came back on from off.
fn apply(raw: i32) -> bool {
    let old = STATE.swap(raw, Ordering::SeqCst);
    if old >= 0 && (old == 0) != (raw == 0) {
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    old == 0 && raw != 0
}

unsafe extern "system" fn callback(
    _context: *const core::ffi::c_void,
    kind: u32,
    setting: *const core::ffi::c_void,
) -> u32 {
    if kind != PBT_POWERSETTINGCHANGE || setting.is_null() {
        return 0;
    }
    // SAFETY: for PBT_POWERSETTINGCHANGE `setting` points to a POWERBROADCAST_SETTING whose
    // data holds at least `DataLength` bytes (a DWORD for this setting).
    let s = unsafe { &*(setting as *const POWERBROADCAST_SETTING) };
    if s.PowerSetting != CONSOLE_DISPLAY_STATE || s.DataLength < 1 {
        return 0;
    }
    let raw = s.Data[0] as i32;
    log::debug!("console display state: {:?}", DisplayPower::from_raw(raw));
    if apply(raw)
        && let Some(f) = ON_WAKE.get().copied()
    {
        let _ = std::thread::Builder::new()
            .name("display-wake".into())
            .spawn(f);
    }
    0
}

/// Start tracking (idempotent, cheap after the first call). `on_wake` runs on a new thread
/// each time the displays come back on after being off; only the first caller's is kept.
pub(crate) fn watch(on_wake: fn()) {
    let _ = ON_WAKE.set(on_wake);
    START.call_once(|| {
        // Leaked on purpose: the registration lives as long as the process.
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(callback),
            Context: std::ptr::null_mut(),
        }));
        let mut handle: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `params` lives forever; `handle` is a live out pointer.
        let err = unsafe {
            PowerSettingRegisterNotification(
                &CONSOLE_DISPLAY_STATE,
                REGISTER_NOTIFICATION_FLAGS(DEVICE_NOTIFY_CALLBACK),
                HANDLE(params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut _),
                &mut handle,
            )
        };
        if err != WIN32_ERROR(0) {
            log::debug!("display power notifications unavailable: {err:?}");
        }
    });
}

/// Current console display state.
pub fn state() -> DisplayPower {
    match OVERRIDE.load(Ordering::SeqCst) {
        -2 => DisplayPower::from_raw(STATE.load(Ordering::SeqCst)),
        v => DisplayPower::from_raw(v),
    }
}

/// -2: no override.
static OVERRIDE: AtomicI32 = AtomicI32::new(-2);

/// Diagnostics only: report `state` instead of the real one (None restores it). Lets a probe
/// time the duplication path while the displays are idle-off.
#[doc(hidden)]
pub fn override_state(state: Option<DisplayPower>) {
    let v = match state {
        None => -2,
        Some(DisplayPower::Unknown) => -1,
        Some(DisplayPower::Off) => 0,
        Some(DisplayPower::On) => 1,
        Some(DisplayPower::Dimmed) => 2,
    };
    OVERRIDE.store(v, Ordering::SeqCst);
}

/// Whether the displays are known to be off (Desktop Duplication is silent).
pub fn is_off() -> bool {
    state() == DisplayPower::Off
}

/// Transitions into or out of Off seen so far; a picture validated under another epoch may
/// predate a display-off period.
pub(crate) fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_bump_the_epoch_and_report_wake() {
        // One test owns the statics (tests in this module run in one process).
        STATE.store(-1, Ordering::SeqCst);
        let e0 = epoch();
        assert!(!apply(1), "first report is not a wake");
        assert_eq!(epoch(), e0, "first report sets the state without a change");
        assert_eq!(state(), DisplayPower::On);
        assert!(!apply(1));
        assert_eq!(epoch(), e0, "same state again is not a change");
        assert!(!apply(0));
        assert!(is_off());
        assert_eq!(epoch(), e0 + 1);
        assert!(apply(1), "off -> on is a wake");
        assert_eq!(epoch(), e0 + 2);
        assert!(!apply(2), "on -> dimmed is not a wake");
        assert_eq!(state(), DisplayPower::Dimmed);
        assert!(!is_off());
        assert_eq!(epoch(), e0 + 2, "dimming does not invalidate pictures");
        assert!(!apply(1));
        assert_eq!(epoch(), e0 + 2, "dimmed -> on neither");
        assert!(!apply(2));
        assert!(!apply(0));
        assert_eq!(epoch(), e0 + 3, "dimmed -> off does");
        assert!(apply(2), "off -> dimmed is a wake");
        assert_eq!(epoch(), e0 + 4);
        STATE.store(-1, Ordering::SeqCst);
    }
}
