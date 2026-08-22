//! Two panes and a divider you can drag.
//!
//! Nest them and you have the layout every editor and every set of developer
//! tools is made of: a tree on the left, an editor in the middle, a panel
//! underneath, each boundary movable.
//!
//! ```
//! # use guirs_ui::{split::{split_x, SplitState}, div::div, Model};
//! let sidebar_width = Model::new(SplitState::new(240.0).range(150.0, 500.0));
//! split_x(sidebar_width, div(), div());
//! ```
//!
//! The first pane is measured in pixels and the second takes what is left.
//! That is deliberate: a sidebar should stay the width somebody dragged it to
//! when the window is resized, rather than growing in proportion and having to
//! be dragged back. Nest two splitters to get a fixed panel on each side of a
//! flexible middle.

use guirs_core::{Axis, Px, SharedString};
use guirs_style::{CursorStyle, Styled};

use crate::div::{div, Div};
use crate::element::IntoElement;
use crate::model::Model;

/// How wide or tall the first pane is, and what is happening to it.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitState {
    /// The first pane's extent along the splitter's axis, in logical pixels.
    size: f32,
    min: f32,
    max: f32,
    /// Where the pointer was when a drag began, and how big the pane was then.
    ///
    /// Both are needed: following the pointer's absolute position would jump
    /// the divider to sit under the pointer the moment it is grabbed, however
    /// far from the edge it was pressed.
    grab: Option<(f32, f32)>,
}

impl SplitState {
    /// A splitter whose first pane starts at `size` logical pixels.
    pub fn new(size: f32) -> Self {
        SplitState {
            size,
            min: 0.0,
            max: f32::INFINITY,
            grab: None,
        }
    }

    /// How small and how large the first pane may be dragged.
    ///
    /// Worth setting for anything with content: a pane dragged to nothing is
    /// easy to do by accident and looks like the pane has been lost.
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.min = min.max(0.0);
        self.max = max.max(self.min);
        self.size = self.size.clamp(self.min, self.max);
        self
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    /// Whether the divider is being dragged right now.
    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// Set the size directly, respecting the range.
    pub fn set_size(&mut self, size: f32) {
        self.size = size.clamp(self.min, self.max);
    }

    /// Note where a drag started.
    fn begin(&mut self, pointer: f32) {
        self.grab = Some((pointer, self.size));
    }

    /// Follow the pointer. Returns whether anything moved.
    fn drag_to(&mut self, pointer: f32) -> bool {
        let Some((started_at, started_size)) = self.grab else {
            return false;
        };
        let size = (started_size + (pointer - started_at)).clamp(self.min, self.max);
        if (size - self.size).abs() < 0.01 {
            return false;
        }
        self.size = size;
        true
    }

    fn end(&mut self) {
        self.grab = None;
    }
}

impl Default for SplitState {
    fn default() -> Self {
        SplitState::new(240.0)
    }
}

/// Two panes side by side, with a divider between them.
pub fn split_x(
    state: Model<SplitState>,
    first: impl IntoElement,
    second: impl IntoElement,
) -> Div {
    split(Axis::Horizontal, state, first, second)
}

/// Two panes stacked, with a divider between them.
pub fn split_y(
    state: Model<SplitState>,
    first: impl IntoElement,
    second: impl IntoElement,
) -> Div {
    split(Axis::Vertical, state, first, second)
}

/// Two panes and a divider, along whichever axis.
pub fn split(
    axis: Axis,
    state: Model<SplitState>,
    first: impl IntoElement,
    second: impl IntoElement,
) -> Div {
    let horizontal = axis == Axis::Horizontal;
    let size = Px(state.read().size());
    let dragging = state.read().is_dragging();

    let container = if horizontal {
        div().class("split").row()
    } else {
        div().class("split").col()
    };

    // A flex item will not shrink below the size of what is inside it unless
    // it is told it may. Without this the divider stops moving part way and
    // appears stuck, which reads as a bug rather than as the content pushing
    // back. The panes already hide their overflow, so shrinking clips rather
    // than spills.
    // Told not to shrink, because a flex item shrinks by default and the first
    // pane's size is the one thing here that is not up for negotiation. Left
    // alone, content in the second pane that does not fit squeezes the first
    // one instead, and the divider stops short of where it was dragged.
    let leading = if horizontal {
        div().class("split-pane").w(size).min_w(Px::ZERO).shrink(0.0).overflow_hidden()
    } else {
        div().class("split-pane").h(size).min_h(Px::ZERO).shrink(0.0).overflow_hidden()
    };

    container
        .size_full()
        .child(leading.child(first))
        .child(divider(axis, state, dragging))
        .child(
            div()
                .class("split-pane")
                .flex_1()
                .min_w(Px::ZERO)
                .min_h(Px::ZERO)
                .overflow_hidden()
                .child(second),
        )
}

