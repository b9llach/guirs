//! guirs: an extensible, GPU accelerated GUI framework for Rust.
//!
//! ```no_run
//! use guirs::prelude::*;
//!
//! fn main() -> Result<(), AppError> {
//!     let count = Model::new(0i32);
//!
//!     App::new()
//!         .title("Counter")
//!         .stylesheet_file("assets/theme.gss")
//!         .run(move |_cx| {
//!             let increment = count.clone();
//!             div()
//!                 .class("root")
//!                 .size_full()
//!                 .col()
//!                 .center()
//!                 .gap(12.0)
//!                 .child(text(format!("{}", count.get())).class("display"))
//!                 .child(
//!                     button("Increment")
//!                         .class("primary")
//!                         .on_click(move |_, _| increment.update(|n| *n += 1)),
//!                 )
//!                 .into_any()
//!         })
//! }
//! ```
//!
//! # How it fits together
//!
//! * [`guirs_core`] holds the vocabulary: units, geometry, color, paint and
//!   transforms.
//! * [`guirs_style`] is the stylesheet language, the cascade and transitions.
//! * [`guirs_text`] loads fonts, shapes runs, and lays out plain and rich text.
//! * [`guirs_render`] turns a scene of rounded boxes and sprites into two GPU
//!   draw pipelines.
//! * [`guirs_ui`] is the element tree, layout, input, widgets and windows.
//!
//! Each layer depends only on the ones beneath it, so a renderer or a text
//! stack can be replaced without touching anything above.
//!
//! # Where to look
//!
//! * A styled paragraph is [`rich()`](fn@rich) and [`rich_text`].
//! * `translate`, `rotate` and `scale` are on [`Styled`], and animate through
//!   `transition: transform`.
//! * A long list is `scroll_view().virtual_rows(..)`, which builds only the
//!   rows on screen.
//! * A second window is `cx.open_window(..)`, a file picker is [`FileDialog`],
//!   and files from the desktop arrive through `on_file_drop`.
//! * What a window is holding is [`MemoryReport`], reachable every frame as
//!   `cx.memory`.

// ---------------------------------------------------------------------------
// Core vocabulary
// ---------------------------------------------------------------------------

pub use guirs_core::color::{rgb, rgba};
pub use guirs_core::{
    px, rems, Affine, Axis, BorderStyle, Bounds, BoxShadow, Corners, DevicePx, Edges, ElementId,
    GlobalElementId, Gradient, GradientStop, Hsla, Icon, IconStyle, Length, Paint, Point, Px,
    Rems, Rgba, ScaleFactor, SharedString, Size, Transform2D,
};

// ---------------------------------------------------------------------------
// Styling
// ---------------------------------------------------------------------------

pub use guirs_style::{
    AlignItems, AnimatableProperty, ComputedStyle, CursorStyle, Display, Easing, ElementDescriptor,
    FlexDirection, FlexWrap, FontFamily, FontStyle, FontWeight, JustifyContent, LineHeight,
    Overflow, ParseError, Position, Selector, Specificity, StateFlags, Style, StyleAnimator,
    StyleEngine, Styled, Stylesheet, TextAlign, TextDecoration, TextOverflow, Transition,
    TransitionTarget, WhiteSpace,
};

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

pub use guirs_text::{
    rich, FaceMetrics, FontId, FontSystem, LayoutOptions, RichText, RichTextBuilder, ScaledMetrics,
    SpanStyle, TextLayout, TextSpan, TextStyle, TextSystem,
};

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub use guirs_render::{GraphicsBackend, IconSprite, ImageId, Scene, SceneStats};

// ---------------------------------------------------------------------------
// The framework
// ---------------------------------------------------------------------------

