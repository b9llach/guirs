//! Windows caption behaviour for a window that draws its own controls.
//!
//! Windows 11 shows a layout picker when the pointer rests on a maximize
//! button. It decides whether the pointer is on one by sending the window
//! `WM_NCHITTEST` and looking for `HTMAXBUTTON` in the reply. A window using
//! the platform's own title bar gets that for free; a window drawing its own
//! controls, which is what an application wanting its own colours has to do,
//! loses it and has no way to ask for it back through winit.
//!
//! So the window procedure is subclassed and that one question answered.
//!
//! Claiming a rectangle has a consequence: Windows then treats it as part of
//! the frame rather than the client area, so the ordinary mouse messages stop
//! arriving over it and the button would go dead and stop lighting up. Both
//! are handed back to the interface here, as the non client messages that
//! replace them.

use std::cell::RefCell;
use std::rc::Rc;

use guirs_core::{Bounds, Px, ScaleFactor};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, HTCLIENT, HTMAXBUTTON, WM_NCHITTEST,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WS_MAXIMIZEBOX,
};

/// Distinguishes this subclass from any other on the same window.
const SUBCLASS_ID: usize = 0x6755_1253;

/// What the subclass saw, for the runtime to act on.
///
/// The window procedure runs on the event loop's own thread, in the middle of
/// dispatching a message, with no way to reach the element tree. So it records
/// what happened and the runtime picks it up on its next turn, a moment later.
#[derive(Default)]
pub struct CaptionState {
    /// Where the maximize control is, in physical pixels relative to the
    /// window's client area. `None` means the window has no such control and
    /// nothing should be claimed.
    target: Option<[i32; 4]>,
    /// The pointer moved over the claimed rectangle, at this client position
    /// in physical pixels.
    pub hover: Option<(i32, i32)>,
    /// The pointer left it.
    pub left: bool,
    /// It was clicked.
    pub clicked: bool,
    scale: f32,
}

impl CaptionState {
    /// Tell the window procedure where the control is now.
    pub fn set_target(&mut self, bounds: Option<Bounds<Px>>, scale: ScaleFactor) {
        self.scale = scale.0.max(0.01);
        self.target = bounds.map(|b| {
            [
                (b.origin.x.0 * self.scale).round() as i32,
                (b.origin.y.0 * self.scale).round() as i32,
                ((b.origin.x.0 + b.size.width.0) * self.scale).round() as i32,
                ((b.origin.y.0 + b.size.height.0) * self.scale).round() as i32,
            ]
        });
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        match self.target {
            Some([left, top, right, bottom]) => {
                x >= left && x < right && y >= top && y < bottom
            }
            None => false,
        }
    }

    /// Convert a client position in physical pixels back to logical ones.
    pub fn to_logical(&self, x: i32, y: i32) -> (Px, Px) {
        (Px(x as f32 / self.scale), Px(y as f32 / self.scale))
    }

    /// Take what happened since the last time anyone asked.
    pub fn take(&mut self) -> (Option<(i32, i32)>, bool, bool) {
        (
            self.hover.take(),
            std::mem::take(&mut self.left),
            std::mem::take(&mut self.clicked),
        )
    }
}

/// A subclass installed on one window, removed when this is dropped.
pub struct Caption {
    hwnd: HWND,
    state: Rc<RefCell<CaptionState>>,
}

impl Caption {
    pub fn state(&self) -> &Rc<RefCell<CaptionState>> {
        &self.state
    }
}

impl Drop for Caption {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID);
            // The pointer handed to the subclass is now dangling, so it is
            // taken back and dropped here rather than leaked.
            drop(Rc::from_raw(Rc::as_ptr(&self.state)));
        }
    }
}

/// Start answering the platform's questions about this window's caption.
///
/// Returns `None` where the window has no usable handle, which is not worth
/// failing over: the application simply keeps the behaviour it had.
pub fn install(handle: &dyn raw_window_handle::HasWindowHandle) -> Option<Caption> {
    let raw = handle.window_handle().ok()?;
    let raw_window_handle::RawWindowHandle::Win32(win32) = raw.as_ref() else {
        return None;
    };
    let hwnd = win32.hwnd.get() as HWND;

    let state = Rc::new(RefCell::new(CaptionState::default()));
    // A second reference, owned by the subclass, released when it is removed.
    let raw_state = Rc::into_raw(state.clone()) as usize;

    unsafe {
        // Windows only offers its layouts for a window that can be maximized,
        // and an undecorated one may have been created without saying so.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style != 0 && (style as u32 & WS_MAXIMIZEBOX) == 0 {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_MAXIMIZEBOX as isize);
        }

        if SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, raw_state) == 0 {
            drop(Rc::from_raw(raw_state as *const RefCell<CaptionState>));
            return None;
        }
    }

    Some(Caption { hwnd, state })
}

