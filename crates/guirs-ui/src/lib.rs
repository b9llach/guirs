//! The guirs element tree, layout, input and widgets.
//!
//! This is the layer an application actually writes against. Everything below
//! it is replaceable: styling, text and rendering each know nothing about
//! elements.
//!
//! # A frame
//!
//! The tree is rebuilt every frame and thrown away. Nothing in it owns
//! application state, which is what makes that cheap and what makes a view a
//! function of the state rather than a thing to keep in step with it:
//!
//! ```
//! # use guirs_style::Styled;
//! # use guirs_ui::{div::div, model::Model, text::text, widgets::button};
//! # let list: Model<Vec<i32>> = Model::new(Vec::new());
//! div().col().gap(8.0)
//!     .child(text(format!("{} items", list.read().len())))
//!     .child(button("Add").on_click(move |_, _| list.update(|l| l.push(0))));
//! ```
//!
//! State that has to survive the rebuild lives in a [`Model`], which the
//! closures clone a handle to. State that belongs to the framework rather than
//! the application, a scroll position or which element has focus, is keyed by
//! element identity in the [`StateStore`] and found again next frame.
//!
//! A frame runs in three passes, all driven by [`Window`]:
//!
//! 1. **Build and request layout.** [`Element::request_layout`] walks the tree,
//!    cascades a style onto each element and hands taffy a node.
//! 2. **Layout.** Taffy computes every box. Text is the one thing it cannot
//!    size on its own, so text nodes carry a measure context and call back into
//!    the text system.
//! 3. **Paint.** [`Element::paint`] walks the tree again with the boxes layout
//!    produced, pushing primitives into a [`Scene`](guirs_render::Scene) and
//!    registering hit regions and handlers as it goes.
//!
//! Painting is what registers hit testing, which is deliberate: an element can
//! only be clicked where it was actually drawn, so a scrolled out row, a
//! transformed button and a clipped popup all hit test correctly with no extra
//! bookkeeping.
//!
//! # Input
//!
//! [`InputRouter`] turns platform events into calls on handlers. It is
//! deliberately separate from [`Window`] and holds no GPU state, because
//! routing is where the subtle bugs live and it needs to be testable without a
//! window.
//!
//! An event travels the whole chain of elements under the pointer rather than
//! only the innermost, which is what makes a press on a button's label click
//! the button.
//!
//! # The rest
//!
//! * [`app`] is the winit runtime: windows, the event loop, the cursor, and
//!   stylesheet hot reloading.
//! * [`dialog`] is the platform's own file and message dialogs.
//! * [`chrome`] draws a title bar and window controls for an undecorated
//!   window.
//! * [`widgets`] is the built in set, each one a [`Div`] with its own element
//!   type name so a stylesheet can target it.

pub mod access;
pub mod access_bridge;
pub mod app;
pub mod chrome;
pub mod context;
pub mod dialog;
pub mod div;
pub mod drag;
pub mod element;
pub mod event;
pub mod icon;
pub mod image;
pub mod keymap;
pub mod menu;
pub mod split;
pub mod table;
pub mod tree;
pub mod input;
pub mod layout;
pub mod model;
pub mod platform;
pub mod task;
pub mod text;
pub mod widgets;
pub mod window;

pub use access::{AccessNode, AccessRole, AccessTree};
pub use app::{App, AppError, WindowOptions};
pub use chrome::{resize_borders, title_bar, window_controls, window_frame};
pub use context::{
    Cx, DismissRegion, EventContext, Handlers, HitRegion, InputState, ScrollState, ScrollbarRegion,
    SelectableText, StateStore, TextSelection,
};
pub use dialog::{confirm, message, FileDialog, MessageDialog, MessageLevel};
pub use div::{div, flex_spacer, separator, spacer, Div};
pub use element::{when, AnyElement, Element, Empty, IntoElement};
pub use event::*;
pub use icon::{icon, icons, IconElement};
pub use input::{InputRouter, Outcome};
pub use layout::{LayoutEngine, LayoutId};
pub use model::Model;
pub use task::{ready, spawn, Task};
pub use text::{caret_position, label, rich_text, text, LinkHandler, Text};
pub use widgets::*;
pub use window::{engine_from_source, MemoryReport, RootFn, Window};