pub use guirs_ui::{
    button, card, checkbox, column, div, icon, icons, resize_borders, rich_text, title_bar,
    window_controls, window_frame, flex_spacer, icon_button, label, list_item, panel,
    progress, radio, row, scroll_view, scroll_view_x, scroll_view_xy, select, separator, slider,
    spacer, stack, tab_bar, text,
    text_area, text_button, text_input, toggle, when, AnyElement, App, AppError, Cx, Div, Element, Empty,
    EventContext, Handlers, HitRegion, InputEvent, InputState, IntoElement, Key, KeyEvent,
    LayoutEngine, LayoutId, MemoryReport, Model, Modifiers, MouseButton, MouseEvent, ResizeEdge, ScrollEvent,
    ScrollState, StateStore, Text, TextInputEvent, TextInputState, Window, WindowAction,
    WindowOptions,
};
pub use guirs_ui::{
    confirm, message, ready, spawn, FileDialog, FileDropEvent, ImeEvent, MessageDialog,
    MessageLevel, NewWindow, Preedit, Task,
};
pub use guirs_ui::image::{img, Img, ImageError, ImageSource, ImageStatus, ImageStore, ObjectFit};
pub use guirs_ui::keymap::{keystroke, Binding, Keymap, KeymapState, Keystroke, Match};
pub use guirs_ui::menu::{
    context_menu, menu_bar, menu_item, menu_separator, submenu, MenuItem, MenuState,
};
pub use guirs_ui::split::{split, split_x, split_y, SplitState};
pub use guirs_ui::drag::{DragState, Dragged};
pub use guirs_ui::tree::{flatten, node, tree, TreeNode, TreeRow, TreeState};
pub use guirs_ui::table::{table, table_column, Column, SortOrder, TableState};

pub use guirs_ui::impl_into_element;

/// Everything needed to write an application, in one import.
pub mod prelude {
    pub use crate::{
        button, card, checkbox, column, div, flex_spacer, icon_button, label, list_item, panel,
        icon, icons, progress, px, radio, rems, resize_borders, rgb, rgba, rich, rich_text, row,
        scroll_view,
        scroll_view_x,
        scroll_view_xy,
        select, separator, slider, spacer, stack, tab_bar, text, text_area, text_button,
        context_menu, img, menu_bar, menu_item, menu_separator, node, split_x, split_y,
        submenu, table, table_column, text_input, tree,
        title_bar, toggle, when, window_controls, window_frame,
    };
    pub use crate::{
        AlignItems, AnimatableProperty, AnyElement, App, AppError, Bounds, BoxShadow, Corners,
        CursorStyle, Cx, Display, Div, Easing, Edges, Element, EventContext, FlexDirection, Icon,
        IconStyle,
        FontWeight, Gradient, GradientStop, InputEvent, IntoElement, JustifyContent, Key, KeyEvent,
        Length, LineHeight, Model, Modifiers, MouseButton, MouseEvent, Overflow, Paint, Point, Px,
        ResizeEdge, Rgba, RichText, ScrollEvent, SharedString, Size, SpanStyle, StateFlags, Style,
        Styled, Stylesheet, Text, TextAlign, TextInputState, TextSpan, Transform2D, Transition,
        Column, Dragged, Keymap, MenuItem, MenuState, ObjectFit, SortOrder, SplitState,
        TableState, TreeNode, TreeState, TransitionTarget, WhiteSpace, Window,
        WindowAction, WindowOptions,
    };
    pub use crate::{
        confirm, message, spawn, FileDialog, FileDropEvent, MessageDialog, MessageLevel, Task,
    };
}

/// The framework's version, for diagnostics and for an about box.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[test]
    fn the_prelude_is_enough_to_build_a_tree() {
        let count = Model::new(0i32);
        let increment = count.clone();

        let mut element = div()
            .class("root")
            .size_full()
            .col()
            .center()
            .gap(12.0)
            .bg(rgb(0x1e1e2e))
            .rounded(8.0)
            .child(text("Hello").text_color(rgb(0xffffff)))
            .child(
                button("Increment")
                    .class("primary")
                    .on_click(move |_, _| increment.update(|n| *n += 1)),
            )
            .child(checkbox("Enabled", true, |_| {}))
            .child(slider(0.5, |_| {}))
            .into_any();

        let mut cx = Cx::new(
            crate::TextSystem::new(),
            crate::StyleEngine::from_source("button.primary { background: #6c5ce7; }"),
        );
        cx.begin_frame(Size::new(px(400.0), px(300.0)), crate::ScaleFactor(1.0), 0.0);
        let node = element.request_layout(&mut cx);
        cx.layout
            .compute(node, Size::new(px(400.0), px(300.0)), &mut cx.text);
        element.paint(cx.layout.bounds(node), &mut cx);
        cx.end_frame();

        assert!(cx.scene.stats().quads > 0);
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn the_version_is_populated() {
        assert!(!crate::VERSION.is_empty());
    }
}
