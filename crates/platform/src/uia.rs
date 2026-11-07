//! UI-element snapping via UI Automation.
//!
//! Our overlay window covers the screen, so `ElementFromPoint` would hit the overlay.
//! Instead: start from `ElementFromHandle(target top-level window)` (picked from the
//! z-ordered snapshot taken before the overlay appeared) and descend with a cached
//! ControlView TreeWalker choosing the child whose BoundingRectangle contains the point,
//! up to a depth limit. UIA calls can take 10-200 ms for some apps, so `ElementSnapper`
//! runs on its own MTA worker thread: the UI posts the latest point with `request` and
//! polls `latest` each frame; stale requests are coalesced. Results are cached per window
//! for the lifetime of the snapper (screen is frozen, so the tree is too).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use stinkshot_core::{PxPoint, PxRect};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
    UIA_IsOffscreenPropertyId, UIA_NamePropertyId,
};

use crate::Hwnd;

/// Deepest element chain followed from the window root.
const MAX_DEPTH: usize = 32;
/// Cap on children inspected per node (pathological lists/grids).
const MAX_CHILDREN: usize = 600;
/// Minimum element edge reported, in px.
const MIN_EDGE: i32 = 4;

#[derive(Default)]
struct Shared {
    request: Option<(Hwnd, PxPoint)>,
    result: Option<SnapResult>,
    shutdown: bool,
}

type SharedPair = Arc<(Mutex<Shared>, Condvar)>;

pub struct ElementSnapper {
    shared: SharedPair,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapResult {
    pub point: PxPoint,
    /// Element rects from innermost to the top-level window, clipped to the window rect,
    /// deduplicated, each with at least 4x4 px. The UI shows `rects[0]` and lets the user
    /// cycle outward with the mouse wheel.
    pub rects: Vec<PxRect>,
    /// What each entry of `rects` is (same length and order). Added for the overlay's
    /// "which element is highlighted" label.
    pub labels: Vec<ElementLabel>,
}

/// UIA control type and name of a snapped element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementLabel {
    /// Readable control type ("Button", "List Item", "Window", ...).
    pub control: &'static str,
    /// The element's UIA name, possibly empty.
    pub name: String,
}

/// Readable name for a UIA control type id (`UIA_*ControlTypeId`, 50000..=50040).
pub fn control_type_name(id: i32) -> &'static str {
    const NAMES: [&str; 41] = [
        "Button",
        "Calendar",
        "Check Box",
        "Combo Box",
        "Edit",
        "Hyperlink",
        "Image",
        "List Item",
        "List",
        "Menu",
        "Menu Bar",
        "Menu Item",
        "Progress Bar",
        "Radio Button",
        "Scroll Bar",
        "Slider",
        "Spinner",
        "Status Bar",
        "Tab",
        "Tab Item",
        "Text",
        "Tool Bar",
        "Tooltip",
        "Tree",
        "Tree Item",
        "Custom",
        "Group",
        "Thumb",
        "Data Grid",
        "Data Item",
        "Document",
        "Split Button",
        "Window",
        "Pane",
        "Header",
        "Header Item",
        "Table",
        "Title Bar",
        "Separator",
        "Semantic Zoom",
        "App Bar",
    ];
    usize::try_from(id - 50000)
        .ok()
        .and_then(|i| NAMES.get(i).copied())
        .unwrap_or("Element")
}

impl ElementSnapper {
    pub fn new() -> anyhow::Result<ElementSnapper> {
        let shared: SharedPair = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("stinkshot-uia".into())
            .spawn(move || worker(worker_shared, ready_tx))
            .map_err(|e| anyhow::anyhow!("spawn UIA worker: {e}"))?;
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| anyhow::anyhow!("UIA worker did not start: {e}"))??;
        Ok(ElementSnapper { shared })
    }

    /// Ask for elements under `p` inside top-level window `hwnd`. Non-blocking.
    pub fn request(&self, hwnd: Hwnd, p: PxPoint) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut s) = lock.lock() {
            // Same target as the last answered/pending request: nothing new to compute.
            if s.request == Some((hwnd, p)) {
                return;
            }
            s.request = Some((hwnd, p));
            cv.notify_one();
        }
    }

    /// Most recent completed result, if any. Non-blocking.
    pub fn latest(&self) -> Option<SnapResult> {
        self.shared.0.lock().ok().and_then(|s| s.result.clone())
    }
}

impl Drop for ElementSnapper {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut s) = lock.lock() {
            s.shutdown = true;
            cv.notify_all();
        }
    }
}

struct Node {
    elem: IUIAutomationElement,
    rect: PxRect,
    label: ElementLabel,
    /// Child node indices, filled on first visit.
    children: Option<Vec<usize>>,
}

struct WindowTree {
    nodes: Vec<Node>,
    window_rect: PxRect,
}

struct Ctx {
    walker: IUIAutomationTreeWalker,
    cache: IUIAutomationCacheRequest,
    automation: IUIAutomation,
    trees: HashMap<Hwnd, Option<WindowTree>>,
}

fn rect_of(r: windows::Win32::Foundation::RECT) -> PxRect {
    crate::monitors::rect_from_win(&r)
}

