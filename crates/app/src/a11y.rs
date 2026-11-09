//! Screen reader support: UI Automation for every surface window via AccessKit.
//!
//! eframe only gives its root viewport an AccessKit adapter, and Stinkshot's root is an
//! invisible 1x1 window. Every real window is an immediate viewport, so this module wires
//! one `accesskit_windows::Adapter` per surface HWND itself:
//!
//! - `attach` subclasses the window (`SetWindowSubclass`) to answer `WM_GETOBJECT` and track
//!   window focus. The first UIA query turns on egui's AccessKit tree generation for the
//!   whole context (`Context::enable_accesskit`); until then it costs nothing per frame.
//! - A `Plugin` sees each pass's input and output. It feeds queued screen reader actions
//!   (click, focus, set value) into the matching viewport's input and hands that viewport's
//!   `TreeUpdate` to its adapter.
//!
//! It also reads the system settings the theme follows (`prefs`): high-contrast colors and
//! the Text size slider.
//!
//! Platform code: the Win32 half is `cfg(windows)`; elsewhere `attach` is a no-op and
//! `prefs` returns the defaults.

use std::sync::{Arc, Mutex};

use egui::accesskit::ActionRequest;
use egui::{FullOutput, RawInput, ViewportId};

type Queue = Arc<Mutex<Vec<(ViewportId, ActionRequest)>>>;

/// Install the input/output plugin on the shared context. Call once.
pub fn install(ctx: &egui::Context) {
    ctx.add_plugin(Bridge::default());
}

/// Live adapter for one surface window; dropping it removes the subclass.
pub struct Attached {
    #[cfg(windows)]
    _inner: win::Attached,
}

/// Give the surface window `hwnd` (shown as viewport `viewport`) a UI Automation provider.
/// Call before the window is focused for best results; later still works.
pub fn attach(ctx: &egui::Context, viewport: ViewportId, hwnd: isize) -> Option<Attached> {
    #[cfg(windows)]
    {
        let queue = ctx.plugin::<Bridge>().lock().queue.clone();
        Some(Attached {
            _inner: win::attach(ctx.clone(), viewport, hwnd, queue)?,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (ctx, viewport, hwnd);
        None
    }
}

/// Windows high-contrast system colors (RGB), when a contrast theme is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighContrast {
    pub window: [u8; 3],
    pub text: [u8; 3],
    pub button: [u8; 3],
    pub button_text: [u8; 3],
    pub highlight: [u8; 3],
    pub highlight_text: [u8; 3],
    pub disabled: [u8; 3],
    pub link: [u8; 3],
}

/// The system accessibility settings the theme follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub high_contrast: Option<HighContrast>,
    /// Settings > Accessibility > Text size, 1.0 to 2.25.
    pub text_scale: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            high_contrast: None,
            text_scale: 1.0,
        }
    }
}

/// Current system accessibility settings, re-read at most once a second (the shell asks
/// every frame).
pub fn prefs() -> Prefs {
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    thread_local! {
        static CACHE: Cell<Option<(Instant, Prefs)>> = const { Cell::new(None) };
    }
    CACHE.with(|c| {
        if let Some((at, p)) = c.get()
            && at.elapsed() < Duration::from_secs(1)
        {
            return p;
        }
        let p = read_prefs();
        c.set(Some((Instant::now(), p)));
        p
    })
}

#[cfg(windows)]
fn read_prefs() -> Prefs {
    Prefs {
        high_contrast: win::high_contrast(),
        text_scale: win::text_scale(),
    }
}

#[cfg(not(windows))]
fn read_prefs() -> Prefs {
    Prefs::default()
}

/// Name a hand-painted control (`ui.interact` / `allocate_exact_size`) for screen readers:
/// its role, accessible name and, for toggles, tabs and radio items, whether it is on.
/// Costs nothing while no screen reader is listening.
pub fn describe(
    resp: &egui::Response,
    role: egui::accesskit::Role,
    name: &str,
    selected: Option<bool>,
) {
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(role, resp.enabled(), name);
        info.selected = selected;
        info
    });
    focus_ring(resp);
}

/// `describe` with a name that is only built while a screen reader is listening.
pub fn describe_with(
    resp: &egui::Response,
    role: egui::accesskit::Role,
    selected: Option<bool>,
    name: impl Fn() -> String,
) {
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(role, resp.enabled(), name());
        info.selected = selected;
        info
    });
    focus_ring(resp);
}

/// What a screen reader calls a History item: its source, size and time.
pub fn item_name(m: &stinkshot_history::ItemMeta) -> String {
    let source = [m.window_title.as_str(), m.app.as_str()]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| m.path.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_default();
    crate::l10n::tr!(
        "shell.a11y.item",
        source = source,
        w = m.width,
        h = m.height,
        date = m.created.format("%b %-d %H:%M")
    )
}