fn divider(axis: Axis, state: Model<SplitState>, dragging: bool) -> Div {
    let horizontal = axis == Axis::Horizontal;
    let press = state.clone();
    let drag = state.clone();
    let release = state;

    let mut handle = div()
        .class("split-divider")
        .class_if(dragging, "dragging")
        // A divider that shrinks is a divider that cannot be grabbed. Left to
        // flex's default it is the first thing squeezed when the panes want
        // more room than there is, and it quietly disappears.
        .shrink(0.0)
        .cursor(if horizontal {
            CursorStyle::ColResize
        } else {
            CursorStyle::RowResize
        })
        // A divider is a control a reader can find and describe, even though
        // it holds nothing and says nothing of its own.
        .role(crate::access::AccessRole::Splitter)
        .label(SharedString::from(if horizontal {
            "Resize panes left and right"
        } else {
            "Resize panes up and down"
        }));

    if horizontal {
        // A hairline is what this should look like and a hairline is far too
        // thin to grab, so the element is wide enough to hit and the line
        // inside it is drawn by the stylesheet.
        handle = handle.w(Px(DIVIDER_HIT));
    } else {
        handle = handle.h(Px(DIVIDER_HIT));
    }

    handle
        .on_mouse_down(move |event, cx| {
            let along = if horizontal {
                event.position.x.0
            } else {
                event.position.y.0
            };
            press.update(|state| state.begin(along));
            cx.stop_propagation();
            cx.request_redraw();
        })
        .on_mouse_move(move |event, cx| {
            // The pointer is captured by whatever it was pressed on, so these
            // keep arriving however far from the divider it travels.
            if !cx.input.is_down(crate::event::MouseButton::Left) {
                return;
            }
            let along = if horizontal {
                event.position.x.0
            } else {
                event.position.y.0
            };
            if drag.update(|state| state.drag_to(along)) {
                cx.request_redraw();
            }
        })
        .on_mouse_up(move |_, cx| {
            if release.read().is_dragging() {
                release.update(|state| state.end());
                cx.request_redraw();
            }
        })
}

/// How wide a divider is to the pointer, in logical pixels.
///
/// Wider than it looks on purpose. A one pixel line is what a divider should
/// look like and is close to impossible to grab.
pub const DIVIDER_HIT: f32 = 6.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_moves_the_pane_by_how_far_the_pointer_moved() {
        // Not to where the pointer is. Grabbing a divider anywhere other than
        // its exact centre would otherwise snap it under the pointer.
        let mut state = SplitState::new(240.0);
        state.begin(1000.0);
        assert!(state.drag_to(1040.0));
        assert_eq!(state.size(), 280.0);

        state.end();
        // And a move with nothing grabbed does nothing at all.
        assert!(!state.drag_to(2000.0));
        assert_eq!(state.size(), 280.0);
    }

    #[test]
    fn a_pane_cannot_be_dragged_past_its_range() {
        let mut state = SplitState::new(240.0).range(150.0, 400.0);
        state.begin(500.0);

        state.drag_to(0.0);
        assert_eq!(state.size(), 150.0, "dragged below the minimum");

        state.drag_to(5000.0);
        assert_eq!(state.size(), 400.0, "dragged above the maximum");
    }

    #[test]
    fn a_range_pulls_a_size_already_outside_it_into_line() {
        let state = SplitState::new(900.0).range(100.0, 300.0);
        assert_eq!(state.size(), 300.0);
        let state = SplitState::new(10.0).range(100.0, 300.0);
        assert_eq!(state.size(), 100.0);
    }

    #[test]
    fn a_backwards_range_is_not_a_trap() {
        // Given a maximum below the minimum, the minimum wins rather than
        // clamp panicking on an inverted range.
        let state = SplitState::new(200.0).range(300.0, 100.0);
        assert_eq!(state.size(), 300.0);
    }

    #[test]
    fn dragging_back_and_forth_does_not_drift() {
        // The size is worked out from where the drag started every time
        // rather than accumulated, so a drag out and back lands where it
        // began however many moves it took.
        let mut state = SplitState::new(240.0).range(0.0, 1000.0);
        state.begin(500.0);
        for x in [520.0, 560.0, 610.0, 480.0, 500.0] {
            state.drag_to(x);
        }
        assert_eq!(state.size(), 240.0);
    }

    #[test]
    fn a_drag_that_ends_stays_where_it_was_left() {
        let mut state = SplitState::new(240.0);
        state.begin(100.0);
        state.drag_to(150.0);
        state.end();
        assert_eq!(state.size(), 290.0);
        assert!(!state.is_dragging());
    }

    #[test]
    fn a_size_set_directly_still_respects_the_range() {
        let mut state = SplitState::new(240.0).range(100.0, 300.0);
        state.set_size(5000.0);
        assert_eq!(state.size(), 300.0);
    }
}