impl Ctx {
    fn new() -> anyhow::Result<Ctx> {
        // SAFETY: COM is initialized on this thread by the caller.
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let walker = automation.ControlViewWalker()?;
            let cache = automation.CreateCacheRequest()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_IsOffscreenPropertyId)?;
            cache.AddProperty(UIA_ControlTypePropertyId)?;
            cache.AddProperty(UIA_NamePropertyId)?;
            Ok(Ctx {
                walker,
                cache,
                automation,
                trees: HashMap::new(),
            })
        }
    }

    fn tree(&mut self, hwnd: Hwnd) -> Option<&mut WindowTree> {
        if !self.trees.contains_key(&hwnd) {
            let built = self.build_tree(hwnd);
            self.trees.insert(hwnd, built);
        }
        self.trees.get_mut(&hwnd).and_then(|t| t.as_mut())
    }

    fn build_tree(&self, hwnd: Hwnd) -> Option<WindowTree> {
        let info = crate::windows_enum::info(hwnd)?;
        let window_rect = info.rect;
        // SAFETY: plain UIA calls on this thread.
        let elem = unsafe { self.automation.ElementFromHandle(HWND(hwnd as *mut _)) }
            .map_err(|e| log::debug!("UIA ElementFromHandle: {e}"))
            .ok()?;
        Some(WindowTree {
            nodes: vec![Node {
                elem,
                rect: window_rect,
                label: ElementLabel {
                    control: "Window",
                    name: info.title,
                },
                children: None,
            }],
            window_rect,
        })
    }

    /// Fill `children` for node `idx` if not yet done.
    fn expand(&self, tree: &mut WindowTree, idx: usize) {
        if tree.nodes[idx].children.is_some() {
            return;
        }
        let mut kids = Vec::new();
        let parent = tree.nodes[idx].elem.clone();
        // SAFETY: plain UIA walker calls; errors mean "no more children".
        unsafe {
            let mut cur = self
                .walker
                .GetFirstChildElementBuildCache(&parent, &self.cache)
                .ok();
            while let Some(e) = cur {
                if kids.len() >= MAX_CHILDREN {
                    break;
                }
                let offscreen = e.CachedIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                let rect = e.CachedBoundingRectangle().map(rect_of).unwrap_or_default();
                let next = self
                    .walker
                    .GetNextSiblingElementBuildCache(&e, &self.cache)
                    .ok();
                if !offscreen && rect.w > 0 && rect.h > 0 {
                    let label = ElementLabel {
                        control: e
                            .CachedControlType()
                            .map(|t| control_type_name(t.0))
                            .unwrap_or("Element"),
                        name: e.CachedName().map(|b| b.to_string()).unwrap_or_default(),
                    };
                    tree.nodes.push(Node {
                        elem: e,
                        rect,
                        label,
                        children: None,
                    });
                    kids.push(tree.nodes.len() - 1);
                }
                cur = next;
            }
        }
        tree.nodes[idx].children = Some(kids);
    }

    fn resolve(&mut self, hwnd: Hwnd, p: PxPoint) -> SnapResult {
        let mut rects: Vec<PxRect> = Vec::new();
        let mut labels: Vec<ElementLabel> = Vec::new();
        // Take the tree out so `expand` can borrow `self` immutably alongside it.
        let _ = self.tree(hwnd);
        if let Some(Some(mut tree)) = self.trees.remove(&hwnd) {
            let window_rect = tree.window_rect;
            let mut chain = vec![(window_rect, 0usize)];
            let mut cur = 0usize;
            for _ in 0..MAX_DEPTH {
                self.expand(&mut tree, cur);
                let kids = tree.nodes[cur].children.clone().unwrap_or_default();
                // Siblings later in the walk are drawn on top of earlier ones, so the last hit wins.
                let hit = kids
                    .into_iter()
                    .rev()
                    .find(|&k| tree.nodes[k].rect.contains(p));
                match hit {
                    Some(k) => {
                        chain.push((tree.nodes[k].rect, k));
                        cur = k;
                    }
                    None => break,
                }
            }
            // Innermost first; when a parent has the same rect, the innermost label wins.
            for (r, node) in chain.iter().rev() {
                if let Some(c) = r.intersect(&window_rect)
                    && c.w >= MIN_EDGE
                    && c.h >= MIN_EDGE
                    && rects.last() != Some(&c)
                {
                    rects.push(c);
                    labels.push(tree.nodes[*node].label.clone());
                }
            }
            self.trees.insert(hwnd, Some(tree));
        } else if let Some(w) = crate::windows_enum::info(hwnd) {
            rects.push(w.rect);
            labels.push(ElementLabel {
                control: "Window",
                name: w.title,
            });
        }
        SnapResult {
            point: p,
            rects,
            labels,
        }
    }
}

fn worker(shared: SharedPair, ready: std::sync::mpsc::Sender<anyhow::Result<()>>) {
    crate::util::ensure_com();
    let mut ctx = match Ctx::new() {
        Ok(c) => {
            let _ = ready.send(Ok(()));
            c
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let (lock, cv) = &*shared;
    loop {
        let job = {
            let Ok(mut s) = lock.lock() else { return };
            loop {
                if s.shutdown {
                    return;
                }
                // Take the newest request; older ones were overwritten (coalesced).
                if let Some(job) = s.request {
                    // Keep `request` set so identical re-requests are ignored.
                    break job;
                }
                s = match cv.wait(s) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
        };
        let result = ctx.resolve(job.0, job.1);
        let Ok(mut s) = lock.lock() else { return };
        s.result = Some(result);
        // Sleep until a request different from the one just served arrives.
        while !s.shutdown && s.request == Some(job) {
            s = match cv.wait(s) {
                Ok(g) => g,
                Err(_) => return,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_type_names() {
        assert_eq!(control_type_name(50000), "Button");
        assert_eq!(control_type_name(50007), "List Item");
        assert_eq!(control_type_name(50032), "Window");
        assert_eq!(control_type_name(50033), "Pane");
        assert_eq!(control_type_name(50040), "App Bar");
        assert_eq!(control_type_name(50041), "Element");
        assert_eq!(control_type_name(0), "Element");
    }
}
