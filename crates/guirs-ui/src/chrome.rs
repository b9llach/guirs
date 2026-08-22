//! Window chrome for undecorated windows.
//!
//! `App::undecorated()` removes the platform title bar, which leaves the window
//! with no way to be moved, resized, minimized, maximized or closed. These put
//! that back, drawn by the application and therefore styleable like anything
//! else.
//!
//! The gestures themselves are handed to the platform rather than reimplemented:
//! dragging a window is the compositor's job, and doing it by hand produces
//! the lag and the snapping failures that give custom title bars a bad name.

use guirs_core::Px;
use guirs_style::{CursorStyle, Styled};

use crate::div::{div, Div};
use crate::element::IntoElement;
use crate::event::ResizeEdge;
#[cfg(not(target_os = "windows"))]
use crate::icon::icon;
use crate::icon::icons;

/// How wide the invisible grab strips along the window edges are.
const RESIZE_BORDER: f32 = 6.0;
/// Corners get a larger target, because they are harder to hit.
const RESIZE_CORNER: f32 = 14.0;

/// A draggable title bar.
///
/// Dragging it moves the window and double clicking it maximizes or restores,
/// which is what a platform title bar does and what people will try.
///
/// Matches `titlebar` in a stylesheet.
pub fn title_bar() -> Div {
    Div::new("titlebar")
        .row()
        .items_center()
        .on_mouse_down(|event, cx| {
            if event.click_count >= 2 {
                cx.toggle_maximize_window();
            } else {
                cx.start_window_drag();
            }
        })
}

/// The minimize, maximize and close buttons.
///
/// `maximized` picks between the maximize and restore icons; read it from
/// `cx.window_maximized`.
///
/// Matches `window-controls` and `window-button`, with `window-button.close`
/// for the one that usually turns red.
pub fn window_controls(maximized: bool) -> Div {
    Div::new("window-controls")
        .row()
        .items_center()
        .child(
            window_button(icons::minimize(), MINIMIZE)
                .class("minimize")
                .label("Minimize")
                .on_click(|_, cx| cx.minimize_window()),
        )
        .child(
            window_button(
                if maximized {
                    icons::restore()
                } else {
                    icons::maximize()
                },
                if maximized { RESTORE } else { MAXIMIZE },
            )
            .class("maximize")
            .label(if maximized { "Restore" } else { "Maximize" })
            // So the platform can offer its window layouts over it.
            .snap_target()
            .on_click(|_, cx| cx.toggle_maximize_window()),
        )
        .child(
            window_button(icons::close(), CLOSE)
                .class("close")
                .label("Close")
                .on_click(|_, cx| cx.close_window()),
        )
}

// The glyphs Windows itself draws in a title bar, from the icon font it ships.
// Using the system's own font means these are the real shapes at the real
// weight rather than an approximation of them, which is most of what makes a
// drawn control read as native.
#[cfg(target_os = "windows")]
const MINIMIZE: char = '\u{e921}';
#[cfg(target_os = "windows")]
const MAXIMIZE: char = '\u{e922}';
#[cfg(target_os = "windows")]
const RESTORE: char = '\u{e923}';
#[cfg(target_os = "windows")]
const CLOSE: char = '\u{e8bb}';

#[cfg(not(target_os = "windows"))]
const MINIMIZE: char = ' ';
#[cfg(not(target_os = "windows"))]
const MAXIMIZE: char = ' ';
#[cfg(not(target_os = "windows"))]
const RESTORE: char = ' ';
#[cfg(not(target_os = "windows"))]
const CLOSE: char = ' ';

/// The glyph inside one window control.
///
/// On Windows this is text from the system's own title bar font, so the shapes
/// are the ones every other window uses. Everywhere else it is a drawn icon,
/// because there is no equivalent font to borrow from.
#[cfg(target_os = "windows")]
fn control_glyph(_drawn: guirs_core::Icon, native: char) -> crate::text::Text {
    crate::text::text(native.to_string())
        .as_type("window-glyph")
        // The glyph is a picture of the button, and it comes from a private
        // use area, so reading it aloud produces nothing a person could act
        // on. The button says its own name instead. Only needed here: the
        // drawn icon used elsewhere describes nothing to begin with.
        .decorative()
        // Named inline as well as in the stylesheet, so a window gets the
        // right shapes even with no stylesheet at all.
        .font_families([
            guirs_core::SharedString::from("Segoe Fluent Icons"),
            guirs_core::SharedString::from("Segoe MDL2 Assets"),
        ])
        .text_size(guirs_core::Px(10.0))
}

