//! The element trait.
//!
//! An element is visited twice per frame. `request_layout` registers it with
//! the layout engine and returns its node; `paint` receives the bounds layout
//! decided on and emits primitives. Splitting the two is what lets a parent
//! size itself from its children before anything is drawn.

use guirs_core::{Bounds, Px};

use crate::context::Cx;
use crate::layout::LayoutId;

/// Something that can be laid out and painted.
pub trait Element: 'static {
    /// Register with the layout engine. Called once per frame, top down.
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId;

    /// Emit primitives for the bounds layout produced.
    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx);
}

/// A boxed element, which is what a container holds.
pub struct AnyElement(Box<dyn Element>);

impl AnyElement {
    pub fn new<E: Element>(element: E) -> Self {
        AnyElement(Box::new(element))
    }
}

impl Element for AnyElement {
    #[inline]
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        self.0.request_layout(cx)
    }

    #[inline]
    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        self.0.paint(bounds, cx)
    }
}

impl std::fmt::Debug for AnyElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AnyElement")
    }
}

/// Anything accepted where a child element is expected.
///
/// This is implemented for the built in elements and for string types, so
/// `div().child("Save")` works without ceremony. Custom elements opt in with
/// [`crate::impl_into_element!`].
pub trait IntoElement {
    fn into_any(self) -> AnyElement;
}

impl IntoElement for AnyElement {
    #[inline]
    fn into_any(self) -> AnyElement {
        self
    }
}

/// Implement [`IntoElement`] for a type that already implements [`Element`].
///
/// A blanket implementation would be nicer, but it would collide with the
/// string implementations under Rust's coherence rules.
#[macro_export]
macro_rules! impl_into_element {
    ($($t:ty),+ $(,)?) => {
        $(
            impl $crate::IntoElement for $t {
                #[inline]
                fn into_any(self) -> $crate::AnyElement {
                    $crate::AnyElement::new(self)
                }
            }
        )+
    };
}

/// An element that draws nothing and occupies no space.
///
/// Useful as the false branch of a conditional child.
pub struct Empty;

impl Element for Empty {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        // Claim a sibling slot even though nothing is drawn. Without this, a
        // conditional child appearing or disappearing renumbers everything
        // after it, and its siblings lose their scroll positions and text
        // selections to whichever element inherits their old identity.
        cx.begin_element(
            None,
            &guirs_core::SharedString::from("empty"),
            &[],
            false,
            guirs_style::StateFlags::EMPTY,
        );
        cx.end_element();

        let hidden = guirs_style::ComputedStyle {
            display: guirs_style::Display::None,
            ..Default::default()
        };
        cx.layout.add_node(&hidden, &[])
    }

    fn paint(&mut self, _bounds: Bounds<Px>, _cx: &mut Cx) {}
}

impl_into_element!(Empty);

/// A conditional child.
///
/// `when(is_open, || panel())` reads better than building an `Option` and
/// unwrapping it into an `Empty`.
pub fn when<E: IntoElement>(condition: bool, build: impl FnOnce() -> E) -> AnyElement {
    if condition {
        build().into_any()
    } else {
        Empty.into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Cx;
    use guirs_style::StyleEngine;
    use guirs_text::TextSystem;

    struct Counter {
        layouts: std::rc::Rc<std::cell::Cell<u32>>,
        paints: std::rc::Rc<std::cell::Cell<u32>>,
    }

    impl Element for Counter {
        fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
            self.layouts.set(self.layouts.get() + 1);
            cx.layout.add_node(&Default::default(), &[])
        }

        fn paint(&mut self, _bounds: Bounds<Px>, _cx: &mut Cx) {
            self.paints.set(self.paints.get() + 1);
        }
    }

    impl_into_element!(Counter);

    #[test]
    fn a_boxed_element_forwards_both_passes() {
        let layouts = std::rc::Rc::new(std::cell::Cell::new(0));
        let paints = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut element = Counter {
            layouts: layouts.clone(),
            paints: paints.clone(),
        }
        .into_any();

        let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
        element.request_layout(&mut cx);
        element.paint(Bounds::zero(), &mut cx);

        assert_eq!(layouts.get(), 1);
        assert_eq!(paints.get(), 1);
    }

    #[test]
    fn an_empty_element_takes_no_space() {
        let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
        let mut element = Empty.into_any();
        let id = element.request_layout(&mut cx);
        cx.layout
            .compute(id, guirs_core::Size::new(Px(100.0), Px(100.0)), &mut cx.text);
        assert_eq!(cx.layout.bounds(id).size.width, Px(0.0));
    }

    #[test]
    fn a_conditional_child_does_not_renumber_its_siblings() {
        use crate::div::div;
        use guirs_style::{StyleEngine, Styled};

        // The identity of the element after a conditional must not depend on
        // whether the conditional produced anything.
        let identity_of_last = |present: bool| {
            let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
            cx.begin_frame(
                guirs_core::Size::new(Px(100.0), Px(100.0)),
                guirs_core::ScaleFactor(1.0),
                0.0,
            );
            let mut root = div()
                .child(when(present, || div().size(5.0)))
                .child(div().id("tail").size(5.0));
            let node = root.request_layout(&mut cx);
            cx.layout
                .compute(node, guirs_core::Size::new(Px(100.0), Px(100.0)), &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);
            cx.hits.last().unwrap().id
        };

        assert_eq!(identity_of_last(true), identity_of_last(false));
    }

    #[test]
    fn when_picks_a_branch() {
        let layouts = std::rc::Rc::new(std::cell::Cell::new(0));
        let paints = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
        let mut taken = when(true, || Counter {
            layouts: layouts.clone(),
            paints: paints.clone(),
        });
        taken.request_layout(&mut cx);
        assert_eq!(layouts.get(), 1);

        let mut skipped = when(false, || Counter {
            layouts: layouts.clone(),
            paints: paints.clone(),
        });
        skipped.request_layout(&mut cx);
        assert_eq!(layouts.get(), 1, "the false branch must not build");
    }
}