/// Name a text field after its placeholder (UI Automation does not read placeholders).
pub fn name_from_placeholder(resp: &egui::Response) {
    resp.ctx.accesskit_node_builder(resp.id, |n| {
        if n.label().is_none()
            && let Some(p) = n.placeholder().map(str::to_owned)
        {
            n.set_label(p);
        }
    });
}

/// Name a stock control that has no visible label (a combo box showing only its value).
pub fn name(resp: &egui::Response, name: &str) {
    resp.ctx
        .accesskit_node_builder(resp.id, |n| n.set_label(name.to_owned()));
}

/// Set only the role of a hand-painted control that is already named (by its tooltip).
pub fn role(resp: &egui::Response, role: egui::accesskit::Role) {
    resp.ctx
        .accesskit_node_builder(resp.id, |n| n.set_role(role));
    focus_ring(resp);
}

/// The Windows 11 keyboard focus ring (as `theme::widgets` draws it) around a hand-painted
/// control that has keyboard focus, so Tab navigation is visible everywhere.
pub fn focus_ring(resp: &egui::Response) {
    if !resp.has_focus() {
        return;
    }
    let (dark, _) = crate::theme::current(&resp.ctx);
    resp.ctx.layer_painter(resp.layer_id).rect_stroke(
        resp.rect.expand(1.0),
        egui::CornerRadius::same(7),
        egui::Stroke::new(2.0, crate::theme::palette(dark).text),
        egui::StrokeKind::Outside,
    );
}

/// A polite live region in the current viewport: screen readers speak its text whenever it
/// changes. `text` only runs while a screen reader is listening (inside egui's lock, so it
/// must not touch the context).
pub fn live_status(ctx: &egui::Context, text: impl FnOnce() -> String) {
    use egui::accesskit::{Live, Role};
    ctx.accesskit_node_builder(egui::Id::unique("stinkshot.a11y.status"), |n| {
        n.set_role(Role::Status);
        n.set_live(Live::Polite);
        n.set_label(text());
    });
}

#[derive(Default)]
struct Bridge {
    queue: Queue,
    /// Viewports of the passes in flight: immediate viewports run nested inside the root's
    /// pass, and `output_hook` runs after the context has already left the viewport.
    stack: Vec<ViewportId>,
}