#[cfg(not(target_os = "windows"))]
fn control_glyph(drawn: guirs_core::Icon, _native: char) -> crate::icon::IconElement {
    icon(drawn)
}

/// One window control.
fn window_button(glyph: guirs_core::Icon, native: char) -> Div {
    Div::new("window-button")
        .row()
        .center()
        // A floor rather than a size, so a stylesheet still decides how big
        // these are. Without it a machine missing the system icon font would
        // get controls with no area at all, and no way to close the window.
        .min_w(guirs_core::Px(30.0))
        .min_h(guirs_core::Px(24.0))
        .cursor(CursorStyle::Default)
        .child(control_glyph(glyph, native))
        // A press on a control must not also start dragging the window.
        .on_mouse_down(|_, cx| cx.stop_propagation())
}

/// Invisible grab strips along the window's edges and corners.
///
/// Add this as the last child of the window root so it sits above everything.
/// It lets presses through everywhere except the strips themselves, so the
/// interface underneath is unaffected.
pub fn resize_borders() -> Div {
    let border = Px(RESIZE_BORDER);
    let corner = Px(RESIZE_CORNER);

    div()
        .class("resize-borders")
        .absolute()
        .inset(Px(0.0))
        // The container covers the window, so it must be transparent to hit
        // testing or it would swallow every click in the application.
        .pointer_events_none()
        .child(edge(ResizeEdge::North).h(border).top(Px(0.0)).left(Px(0.0)).right(Px(0.0)))
        .child(edge(ResizeEdge::South).h(border).bottom(Px(0.0)).left(Px(0.0)).right(Px(0.0)))
        .child(edge(ResizeEdge::West).w(border).top(Px(0.0)).bottom(Px(0.0)).left(Px(0.0)))
        .child(edge(ResizeEdge::East).w(border).top(Px(0.0)).bottom(Px(0.0)).right(Px(0.0)))
        // Corners last, so they win the overlap with the edges.
        .child(edge(ResizeEdge::NorthWest).size(corner).top(Px(0.0)).left(Px(0.0)))
        .child(edge(ResizeEdge::NorthEast).size(corner).top(Px(0.0)).right(Px(0.0)))
        .child(edge(ResizeEdge::SouthWest).size(corner).bottom(Px(0.0)).left(Px(0.0)))
        .child(edge(ResizeEdge::SouthEast).size(corner).bottom(Px(0.0)).right(Px(0.0)))
}

fn edge(edge: ResizeEdge) -> Div {
    div()
        .absolute()
        .pointer_events_auto()
        .cursor(edge.cursor())
        .on_mouse_down(move |_, cx| {
            cx.start_window_resize(edge);
            cx.stop_propagation();
        })
}

