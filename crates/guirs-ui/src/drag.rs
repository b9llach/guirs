//! Picking something up and putting it somewhere else.
//!
//! Dragging inside a window, which is a different thing from files arriving
//! from the desktop: nothing leaves the process, so what travels is a value
//! rather than a path.
//!
//! ```
//! # use guirs_ui::{div::div, drag::Dragged};
//! # let index = 3usize;
//! div()
//!     .draggable("tab", index)
//!     .on_drop("tab", |dropped: &Dragged, _cx| {
//!         if let Some(from) = dropped.value::<usize>() {
//!             let _ = from;
//!         }
//!     });
//! ```
//!
//! A drag is named. A target says which name it accepts, so a tab dropped on a
//! file tree does nothing rather than something surprising, and neither side
//! has to know what the other is.

use std::any::Any;
use std::rc::Rc;

use guirs_core::{GlobalElementId, Point, Px, SharedString};

/// How far the pointer has to move before a press becomes a drag.
///
/// Without a threshold every click on a draggable thing is also a very short
/// drag, and clicking one stops working.
pub const DRAG_THRESHOLD: f32 = 4.0;

/// Something being carried.
#[derive(Clone)]
pub struct Dragged {
    /// What kind of thing this is. A target accepts by name.
    pub kind: SharedString,
    /// The thing itself. Whatever the source put in.
    pub value: Rc<dyn Any>,
    /// Which element it was picked up from.
    pub from: GlobalElementId,
    /// Where the pointer went down.
    pub origin: Point<Px>,
    /// Where the pointer is now.
    pub position: Point<Px>,
}

impl Dragged {
    /// The value, if it is of the type asked for.
    ///
    /// A target that accepts a name it recognises will normally get `Some`,
    /// but the name and the type are two separate promises and this checks the
    /// second one rather than assuming it.
    pub fn value<T: 'static>(&self) -> Option<&T> {
        self.value.downcast_ref::<T>()
    }

    /// How far it has been carried.
    pub fn offset(&self) -> Point<Px> {
        Point::new(
            self.position.x - self.origin.x,
            self.position.y - self.origin.y,
        )
    }
}

impl std::fmt::Debug for Dragged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dragged")
            .field("kind", &self.kind)
            .field("from", &self.from)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

/// A press that might become a drag, or one that has.
#[derive(Clone, Default)]
pub enum DragState {
    /// Nothing is happening.
    #[default]
    Idle,
    /// Pressed on something draggable, but not yet far enough to mean it.
    Pending {
        kind: SharedString,
        value: Rc<dyn Any>,
        from: GlobalElementId,
        origin: Point<Px>,
    },
    /// Being carried.
    Active(Dragged),
}

impl std::fmt::Debug for DragState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DragState::Idle => f.write_str("Idle"),
            DragState::Pending { kind, .. } => write!(f, "Pending({kind})"),
            DragState::Active(dragged) => write!(f, "Active({dragged:?})"),
        }
    }
}

impl DragState {
    /// What is being carried, if anything is.
    pub fn dragged(&self) -> Option<&Dragged> {
        match self {
            DragState::Active(dragged) => Some(dragged),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, DragState::Active(_))
    }

    /// Note a press on something that can be picked up.
    pub fn press(
        &mut self,
        kind: SharedString,
        value: Rc<dyn Any>,
        from: GlobalElementId,
        origin: Point<Px>,
    ) {
        *self = DragState::Pending {
            kind,
            value,
            from,
            origin,
        };
    }