impl egui::Plugin for Bridge {
    fn debug_name(&self) -> &'static str {
        "stinkshot.a11y"
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut RawInput) {
        let vp = input.viewport_id;
        self.stack.push(vp);
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() {
            return;
        }
        // Requests for a window that closed before its next pass are never taken.
        if q.len() > 64 {
            q.clear();
            return;
        }
        let mut i = 0;
        while i < q.len() {
            if q[i].0 == vp {
                let (_, req) = q.remove(i);
                input.events.push(egui::Event::AccessKitActionRequest(req));
            } else {
                i += 1;
            }
        }
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut FullOutput) {
        let Some(vp) = self.stack.pop() else { return };
        #[cfg(windows)]
        win::publish(vp, &mut output.platform_output.accesskit_update);
        #[cfg(not(windows))]
        let _ = (vp, output);
    }
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use accesskit_windows::Adapter;
    use egui::ViewportId;
    use egui::accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, WM_GETOBJECT, WM_KILLFOCUS, WM_NCDESTROY, WM_SETFOCUS,
    };

    use super::Queue;

    const SUBCLASS_ID: usize = 0x5354_4b41; // "STKA"

    pub fn high_contrast() -> Option<super::HighContrast> {
        use windows::Win32::Graphics::Gdi::{
            COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
            COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
        };
        use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
        use windows::Win32::UI::WindowsAndMessaging::{
            SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        };
        let mut hc = HIGHCONTRASTW {
            cbSize: size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: `hc` is a live, correctly sized out struct.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                hc.cbSize,
                Some(&mut hc as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        if ok.is_err() || !hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
            return None;
        }
        // COLORREF is 0x00BBGGRR.
        let c = |i: SYS_COLOR_INDEX| {
            // SAFETY: trivial query.
            let v = unsafe { GetSysColor(i) };
            [v as u8, (v >> 8) as u8, (v >> 16) as u8]
        };
        Some(super::HighContrast {
            window: c(COLOR_WINDOW),
            text: c(COLOR_WINDOWTEXT),
            button: c(COLOR_BTNFACE),
            button_text: c(COLOR_BTNTEXT),
            highlight: c(COLOR_HIGHLIGHT),
            highlight_text: c(COLOR_HIGHLIGHTTEXT),
            disabled: c(COLOR_GRAYTEXT),
            link: c(COLOR_HOTLIGHT),
        })
    }

    /// Settings > Accessibility > Text size (`TextScaleFactor`, 100 to 225 percent).
    pub fn text_scale() -> f32 {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
        use windows::core::w;
        let mut v = 0u32;
        let mut len = size_of::<u32>() as u32;
        // SAFETY: the out pointers are live locals of the advertised size.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Accessibility"),
                w!("TextScaleFactor"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        if r.is_ok() {
            (v as f32 / 100.0).clamp(1.0, 2.25)
        } else {
            1.0
        }
    }

    struct Activation {
        ctx: egui::Context,
    }

    impl ActivationHandler for Activation {
        fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
            // The first full tree arrives with the next frame (`publish`).
            self.ctx.enable_accesskit();
            self.ctx.request_repaint_of(ViewportId::ROOT);
            None
        }
    }

    struct Actions {
        ctx: egui::Context,
        viewport: ViewportId,
        queue: Queue,
    }

    impl ActionHandler for Actions {
        fn do_action(&mut self, request: ActionRequest) {
            self.queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((self.viewport, request));
            self.ctx.request_repaint_of(ViewportId::ROOT);
        }
    }

    struct Inner {
        adapter: RefCell<Adapter>,
        activation: RefCell<Activation>,
    }

    thread_local! {
        static ADAPTERS: RefCell<HashMap<ViewportId, Rc<Inner>>> = RefCell::new(HashMap::new());
    }

    pub struct Attached {
        hwnd: HWND,
        viewport: ViewportId,
        _inner: Rc<Inner>,
    }

    impl Drop for Attached {
        fn drop(&mut self) {
            // SAFETY: removes only our own subclass; fails harmlessly if the window is gone.
            unsafe {
                let _ = RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID);
            }
            ADAPTERS.with(|a| a.borrow_mut().remove(&self.viewport));
        }
    }

    pub fn attach(
        ctx: egui::Context,
        viewport: ViewportId,
        hwnd: isize,
        queue: Queue,
    ) -> Option<Attached> {
        let hwnd = HWND(hwnd as *mut _);
        // SAFETY: trivial query.
        let focused = unsafe { GetForegroundWindow() } == hwnd;
        let actions = Actions {
            ctx: ctx.clone(),
            viewport,
            queue,
        };
        let inner = Rc::new(Inner {
            adapter: RefCell::new(Adapter::new(hwnd, focused, actions)),
            activation: RefCell::new(Activation { ctx }),
        });
        // SAFETY: `inner` outlives the subclass: `Attached` keeps it alive and removes the
        // subclass on drop, and the proc removes it itself on WM_NCDESTROY.
        let ok = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(subclass_proc),
                SUBCLASS_ID,
                Rc::as_ptr(&inner) as usize,
            )
        };
        if !ok.as_bool() {
            log::warn!("accessibility: SetWindowSubclass failed");
            return None;
        }
        ADAPTERS.with(|a| a.borrow_mut().insert(viewport, inner.clone()));
        Some(Attached {
            hwnd,
            viewport,
            _inner: inner,
        })
    }

    /// Hand `viewport`'s tree update (if any) to its adapter.
    pub fn publish(viewport: ViewportId, update: &mut Option<TreeUpdate>) {
        if update.is_none() {
            return;
        }
        let Some(inner) = ADAPTERS.with(|a| a.borrow().get(&viewport).cloned()) else {
            return;
        };
        let Ok(mut adapter) = inner.adapter.try_borrow_mut() else {
            return;
        };
        let events = adapter.update_if_active(|| update.take().expect("checked above"));
        drop(adapter);
        if let Some(events) = events {
            events.raise();
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        // SAFETY: `data` is the `Inner` registered in `attach`, alive while subclassed.
        let inner = unsafe { &*(data as *const Inner) };
        match msg {
            WM_GETOBJECT => {
                if let (Ok(mut adapter), Ok(mut act)) = (
                    inner.adapter.try_borrow_mut(),
                    inner.activation.try_borrow_mut(),
                ) && let Some(r) = adapter.handle_wm_getobject(wparam, lparam, &mut *act)
                {
                    drop(act);
                    drop(adapter);
                    return r.into();
                }
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                let events = inner
                    .adapter
                    .try_borrow_mut()
                    .ok()
                    .and_then(|mut a| a.update_window_focus_state(msg == WM_SETFOCUS));
                if let Some(events) = events {
                    events.raise();
                }
            }
            WM_NCDESTROY => {
                // SAFETY: removing our own subclass from inside its proc is allowed.
                unsafe {
                    let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                }
            }
            _ => {}
        }
        // SAFETY: forwards to the next proc in the chain.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}