/// A complete undecorated window frame.
///
/// Wraps content with a title bar carrying `title` and the window controls, and
/// adds the resize borders. The quickest way from `App::undecorated()` to a
/// window that behaves.
pub fn window_frame(
    title: impl Into<guirs_core::SharedString>,
    maximized: bool,
    content: impl IntoElement,
) -> Div {
    div()
        .class("window-frame")
        .relative()
        .size_full()
        .col()
        .child(
            title_bar()
                .child(crate::text::text(title).class("titlebar-title").whitespace_nowrap())
                .child(crate::div::flex_spacer())
                .child(window_controls(maximized)),
        )
        .child(div().flex_1().col().child(content))
        .child(resize_borders())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Cx;
    use crate::element::Element;
    use crate::input::InputRouter;
    use crate::event::{InputEvent, MouseButton, MouseEvent, WindowAction};
    use guirs_core::{Point, ScaleFactor, Size};
    use guirs_style::StyleEngine;
    use guirs_text::TextSystem;

    struct Harness {
        cx: Cx,
        router: InputRouter,
    }

    impl Harness {
        fn new(mut root: crate::AnyElement) -> Self {
            let size = Size::new(Px(400.0), Px(300.0));
            let mut cx = Cx::new(
                TextSystem::new(),
                StyleEngine::from_source("titlebar { height: 40px; }"),
            );
            cx.begin_frame(size, ScaleFactor(1.0), 0.0);
            let node = root.request_layout(&mut cx);
            cx.layout.compute(node, size, &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);
            cx.end_frame();
            Harness {
                cx,
                router: InputRouter::new(),
            }
        }

        fn press(&mut self, x: f32, y: f32, clicks: u32) {
            self.router.handle(
                &mut self.cx,
                &InputEvent::MouseDown(MouseEvent {
                    position: Point::new(Px(x), Px(y)),
                    button: Some(MouseButton::Left),
                    click_count: clicks,
                    ..Default::default()
                }),
            );
        }

        fn release(&mut self, x: f32, y: f32) {
            self.router.handle(
                &mut self.cx,
                &InputEvent::MouseUp(MouseEvent {
                    position: Point::new(Px(x), Px(y)),
                    button: Some(MouseButton::Left),
                    ..Default::default()
                }),
            );
        }

        fn actions(&mut self) -> Vec<WindowAction> {
            std::mem::take(&mut self.cx.input.window_actions)
        }
    }

    #[test]
    fn dragging_the_title_bar_moves_the_window() {
        let mut harness = Harness::new(
            div()
                .size_full()
                .child(title_bar().w_full().h(Px(40.0)))
                .into_any(),
        );
        harness.press(200.0, 20.0, 1);
        assert_eq!(harness.actions(), vec![WindowAction::StartDrag]);
    }

    #[test]
    fn double_clicking_the_title_bar_maximizes_instead() {
        let mut harness = Harness::new(
            div()
                .size_full()
                .child(title_bar().w_full().h(Px(40.0)))
                .into_any(),
        );
        harness.press(200.0, 20.0, 2);
        assert_eq!(harness.actions(), vec![WindowAction::ToggleMaximize]);
    }

    #[test]
    fn the_controls_do_their_jobs_without_dragging_the_window() {
        let mut harness = Harness::new(
            div()
                .size_full()
                .child(
                    title_bar()
                        .w_full()
                        .h(Px(40.0))
                        .child(window_controls(false).h(Px(40.0))),
                )
                .into_any(),
        );

        // The controls sit at the left here, because the title bar has no
        // spacer in this tree. Press the first one.
        harness.press(10.0, 20.0, 1);
        harness.release(10.0, 20.0);
        let actions = harness.actions();
        assert!(
            actions.contains(&WindowAction::Minimize),
            "expected a minimize, got {actions:?}"
        );
        assert!(
            !actions.contains(&WindowAction::StartDrag),
            "a control must not also drag the window: {actions:?}"
        );
    }

    #[test]
    fn the_resize_borders_grab_only_at_the_edges() {
        let mut harness = Harness::new(
            div()
                .size_full()
                .relative()
                .child(div().size_full().class("body"))
                .child(resize_borders())
                .into_any(),
        );

        // The middle of the window belongs to the application.
        harness.press(200.0, 150.0, 1);
        assert!(
            harness.actions().is_empty(),
            "the overlay must let clicks through"
        );

        // The very top edge resizes.
        harness.press(200.0, 2.0, 1);
        assert_eq!(
            harness.actions(),
            vec![WindowAction::StartResize(ResizeEdge::North)]
        );

        // A corner takes priority over the edges it overlaps.
        harness.press(3.0, 3.0, 1);
        assert_eq!(
            harness.actions(),
            vec![WindowAction::StartResize(ResizeEdge::NorthWest)]
        );
    }

    #[test]
    fn every_edge_names_a_sensible_cursor() {
        assert_eq!(ResizeEdge::North.cursor(), CursorStyle::ResizeVertical);
        assert_eq!(ResizeEdge::East.cursor(), CursorStyle::ResizeHorizontal);
        assert_eq!(ResizeEdge::NorthWest.cursor(), CursorStyle::ResizeNwSe);
        assert_eq!(ResizeEdge::NorthEast.cursor(), CursorStyle::ResizeNeSw);
    }
}
