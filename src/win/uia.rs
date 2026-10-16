//! UI Automation worker: one background thread that turns a window into a flat list of
//! element rectangles. The list is fetched once per window per capture (one cross-process
//! round trip with caching), so hovering afterwards is a local, allocation-free hit test.

use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Subtree,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId,
};

#[derive(Clone, Copy)]
pub struct Node {
    pub rect: RECT,
    pub kind: &'static str,
    /// Index of the nearest kept ancestor, or `NO_PARENT`.
    pub parent: u32,
}

const NO_PARENT: u32 = u32::MAX;

pub type Reply = UnboundedSender<(isize, Vec<Node>)>;

struct Request {
    hwnd: isize,
    reply: Reply,
}

static WORKER: OnceLock<Sender<Request>> = OnceLock::new();

const MAX_NODES: usize = 40_000;

pub fn request_tree(hwnd: isize, reply: Reply) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        std::thread::Builder::new()
            .name("shoter-uia".into())
            .spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let uia: Option<IUIAutomation> =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
                for req in rx {
                    let nodes = uia.as_ref().and_then(|u| tree(u, req.hwnd).ok()).unwrap_or_default();
                    let _ = req.reply.unbounded_send((req.hwnd, nodes));
                }
            })
            .expect("spawn UIA thread");
        tx
    });
    let _ = worker.send(Request { hwnd, reply });
}

unsafe fn tree(uia: &IUIAutomation, hwnd: isize) -> windows::core::Result<Vec<Node>> {
    let request = uia.CreateCacheRequest()?;
    request.AddProperty(UIA_BoundingRectanglePropertyId)?;
    request.AddProperty(UIA_ControlTypePropertyId)?;
    request.SetTreeScope(TreeScope_Subtree)?;
    // Scrolled-away content never gets cached: on long pages that's most of the tree.
    let onscreen = uia.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false))?;
    request.SetTreeFilter(&uia.CreateAndCondition(&uia.ControlViewCondition()?, &onscreen)?)?;
    let root = uia.ElementFromHandleBuildCache(HWND(hwnd as *mut _), &request)?;
    let mut out = Vec::with_capacity(256);
    walk(&root, NO_PARENT, 0, &mut out);
    out.shrink_to_fit();
    Ok(out)
}

/// Flattens the tree in document order (pre-order), each node pointing at its nearest kept ancestor.
unsafe fn walk(el: &IUIAutomationElement, parent: u32, depth: u32, out: &mut Vec<Node>) {
    if depth > 80 || out.len() >= MAX_NODES {
        return;
    }
    let mut me = parent;
    if let Ok(rect) = el.CachedBoundingRectangle() {
        if rect.right - rect.left >= 3 && rect.bottom - rect.top >= 3 {
            let kind = el.CachedControlType().map(|c| kind_name(c.0)).unwrap_or("Element");
            me = out.len() as u32;
            out.push(Node { rect, kind, parent });
        }
    }
    if let Ok(children) = el.GetCachedChildren() {
        let n = children.Length().unwrap_or(0);
        for i in 0..n {
            if let Ok(child) = children.GetElement(i) {
                walk(&child, me, depth + 1, out);
            }
        }
    }
}

/// The element stack under `p`, innermost first, counting only the front-most layer.
///
/// Later nodes in document order paint over earlier ones (a modal is appended after the page it
/// covers), so the last node containing `p` is the one actually visible there, and only its
/// ancestors are candidates; whatever sits behind it is ignored.
/// ponytail: painter's-order heuristic, the same one Chromium's approximate hit test uses. A
/// later, transparent full-size layer (e.g. a pointer-events:none toast region) would win the
/// hit; upgrade path is per-hover IAccessible::accHitTest, which asks the app itself.
pub fn stack_at(nodes: &[Node], p: POINT, mut f: impl FnMut(&Node)) {
    let hit = |n: &Node| p.x >= n.rect.left && p.x < n.rect.right && p.y >= n.rect.top && p.y < n.rect.bottom;
    let Some(mut i) = nodes.iter().rposition(hit) else { return };
    loop {
        let n = &nodes[i];
        if hit(n) {
            f(n);
        }
        if n.parent == NO_PARENT {
            return;
        }
        i = n.parent as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(l: i32, t: i32, r: i32, b: i32, parent: u32) -> Node {
        Node { rect: RECT { left: l, top: t, right: r, bottom: b }, kind: "Element", parent }
    }

    #[test]
    fn modal_hides_the_page_behind_it() {
        // document > [page > card], [dialog > button]; the card lies under the dialog.
        let nodes = [
            node(0, 0, 100, 100, NO_PARENT),
            node(0, 0, 100, 100, 0),
            node(10, 10, 40, 40, 1),
            node(20, 20, 80, 80, 0),
            node(60, 60, 70, 70, 3),
        ];
        let mut got = Vec::new();
        stack_at(&nodes, POINT { x: 30, y: 30 }, |n| got.push(n.rect.left));
        assert_eq!(got, [20, 0], "dialog then document, never the card behind");
        got.clear();
        stack_at(&nodes, POINT { x: 65, y: 65 }, |n| got.push(n.rect.left));
        assert_eq!(got, [60, 20, 0]);
    }
}

fn kind_name(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50002 => "Checkbox",
        50003 => "Combo box",
        50004 => "Input",
        50005 => "Link",
        50006 => "Image",
        50007 => "List item",
        50008 => "List",
        50009 => "Menu",
        50010 => "Menu bar",
        50011 => "Menu item",
        50012 => "Progress",
        50013 => "Radio",
        50014 => "Scrollbar",
        50015 => "Slider",
        50017 => "Status bar",
        50018 => "Tabs",
        50019 => "Tab",
        50020 => "Text",
        50021 => "Toolbar",
        50022 => "Tooltip",
        50023 => "Tree",
        50024 => "Tree item",
        50026 => "Group",
        50028 => "Grid",
        50029 => "Row",
        50030 => "Document",
        50031 => "Button",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50036 => "Table",
        50037 => "Title bar",
        _ => "Element",
    }
}