/// Client coordinates, in physical pixels, of a screen position in an lparam.
unsafe fn client_point(hwnd: HWND, lparam: LPARAM) -> (i32, i32) {
    // The non client messages carry a position in screen coordinates, unlike
    // the client ones, which is a good way to spend an afternoon.
    let mut point = POINT {
        x: (lparam & 0xffff) as i16 as i32,
        y: ((lparam >> 16) & 0xffff) as i16 as i32,
    };
    unsafe { ScreenToClient(hwnd, &mut point) };
    (point.x, point.y)
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let state = unsafe { &*(data as *const RefCell<CaptionState>) };

    match message {
        WM_NCHITTEST => {
            // Let the platform answer first, so its resize borders keep
            // working, and only override where it said "ordinary content".
            let below = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if below != HTCLIENT as LRESULT {
                return below;
            }
            let (x, y) = unsafe { client_point(hwnd, lparam) };
            if state.borrow().contains(x, y) {
                return HTMAXBUTTON as LRESULT;
            }
            below
        }

        // Once the rectangle is claimed the ordinary mouse messages stop
        // arriving over it, so the hover has to come from here or the button
        // would never light up.
        WM_NCMOUSEMOVE if wparam == HTMAXBUTTON as WPARAM => {
            let (x, y) = unsafe { client_point(hwnd, lparam) };
            state.borrow_mut().hover = Some((x, y));
            0
        }

        WM_NCMOUSELEAVE => {
            state.borrow_mut().left = true;
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }

        // Swallowed rather than passed on: handing this to the platform would
        // start its own maximize gesture as well as ours.
        WM_NCLBUTTONDOWN if wparam == HTMAXBUTTON as WPARAM => 0,

        WM_NCLBUTTONUP if wparam == HTMAXBUTTON as WPARAM => {
            state.borrow_mut().clicked = true;
            0
        }

        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button_at(x: f32, width: f32, scale: f32) -> CaptionState {
        let mut state = CaptionState::default();
        state.set_target(
            Some(Bounds::from_xywh(Px(x), Px::ZERO, Px(width), Px(32.0))),
            ScaleFactor(scale),
        );
        state
    }

    // Windows asks in physical pixels and the interface answers in logical
    // ones, so the conversion is the whole of what this has to get right. A
    // scale of one hides every mistake in it, which is why these are at 1.5.

    #[test]
    fn the_claimed_rectangle_is_in_physical_pixels() {
        let state = button_at(1208.0, 46.0, 1.5);
        assert_eq!(state.target, Some([1812, 0, 1881, 48]));
    }

    #[test]
    fn a_point_inside_the_button_is_claimed_and_one_outside_is_not() {
        let state = button_at(1208.0, 46.0, 1.5);
        // The middle of the button, in physical pixels.
        assert!(state.contains(1847, 25));
        // Just past its right edge, which is the close button's business.
        assert!(!state.contains(1881, 25));
        // Just left of it, which is minimize.
        assert!(!state.contains(1811, 25));
        // Below the title bar, which is the application's own content. Getting
        // this wrong would put a layout picker over the interface.
        assert!(!state.contains(1847, 48));
    }

    #[test]
    fn the_edges_belong_to_the_button_at_the_start_and_not_at_the_end() {
        let state = button_at(100.0, 40.0, 1.0);
        assert!(state.contains(100, 0), "the first pixel is inside");
        assert!(!state.contains(140, 0), "the pixel past the end is not");
        assert!(state.contains(139, 31));
        assert!(!state.contains(139, 32));
    }

    #[test]
    fn a_window_with_no_maximize_control_claims_nothing() {
        let mut state = CaptionState::default();
        state.set_target(None, ScaleFactor(1.5));
        assert!(state.target.is_none());
        // Whatever is asked, the answer is no.
        assert!(!state.contains(0, 0));
        assert!(!state.contains(1847, 25));
    }

    #[test]
    fn what_the_window_saw_is_reported_once() {
        let mut state = button_at(100.0, 40.0, 1.0);
        state.hover = Some((110, 10));
        state.clicked = true;
        state.left = true;

        let (hover, left, clicked) = state.take();
        assert_eq!(hover, Some((110, 10)));
        assert!(left);
        assert!(clicked);

        // Taken once: a click read twice would maximize and restore again.
        let (hover, left, clicked) = state.take();
        assert!(hover.is_none());
        assert!(!left);
        assert!(!clicked);
    }

    #[test]
    fn a_physical_position_converts_back_for_the_interface() {
        let state = button_at(1208.0, 46.0, 1.5);
        let (x, y) = state.to_logical(1847, 24);
        assert!((x.0 - 1231.33).abs() < 0.1, "got {x:?}");
        assert!((y.0 - 16.0).abs() < 0.1, "got {y:?}");
    }

    #[test]
    fn a_nonsense_scale_does_not_divide_by_zero() {
        let mut state = CaptionState::default();
        state.set_target(
            Some(Bounds::from_xywh(Px::ZERO, Px::ZERO, Px(10.0), Px(10.0))),
            ScaleFactor(0.0),
        );
        let (x, y) = state.to_logical(100, 100);
        assert!(x.0.is_finite() && y.0.is_finite(), "got {x:?} {y:?}");
    }
}