    /// Follow the pointer. Returns whether a drag is now under way.
    ///
    /// A press only becomes a drag once the pointer has travelled far enough
    /// to mean it, so that clicking something draggable still counts as a
    /// click rather than as a drag of no distance.
    pub fn moved(&mut self, position: Point<Px>) -> bool {
        match self {
            DragState::Idle => false,
            DragState::Pending {
                kind,
                value,
                from,
                origin,
            } => {
                let dx = position.x.0 - origin.x.0;
                let dy = position.y.0 - origin.y.0;
                if dx.hypot(dy) < DRAG_THRESHOLD {
                    return false;
                }
                *self = DragState::Active(Dragged {
                    kind: kind.clone(),
                    value: value.clone(),
                    from: *from,
                    origin: *origin,
                    position,
                });
                true
            }
            DragState::Active(dragged) => {
                dragged.position = position;
                true
            }
        }
    }

    /// Let go. Returns what was being carried, if anything was.
    pub fn release(&mut self) -> Option<Dragged> {
        let carried = match std::mem::take(self) {
            DragState::Active(dragged) => Some(dragged),
            _ => None,
        };
        *self = DragState::Idle;
        carried
    }

    /// Abandon whatever is happening, dropping nothing.
    pub fn cancel(&mut self) {
        *self = DragState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> Point<Px> {
        Point::new(Px(x), Px(y))
    }

    fn pressed() -> DragState {
        let mut state = DragState::default();
        state.press(
            "tab".into(),
            Rc::new(7usize),
            GlobalElementId(1),
            at(100.0, 100.0),
        );
        state
    }

    #[test]
    fn a_press_alone_is_not_a_drag() {
        // Otherwise clicking anything draggable also drags it nowhere, and
        // the click stops working.
        let state = pressed();
        assert!(!state.is_active());
        assert!(state.dragged().is_none());
    }

    #[test]
    fn a_small_movement_is_still_not_a_drag() {
        let mut state = pressed();
        assert!(!state.moved(at(102.0, 101.0)));
        assert!(!state.is_active());
    }

    #[test]
    fn moving_far_enough_picks_it_up() {
        let mut state = pressed();
        assert!(state.moved(at(120.0, 100.0)));
        let dragged = state.dragged().expect("nothing was picked up");
        assert_eq!(dragged.kind.as_ref(), "tab");
        assert_eq!(dragged.value::<usize>(), Some(&7));
        assert_eq!(dragged.offset().x, Px(20.0));
    }

    #[test]
    fn the_threshold_is_a_distance_rather_than_a_direction() {
        // Three across and three down is more than four away, even though
        // neither axis alone reaches it.
        let mut state = pressed();
        assert!(state.moved(at(103.0, 103.0)));
    }

    #[test]
    fn a_drag_follows_the_pointer_once_it_has_started() {
        let mut state = pressed();
        state.moved(at(120.0, 100.0));
        state.moved(at(300.0, 250.0));
        let dragged = state.dragged().unwrap();
        assert_eq!(dragged.position, at(300.0, 250.0));
        assert_eq!(dragged.offset(), at(200.0, 150.0));
    }

    #[test]
    fn releasing_hands_over_what_was_carried() {
        let mut state = pressed();
        state.moved(at(200.0, 100.0));
        let dropped = state.release().expect("nothing was handed over");
        assert_eq!(dropped.value::<usize>(), Some(&7));
        assert!(!state.is_active(), "the drag outlived its release");
    }

    #[test]
    fn releasing_without_dragging_hands_over_nothing() {
        // A press and release with no movement is a click. Treating it as a
        // drop would fire a drop handler on every click.
        let mut state = pressed();
        assert!(state.release().is_none());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused() {
        // The name and the type are separate promises. A target that checks
        // only the name still has to be told when the second one is broken.
        let mut state = pressed();
        state.moved(at(200.0, 100.0));
        let dragged = state.dragged().unwrap();
        assert_eq!(dragged.value::<String>(), None);
        assert_eq!(dragged.value::<usize>(), Some(&7));
    }

    #[test]
    fn cancelling_drops_nothing() {
        let mut state = pressed();
        state.moved(at(200.0, 100.0));
        state.cancel();
        assert!(!state.is_active());
        assert!(state.release().is_none());
    }
}
