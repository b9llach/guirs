//! The per frame context.
//!
//! Everything an element needs during a frame lives in one [`Cx`]: the layout
//! tree, the style engine, the text system, the scene being built, and the
//! retained state store. Keeping it as a single struct rather than a family of
//! phase specific contexts is what keeps element signatures free of lifetime
//! parameters, which matters a great deal for a trait that users implement.
//!
//! A frame runs in two passes. `request_layout` walks the tree building style
//! and layout nodes; `paint` walks it again with resolved bounds, emitting
//! primitives and registering hit regions. Input for the next frame is
//! dispatched against what the last paint registered.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use guirs_core::{Affine, 
    Bounds, Edges, ElementId, GlobalElementId, Length, Point, Px, Rgba, ScaleFactor, SharedString,
    Size,
};
use guirs_render::Scene;
use guirs_style::{
    ComputedStyle, CursorStyle, Display, ElementDescriptor, StateFlags, Style, StyleAnimator,
    StyleEngine,
};
use guirs_text::TextSystem;
use smallvec::SmallVec;

use crate::event::{
    FileDropEvent, KeyEvent, Modifiers, MouseButton, MouseEvent, ScrollEvent, TextInputEvent,
};
use crate::layout::LayoutEngine;

/// A handler for pointer events.
pub type MouseHandler = Rc<dyn Fn(&MouseEvent, &mut EventContext)>;
/// A handler for wheel and trackpad events.
pub type ScrollHandler = Rc<dyn Fn(&ScrollEvent, &mut EventContext)>;
/// A handler for key presses.
pub type KeyHandler = Rc<dyn Fn(&KeyEvent, &mut EventContext)>;
/// A handler for composed text.
pub type TextHandler = Rc<dyn Fn(&TextInputEvent, &mut EventContext)>;
/// A handler for files dropped from the desktop.
pub type FileDropHandler = Rc<dyn Fn(&FileDropEvent, &mut EventContext)>;

/// The callbacks an element registered.
#[derive(Clone, Default)]
pub struct Handlers {
    pub on_click: Option<MouseHandler>,
    pub on_mouse_down: Option<MouseHandler>,
    pub on_mouse_up: Option<MouseHandler>,
    pub on_mouse_move: Option<MouseHandler>,
    pub on_scroll: Option<ScrollHandler>,
    pub on_key_down: Option<KeyHandler>,
    pub on_text: Option<TextHandler>,
    pub on_file_drop: Option<FileDropHandler>,
    /// Called while files are over this element, and again with an empty list
    /// when they leave, so a drop target can light up and go dark again.
    pub on_file_hover: Option<FileDropHandler>,
}

impl Handlers {
    pub fn is_empty(&self) -> bool {
        self.on_click.is_none()
            && self.on_mouse_down.is_none()
            && self.on_mouse_up.is_none()
            && self.on_mouse_move.is_none()
            && self.on_scroll.is_none()
            && self.on_key_down.is_none()
            && self.on_text.is_none()
            && self.on_file_drop.is_none()
            && self.on_file_hover.is_none()
    }
}

impl std::fmt::Debug for Handlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handlers")
            .field("click", &self.on_click.is_some())
            .field("scroll", &self.on_scroll.is_some())
            .field("keys", &self.on_key_down.is_some())
            .finish()
    }
}

/// A painted element's interactive footprint.
#[derive(Clone, Debug)]
pub struct HitRegion {
    pub id: GlobalElementId,
    /// The element that painted this one.
    ///
    /// Geometry alone cannot answer "is this click inside my dropdown", because
    /// a popup is a child that deliberately sits outside its parent's box. The
    /// paint tree can.
    pub parent: Option<GlobalElementId>,
    pub bounds: Bounds<Px>,
    /// Matches the paint layer. A popup is hit before whatever it covers.
    pub layer: u8,
    /// The clip in force when it was painted. A region is only hit where it is
    /// actually visible, which is what stops a scrolled out row from
    /// responding to clicks.
    pub clip: Option<Bounds<Px>>,
    /// The transform in force when it was painted, if any.
    ///
    /// A transform moves what is drawn without moving what was laid out, so a
    /// rotated button is only clickable where it appears. Testing that means
    /// mapping the pointer back into the element's own space.
    pub transform: Option<Affine>,
    pub cursor: CursorStyle,
    pub focusable: bool,
    pub scrollable: bool,
}

impl HitRegion {
    #[inline]
    pub fn contains(&self, point: Point<Px>) -> bool {
        // The clip is in screen space, and so is the pointer.
        if let Some(clip) = self.clip {
            if !clip.contains(point) {
                return false;
            }
        }
        match self.transform {
            None => self.bounds.contains(point),
            Some(transform) => match transform.invert() {
                Some(inverse) => self.bounds.contains(inverse.apply(point)),
                // A transform scaled to nothing draws nothing, so there is
                // nothing to hit either.
                None => false,
            },
        }
    }
}

/// A text element that can be selected with the pointer.
///
/// Registered during paint, because selecting text means turning a pointer
/// position into a byte offset, and only the laid out text can do that.
#[derive(Clone)]
pub struct SelectableText {
    /// Where the text's content box starts, in window coordinates.
    pub origin: Point<Px>,
    pub layout: std::sync::Arc<guirs_text::TextLayout>,
}

impl std::fmt::Debug for SelectableText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectableText")
            .field("origin", &self.origin)
            .field("chars", &self.layout.text.len())
            .finish()
    }
}

/// A range of selected text within one element.
///
/// Selection does not span elements. Doing that well means an ordering over the
/// whole tree and a notion of what sits between two runs, which is a larger
/// piece of work than it looks; within one block covers copying a message or a
/// code sample, which is what it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub id: GlobalElementId,
    /// Where the selection started.
    pub anchor: usize,
    /// Where the pointer is now. May be before the anchor.
    pub head: usize,
}

impl TextSelection {
    #[inline]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// An editable text field painted this frame.
///
/// Registered by the field so that the window can do the parts of editing that
/// do not belong to any one widget: turning a pointer position into a caret,
/// sweeping out a selection, and talking to the system clipboard.
#[derive(Clone)]
pub struct TextEditor {
    /// Where the field's text starts, in window coordinates.
    pub origin: Point<Px>,
    pub layout: std::sync::Arc<guirs_text::TextLayout>,
    /// The field's own state, so edits land where the application can see them.
    pub state: crate::model::Model<crate::widgets::TextInputState>,
    /// Whether newlines belong in this field.
    ///
    /// The window needs to know: a paste carrying newlines has to either keep
    /// them or flatten them, and Enter has to either insert one or be left for
    /// the application to interpret.
    pub multiline: bool,
}

impl std::fmt::Debug for TextEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEditor")
            .field("origin", &self.origin)
            .finish()
    }
}

/// A callback for a click that landed outside an element and its subtree.
pub type DismissHandler = Rc<dyn Fn(&mut EventContext)>;

/// An element that wants to know when a click lands elsewhere.
///
/// This is what closes a dropdown, a menu or a popover. Registered during
/// paint, consulted on the next press.
#[derive(Clone)]
pub struct DismissRegion {
    pub id: GlobalElementId,
    pub handler: DismissHandler,
}

impl std::fmt::Debug for DismissRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DismissRegion").field("id", &self.id).finish()
    }
}

/// A scrollbar drawn for a scrollable element.
///
/// Recorded during paint so that dispatch can hit test and drag it. The bar is
/// an overlay drawn over the content, so it has to be consulted before the
/// content underneath it.
#[derive(Clone, Copy, Debug)]
pub struct ScrollbarRegion {
    pub owner: GlobalElementId,
    pub axis: guirs_core::Axis,
    pub track: Bounds<Px>,
    pub thumb: Bounds<Px>,
}

/// Scroll position for one scrollable element.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    pub offset: Point<Px>,
    pub content: Size<Px>,
    pub viewport: Size<Px>,
}

impl ScrollState {
    /// How far the content can move on each axis before running out.
    pub fn max_offset(&self) -> Point<Px> {
        Point::new(
            (self.content.width - self.viewport.width).max(Px::ZERO),
            (self.content.height - self.viewport.height).max(Px::ZERO),
        )
    }

    /// Clamp the offset into range. Called after scrolling and after a resize,
    /// because shrinking the content can strand the viewport past the end.
    pub fn clamp(&mut self) {
        let max = self.max_offset();
        self.offset.x = self.offset.x.clamp(Px::ZERO, max.x);
        self.offset.y = self.offset.y.clamp(Px::ZERO, max.y);
    }

    #[inline]
    pub fn can_scroll_vertically(&self) -> bool {
        self.max_offset().y > Px::ZERO
    }

    #[inline]
    pub fn can_scroll_horizontally(&self) -> bool {
        self.max_offset().x > Px::ZERO
    }
}

/// Retained per element state, keyed by identity rather than tree position.
#[derive(Default)]
pub struct StateStore {
    entries: HashMap<GlobalElementId, (Box<dyn Any>, u64)>,
    frame: u64,
}

impl std::fmt::Debug for StateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateStore")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl StateStore {
    /// Remove and return the state for `id`, or a default.
    pub fn take<T: Default + 'static>(&mut self, id: GlobalElementId) -> T {
        match self.entries.remove(&id) {
            Some((value, _)) => match value.downcast::<T>() {
                Ok(value) => *value,
                // The element at this identity changed type, which means the
                // old state is meaningless. Starting fresh is the only sane
                // interpretation.
                Err(_) => T::default(),
            },
            None => T::default(),
        }
    }

    pub fn put<T: 'static>(&mut self, id: GlobalElementId, value: T) {
        self.entries.insert(id, (Box::new(value), self.frame));
    }

    pub fn peek<T: 'static>(&self, id: GlobalElementId) -> Option<&T> {
        self.entries.get(&id)?.0.downcast_ref::<T>()
    }

    pub fn peek_mut<T: 'static>(&mut self, id: GlobalElementId) -> Option<&mut T> {
        self.entries.get_mut(&id)?.0.downcast_mut::<T>()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Drop state for elements that have not been seen for a while.
    ///
    /// Not every element touches its state every frame, so eviction waits a
    /// generous number of frames rather than dropping anything missed once.
    pub fn sweep(&mut self, keep_frames: u64) {
        let frame = self.frame;
        self.entries
            .retain(|_, (_, last)| frame.saturating_sub(*last) <= keep_frames);
    }
}

/// Pointer and keyboard state carried between frames.
#[derive(Debug, Default)]
pub struct InputState {
    pub mouse_position: Point<Px>,
    pub mouse_inside: bool,
    pub buttons: SmallVec<[MouseButton; 3]>,
    pub modifiers: Modifiers,
    /// Every element under the pointer, ancestors included, which is what makes
    /// `:hover` work on containers.
    pub hovered: HashSet<GlobalElementId>,
    /// Every element the pointer is pressed on, innermost first.
    ///
    /// A press lands on the innermost element under the pointer, which for a
    /// button is its label. `button:active` still has to match, so the whole
    /// chain counts as active, exactly as hover does.
    pub active: SmallVec<[GlobalElementId; 8]>,
    pub focused: Option<GlobalElementId>,
    /// Whether focus arrived from the keyboard, which decides whether a focus
    /// ring is drawn.
    pub focus_visible: bool,
    pub window_focused: bool,
    /// Whether files from the desktop are currently over this window.
    pub hovering_files: bool,
    /// The current text selection, if any.
    pub text_selection: Option<TextSelection>,
    /// Window operations requested by handlers this pass, drained by the
    /// runtime once dispatch is finished.
    pub window_actions: Vec<crate::event::WindowAction>,
}

impl InputState {
    /// The state flags for one element, for the style cascade to match on.
    pub fn flags_for(&self, id: GlobalElementId, focus_within: bool) -> StateFlags {
        let mut flags = StateFlags::EMPTY;
        flags.set(StateFlags::HOVER, self.hovered.contains(&id));
        flags.set(StateFlags::ACTIVE, self.active.contains(&id));
        let focused = self.focused == Some(id);
        flags.set(StateFlags::FOCUS, focused);
        flags.set(StateFlags::FOCUS_VISIBLE, focused && self.focus_visible);
        flags.set(StateFlags::FOCUS_WITHIN, focused || focus_within);
        flags
    }

    #[inline]
    pub fn is_down(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button)
    }
}

/// What an event handler can do to the window.
pub struct EventContext<'a> {
    pub input: &'a mut InputState,
    pub state: &'a mut StateStore,
    redraw: &'a mut bool,
    quit: &'a mut bool,
    propagate: bool,
    target: GlobalElementId,
}

impl<'a> EventContext<'a> {
    pub fn new(
        input: &'a mut InputState,
        state: &'a mut StateStore,
        redraw: &'a mut bool,
        quit: &'a mut bool,
    ) -> Self {
        EventContext {
            input,
            state,
            redraw,
            quit,
            propagate: true,
            target: GlobalElementId::ROOT,
        }
    }

    /// Aim this context at the element whose handler is about to run.
    pub fn for_target(mut self, target: GlobalElementId) -> Self {
        self.target = target;
        self
    }

    /// The element this handler belongs to.
    #[inline]
    pub fn target(&self) -> GlobalElementId {
        self.target
    }

    /// Ask for another frame.
    pub fn request_redraw(&mut self) {
        *self.redraw = true;
    }

    /// Close the window.
    pub fn quit(&mut self) {
        *self.quit = true;
    }

    /// Ask the platform window to do something.
    pub fn window_action(&mut self, action: crate::event::WindowAction) {
        self.input.window_actions.push(action);
        *self.redraw = true;
    }

    /// Minimize the window.
    pub fn minimize_window(&mut self) {
        self.window_action(crate::event::WindowAction::Minimize);
    }

    /// Maximize the window, or restore it if it already is.
    pub fn toggle_maximize_window(&mut self) {
        self.window_action(crate::event::WindowAction::ToggleMaximize);
    }

    /// Close the window. The application exits when the last one goes.
    pub fn close_window(&mut self) {
        self.window_action(crate::event::WindowAction::Close);
    }

    /// Rename the window.
    pub fn set_window_title(&mut self, title: impl Into<String>) {
        self.window_action(crate::event::WindowAction::SetTitle(title.into()));
    }

    /// Open another window, with its own root and its own options.
    ///
    /// The new window shares this one's stylesheet and fonts, and keeps its own
    /// element state, scroll positions and focus. It is a peer rather than a
    /// child: closing this one leaves it open.
    ///
    /// ```
    /// # use guirs_ui::app::WindowOptions;
    /// # use guirs_ui::element::{AnyElement, IntoElement};
    /// # use guirs_ui::div::div;
    /// # fn example(cx: &mut guirs_ui::EventContext) {
    /// # fn preferences() -> guirs_ui::div::Div { div() }
    /// cx.open_window(WindowOptions::new("Preferences").sized(420.0, 520.0), |_| {
    ///     preferences().into_any()
    /// });
    /// # }
    /// ```
    pub fn open_window(
        &mut self,
        options: crate::app::WindowOptions,
        root: impl FnMut(&mut Cx) -> crate::element::AnyElement + 'static,
    ) {
        self.window_action(crate::event::WindowAction::Open(Box::new(
            crate::event::NewWindow::new(options, root),
        )));
    }

    /// Hand the press that is in progress to the platform as a window move.
    pub fn start_window_drag(&mut self) {
        self.window_action(crate::event::WindowAction::StartDrag);
    }

    /// Hand the press that is in progress to the platform as a window resize.
    pub fn start_window_resize(&mut self, edge: crate::event::ResizeEdge) {
        self.window_action(crate::event::WindowAction::StartResize(edge));
    }

    /// Move keyboard focus.
    pub fn focus(&mut self, id: GlobalElementId) {
        self.input.focused = Some(id);
        *self.redraw = true;
    }

    pub fn blur(&mut self) {
        self.input.focused = None;
        *self.redraw = true;
    }

    #[inline]
    pub fn is_focused(&self, id: GlobalElementId) -> bool {
        self.input.focused == Some(id)
    }

    /// Stop this event reaching enclosing elements.
    pub fn stop_propagation(&mut self) {
        self.propagate = false;
    }

    #[inline]
    pub fn propagating(&self) -> bool {
        self.propagate
    }

    #[allow(dead_code)]
    pub(crate) fn resume_propagation(&mut self) {
        self.propagate = true;
    }
}

/// Everything a frame needs.
pub struct Cx {
    pub layout: LayoutEngine,
    pub text: TextSystem,
    pub styles: StyleEngine,
    pub animator: StyleAnimator,
    pub scene: Scene,
    pub state: StateStore,
    pub input: InputState,

    /// Hit regions registered by this frame's paint pass.
    pub hits: Vec<HitRegion>,
    /// Handlers registered by this frame's paint pass.
    pub handlers: HashMap<GlobalElementId, Handlers>,
    /// Focusable elements in paint order, for tab navigation.
    pub focus_order: Vec<GlobalElementId>,
    /// Scrollbars painted this frame, so they can be grabbed.
    pub scrollbars: Vec<ScrollbarRegion>,
    /// Elements waiting to be told about a click somewhere else.
    pub dismiss: Vec<DismissRegion>,
    /// Text painted this frame that the pointer may select.
    pub selectable: HashMap<GlobalElementId, SelectableText>,
    /// Editable fields painted this frame.
    pub editors: HashMap<GlobalElementId, TextEditor>,
    /// The interface as a screen reader sees it.
    ///
    /// Built during paint, and only when something is listening: an
    /// application with nothing attached does none of the work.
    pub access: crate::access::AccessTree,

    /// Every picture this window has been asked for.
    ///
    /// On the window rather than on the elements, so the same file drawn in
    /// two places is read once and held once.
    pub images: crate::image::ImageStore,

    /// Where the focused field's caret is, for an input method's candidate
    /// window. `None` when nothing that accepts composition has focus, which
    /// is also how the runtime knows to turn the input method off.
    pub ime_area: Option<Bounds<Px>>,

    /// Where the window's own maximize control was drawn, if it has one.
    ///
    /// Windows offers a layout picker when the pointer rests on a maximize
    /// button, and it decides whether the pointer is on one by asking the
    /// window. A window drawing its own controls has to answer, and it can
    /// only answer if it knows where it put them.
    pub snap_target: Option<Bounds<Px>>,

    /// The soonest something has asked to be drawn again, in the same seconds
    /// as [`Cx::now`].
    ///
    /// Separate from `animating`, which means "draw continuously". Something
    /// that changes on its own but rarely, a caret blinking or a tooltip that
    /// appears after a pause, wants one frame at a known moment rather than
    /// sixty a second until then.
    pub redraw_at: Option<f64>,

    /// Whether anything is mid transition and needs another frame.
    pub animating: bool,

    pub size: Size<Px>,
    pub scale: ScaleFactor,
    pub root_font_size: Px,
    /// Monotonic time in seconds.
    pub now: f64,
    /// Whether the platform window is currently maximized, so an application
    /// can show the right control.
    pub window_maximized: bool,
    /// How long the previous frame took to build, lay out, paint and submit.
    pub frame_ms: f32,
    /// Smoothed frames per second, from the same measurement.
    pub fps: f32,
    /// Where the previous frame's time went, in milliseconds. Building is the
    /// element tree and the cascade, layout is flexbox and text measurement,
    /// paint is primitive emission, render is the GPU submission.
    pub build_ms: f32,
    pub layout_ms: f32,
    pub paint_ms: f32,
    pub render_ms: f32,
    /// The render phase, split into its three unrelated parts.
    pub prepare_ms: f32,
    pub acquire_ms: f32,
    pub submit_ms: f32,
    /// Glyphs rasterized on the previous frame.
    pub rasterized: u32,
    /// What the window was holding as of the previous frame.
    pub memory: crate::window::MemoryReport,

    // Traversal state, valid only during a pass.
    id_stack: Vec<GlobalElementId>,
    child_counters: Vec<usize>,
    sibling_counts: Vec<usize>,
    chain: Vec<ElementDescriptor>,
    style_stack: Vec<ComputedStyle>,
    opacity_stack: Vec<f32>,
    /// The ancestors of the element currently being painted.
    paint_stack: Vec<GlobalElementId>,
}

impl std::fmt::Debug for Cx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cx")
            .field("size", &self.size)
            .field("hits", &self.hits.len())
            .field("state", &self.state.len())
            .field("animating", &self.animating)
            .finish()
    }
}

impl Cx {
    pub fn new(text: TextSystem, styles: StyleEngine) -> Self {
        Cx {
            layout: LayoutEngine::new(),
            text,
            styles,
            animator: StyleAnimator::new(),
            scene: Scene::new(),
            state: StateStore::default(),
            input: InputState::default(),
            hits: Vec::new(),
            handlers: HashMap::new(),
            focus_order: Vec::new(),
            scrollbars: Vec::new(),
            dismiss: Vec::new(),
            selectable: HashMap::new(),
            editors: HashMap::new(),
            access: crate::access::AccessTree::default(),
            images: crate::image::ImageStore::default(),
            ime_area: None,
            snap_target: None,
            redraw_at: None,
            animating: false,
            size: Size::new(Px(800.0), Px(600.0)),
            scale: ScaleFactor(1.0),
            root_font_size: Px(16.0),
            now: 0.0,
            window_maximized: false,
            frame_ms: 0.0,
            fps: 0.0,
            build_ms: 0.0,
            layout_ms: 0.0,
            paint_ms: 0.0,
            render_ms: 0.0,
            prepare_ms: 0.0,
            acquire_ms: 0.0,
            submit_ms: 0.0,
            rasterized: 0,
            memory: crate::window::MemoryReport::default(),
            id_stack: Vec::new(),
            child_counters: vec![0],
            sibling_counts: Vec::new(),
            chain: Vec::new(),
            style_stack: Vec::new(),
            opacity_stack: Vec::new(),
            paint_stack: Vec::new(),
        }
    }

    /// Reset for a new frame.
    pub fn begin_frame(&mut self, size: Size<Px>, scale: ScaleFactor, now: f64) {
        self.size = size;
        self.scale = scale;
        self.now = now;
        self.layout.clear();
        self.scene.clear();
        self.hits.clear();
        self.handlers.clear();
        self.focus_order.clear();
        self.scrollbars.clear();
        self.dismiss.clear();
        self.selectable.clear();
        self.access.clear();
        // Re-established by whichever field has focus during paint, so losing
        // focus turns the input method off without anyone having to say so.
        self.ime_area = None;
        self.snap_target = None;
        self.redraw_at = None;
        self.editors.clear();
        self.animating = false;
        self.state.begin_frame();
        self.animator.begin_frame();
        self.id_stack.clear();
        self.child_counters.clear();
        // The root level needs a counter of its own, otherwise every top level
        // element is sibling zero and they all collide on one identity.
        self.child_counters.push(0);
        self.sibling_counts.clear();
        self.chain.clear();
        self.style_stack.clear();
        self.opacity_stack.clear();
        self.paint_stack.clear();
    }

    /// Finish a frame, evicting state nobody used.
    pub fn end_frame(&mut self) {
        self.animator.end_frame();
        self.state.sweep(600);
        // The description of a control is its own text, and that text is only
        // known once its children have been painted. Nothing named itself is
        // touched, so this is where a button finds out it says "Send".
        self.access.resolve_labels();
    }

    // -- identity and the cascade --

    /// The identity of the element currently being visited.
    #[inline]
    pub fn current_id(&self) -> GlobalElementId {
        *self.id_stack.last().unwrap_or(&GlobalElementId::ROOT)
    }

    /// Begin visiting an element. Returns its stable identity.
    pub fn begin_element(
        &mut self,
        id: Option<&ElementId>,
        element_type: &SharedString,
        classes: &[SharedString],
        has_children: bool,
        extra_state: StateFlags,
    ) -> GlobalElementId {
        let index = self.child_counters.last().copied().unwrap_or(0);
        if let Some(counter) = self.child_counters.last_mut() {
            *counter += 1;
        }
        let sibling_count = self.sibling_counts.last().copied().unwrap_or(1);

        let segment = match id {
            Some(id) => id.clone(),
            None => ElementId::Index(index),
        };
        let global = self.current_id().child(&segment);

        let mut state = self.input.flags_for(global, false);
        state.insert(extra_state);

        self.chain.push(ElementDescriptor {
            element_type: element_type.clone(),
            id: match id {
                Some(ElementId::Name(name)) => Some(name.clone()),
                _ => None,
            },
            classes: classes.iter().cloned().collect(),
            state,
            child_index: index,
            sibling_count,
            has_children,
            is_root: self.id_stack.is_empty(),
        });

        self.id_stack.push(global);
        self.child_counters.push(0);
        global
    }

    /// Finish visiting an element.
    pub fn end_element(&mut self) {
        self.id_stack.pop();
        self.child_counters.pop();
        self.chain.pop();
    }

    /// Declare how many children are about to be visited, so structural
    /// selectors such as `:last-child` can be evaluated.
    pub fn enter_children(&mut self, count: usize) {
        self.sibling_counts.push(count.max(1));
    }

    pub fn exit_children(&mut self) {
        self.sibling_counts.pop();
    }

    /// Cascade the stylesheet onto the current element, resolve it against the
    /// parent, and advance any transitions.
    pub fn compute_style(&mut self, inline: &Style) -> ComputedStyle {
        let parent = self
            .style_stack
            .last()
            .cloned()
            .unwrap_or_else(root_style_for_inheritance);

        let target = self
            .styles
            .cascade(&self.chain, inline)
            .resolve(&parent, self.root_font_size);

        let animated = self.animator.animate(self.current_id(), target, self.now);
        if animated.active {
            self.animating = true;
        }
        animated.style
    }

    /// Push a computed style so children inherit from it.
    pub fn push_style(&mut self, style: ComputedStyle) {
        self.style_stack.push(style);
    }

    pub fn pop_style(&mut self) {
        self.style_stack.pop();
    }

    // -- per element state --

    /// Borrow retained state for an element, creating it on first use.
    pub fn with_state<T: Default + 'static, R>(
        &mut self,
        id: GlobalElementId,
        f: impl FnOnce(&mut T, &mut Cx) -> R,
    ) -> R {
        let mut value: T = self.state.take(id);
        let result = f(&mut value, self);
        self.state.put(id, value);
        result
    }

    /// Ask for another frame at a particular moment.
    ///
    /// The earliest request wins, so several things waiting on different
    /// moments all get the frame they asked for. Use this rather than
    /// [`request_animation`](Self::request_animation) for anything that
    /// changes on its own but rarely: a caret blink asking for sixty frames a
    /// second would keep a machine awake to draw the same thing.
    pub fn request_redraw_in(&mut self, seconds: f64) {
        let at = self.now + seconds.max(0.0);
        self.redraw_at = Some(match self.redraw_at {
            Some(existing) => existing.min(at),
            None => at,
        });
    }

    /// Ask the platform window to do something, from the build pass.
    ///
    /// Handlers use [`EventContext`] for this. The build pass needs it too,
    /// for the things an application decides before anyone has clicked
    /// anything: restoring the windows that were open last time, or renaming
    /// this one when the document it shows changes.
    pub fn window_action(&mut self, action: crate::event::WindowAction) {
        self.input.window_actions.push(action);
    }

    /// Open another window, with its own root and its own options.
    pub fn open_window(
        &mut self,
        options: crate::app::WindowOptions,
        root: impl FnMut(&mut Cx) -> crate::element::AnyElement + 'static,
    ) {
        self.window_action(crate::event::WindowAction::Open(Box::new(
            crate::event::NewWindow::new(options, root),
        )));
    }

    /// Rename the window.
    pub fn set_window_title(&mut self, title: impl Into<String>) {
        self.window_action(crate::event::WindowAction::SetTitle(title.into()));
    }

    /// The scroll position of a scrollable element.
    pub fn scroll_state(&self, id: GlobalElementId) -> ScrollState {
        self.state
            .peek::<ScrollState>(id.scoped("scroll"))
            .copied()
            .unwrap_or_default()
    }

    pub fn set_scroll_state(&mut self, id: GlobalElementId, state: ScrollState) {
        self.state.put(id.scoped("scroll"), state);
    }

    // -- painting --

    /// Note that an element is now painting its children.
    pub fn begin_paint(&mut self, id: GlobalElementId) {
        self.paint_stack.push(id);
    }

    pub fn end_paint(&mut self) {
        self.paint_stack.pop();
    }

    /// The opacity in force, as the product of every enclosing element's.
    #[inline]
    pub fn opacity(&self) -> f32 {
        self.opacity_stack.last().copied().unwrap_or(1.0)
    }

    pub fn push_opacity(&mut self, opacity: f32) {
        let combined = self.opacity() * opacity;
        self.opacity_stack.push(combined);
    }

    pub fn pop_opacity(&mut self) {
        self.opacity_stack.pop();
    }

    /// Record an element's interactive footprint.
    pub fn register_hit(
        &mut self,
        id: GlobalElementId,
        bounds: Bounds<Px>,
        cursor: CursorStyle,
        focusable: bool,
        scrollable: bool,
    ) {
        self.hits.push(HitRegion {
            id,
            parent: self.paint_stack.last().copied(),
            bounds,
            layer: self.scene.layer(),
            clip: self.scene.clip(),
            transform: self.scene.transform(),
            cursor,
            focusable,
            scrollable,
        });
        if focusable {
            self.focus_order.push(id);
        }
    }

    /// Record an element's callbacks.
    pub fn register_handlers(&mut self, id: GlobalElementId, handlers: &Handlers) {
        if handlers.is_empty() {
            return;
        }
        self.handlers.insert(id, handlers.clone());
    }

    /// Ask for another frame.
    pub fn request_animation(&mut self) {
        self.animating = true;
    }

    // -- hit testing --

    /// Regions under a point, innermost last.
    pub fn hits_at(&self, point: Point<Px>) -> impl Iterator<Item = &HitRegion> {
        self.hits.iter().filter(move |region| region.contains(point))
    }

    /// Every element under a point, innermost first, overlays before content.
    ///
    /// This is the chain an event travels along. Working with the chain rather
    /// than only its innermost element is what lets a press on a button's label
    /// reach the button.
    pub fn chain_at(&self, point: Point<Px>) -> SmallVec<[HitRegion; 8]> {
        let top = self
            .hits
            .iter()
            .filter(|region| region.contains(point))
            .map(|region| region.layer)
            .max();
        let Some(top) = top else {
            return SmallVec::new();
        };
        self.hits
            .iter()
            .rev()
            .filter(|region| region.layer == top && region.contains(point))
            .cloned()
            .collect()
    }

    /// The topmost region under a point.
    pub fn topmost_at(&self, point: Point<Px>) -> Option<&HitRegion> {
        let top = self
            .hits
            .iter()
            .filter(|region| region.contains(point))
            .map(|region| region.layer)
            .max()?;
        self.hits
            .iter()
            .rev()
            .find(|region| region.layer == top && region.contains(point))
    }

    /// The selectable text under a point, innermost first.
    pub fn selectable_at(&self, point: Point<Px>) -> Option<(GlobalElementId, SelectableText)> {
        self.chain_at(point).into_iter().find_map(|region| {
            self.selectable
                .get(&region.id)
                .map(|text| (region.id, text.clone()))
        })
    }

    /// The byte offset in a selectable element nearest to a window position.
    pub fn text_index_at(&self, id: GlobalElementId, point: Point<Px>) -> Option<usize> {
        let text = self.selectable.get(&id)?;
        let local = Point::new(point.x - text.origin.x, point.y - text.origin.y);
        Some(text.layout.index_for_position(local))
    }

    /// The currently selected text, ready to be copied.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.input.text_selection?;
        let text = self.selectable.get(&selection.id)?;
        let range = selection.range();
        let content = text.layout.text.as_str();
        if range.start >= range.end || range.end > content.len() {
            return None;
        }
        Some(content[range].to_string())
    }

    /// The editable field under a point, innermost first.
    pub fn editor_at(&self, point: Point<Px>) -> Option<(GlobalElementId, TextEditor)> {
        self.chain_at(point).into_iter().find_map(|region| {
            self.editors
                .get(&region.id)
                .map(|editor| (region.id, editor.clone()))
        })
    }

    /// The field that currently holds keyboard focus.
    pub fn focused_editor(&self) -> Option<TextEditor> {
        let focused = self.input.focused?;
        self.editors.get(&focused).cloned()
    }

    /// The byte offset in a field nearest to a window position.
    pub fn editor_index_at(&self, editor: &TextEditor, point: Point<Px>) -> usize {
        let local = Point::new(point.x - editor.origin.x, point.y - editor.origin.y);
        editor.layout.index_for_position(local)
    }

    /// Every element under a point plus all of their ancestors.
    ///
    /// Unlike [`Cx::chain_at`] this follows the paint tree rather than
    /// geometry, so an element counts as "involved" in a click even when the
    /// part that was clicked sits outside its own box. That is what makes a
    /// click on a dropdown's list count as a click on the dropdown.
    pub fn ancestry_at(&self, point: Point<Px>) -> SmallVec<[GlobalElementId; 16]> {
        let chain = self.chain_at(point);
        let mut out: SmallVec<[GlobalElementId; 16]> = SmallVec::new();
        for region in &chain {
            if !out.contains(&region.id) {
                out.push(region.id);
            }
        }

        let mut next = chain.first().and_then(|region| region.parent);
        // The bound is a guard against a malformed paint tree, not an expected
        // depth: real trees are nowhere near this deep.
        for _ in 0..64 {
            let Some(id) = next else {
                break;
            };
            if !out.contains(&id) {
                out.push(id);
            }
            next = self
                .hits
                .iter()
                .find(|region| region.id == id)
                .and_then(|region| region.parent);
        }
        out
    }

    /// The scrollbar thumb or track under a point, if any.
    pub fn scrollbar_at(&self, point: Point<Px>) -> Option<ScrollbarRegion> {
        self.scrollbars
            .iter()
            .rev()
            .find(|bar| bar.track.contains(point) || bar.thumb.contains(point))
            .copied()
    }

    /// Every element under a point, which is the hover set.
    pub fn hovered_at(&self, point: Point<Px>) -> HashSet<GlobalElementId> {
        self.hits_at(point).map(|region| region.id).collect()
    }

    /// The cursor the topmost region asks for.
    pub fn cursor_at(&self, point: Point<Px>) -> CursorStyle {
        self.topmost_at(point)
            .map(|region| region.cursor)
            .unwrap_or(CursorStyle::Default)
    }
}

/// The style the window root inherits from.
///
/// Only inherited properties matter here; everything else is reset per element
/// during resolution anyway.
pub fn root_style_for_inheritance() -> ComputedStyle {
    ComputedStyle {
        color: Rgba::hex(0xe6e6e6),
        font_size: Px(14.0),
        ..ComputedStyle::default()
    }
}

/// The padding box of an element: inside the border, outside the padding.
pub fn padding_box(bounds: Bounds<Px>, style: &ComputedStyle) -> Bounds<Px> {
    bounds.inset(style.border_widths)
}

/// The content box: inside both border and padding.
pub fn content_box(bounds: Bounds<Px>, style: &ComputedStyle, root_font_size: Px) -> Bounds<Px> {
    let resolve = |length: Length, basis: Px| -> Px {
        length.resolve(basis, root_font_size).unwrap_or(Px::ZERO)
    };
    let padding = Edges {
        top: resolve(style.padding.top, bounds.size.height),
        right: resolve(style.padding.right, bounds.size.width),
        bottom: resolve(style.padding.bottom, bounds.size.height),
        left: resolve(style.padding.left, bounds.size.width),
    };
    bounds.inset(style.border_widths).inset(padding)
}

/// Whether an element is laid out and painted at all.
#[inline]
pub fn is_visible(style: &ComputedStyle) -> bool {
    style.display != Display::None && style.opacity > 0.001
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs_style::{Style, Styled};

    fn cx() -> Cx {
        Cx::new(TextSystem::new(), StyleEngine::default())
    }

    #[test]
    fn state_survives_across_frames_and_defaults_on_first_use() {
        let mut store = StateStore::default();
        let id = GlobalElementId(7);
        assert_eq!(store.take::<i32>(id), 0);
        store.put(id, 42i32);
        assert_eq!(store.peek::<i32>(id), Some(&42));
        assert_eq!(store.take::<i32>(id), 42);
    }

    #[test]
    fn state_of_the_wrong_type_resets_rather_than_panicking() {
        let mut store = StateStore::default();
        let id = GlobalElementId(1);
        store.put(id, "text".to_string());
        assert_eq!(store.take::<i32>(id), 0);
    }

    #[test]
    fn stale_state_is_swept_but_recent_state_is_kept() {
        let mut store = StateStore::default();
        store.begin_frame();
        store.put(GlobalElementId(1), 1i32);
        for _ in 0..10 {
            store.begin_frame();
        }
        store.put(GlobalElementId(2), 2i32);
        store.sweep(5);
        assert!(store.peek::<i32>(GlobalElementId(1)).is_none());
        assert_eq!(store.peek::<i32>(GlobalElementId(2)), Some(&2));
    }

    #[test]
    fn sibling_identity_is_positional_and_stable() {
        let mut cx = cx();
        let div = SharedString::from("div");
        cx.enter_children(2);
        let a = cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        let b = cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        cx.exit_children();
        assert_ne!(a, b);

        // A second frame produces the same identities.
        cx.begin_frame(cx.size, cx.scale, 0.0);
        cx.enter_children(2);
        let a2 = cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        cx.exit_children();
        assert_eq!(a, a2);
    }

    #[test]
    fn named_ids_are_stable_regardless_of_position() {
        let mut cx = cx();
        let div = SharedString::from("div");
        let name = ElementId::from("panel");

        cx.enter_children(2);
        let _first = cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        let named = cx.begin_element(Some(&name), &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        cx.exit_children();

        cx.begin_frame(cx.size, cx.scale, 0.0);
        cx.enter_children(1);
        let named_again = cx.begin_element(Some(&name), &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        cx.exit_children();

        assert_eq!(named, named_again);
    }

    #[test]
    fn nesting_produces_distinct_identities() {
        let mut cx = cx();
        let div = SharedString::from("div");
        let outer = cx.begin_element(None, &div, &[], true, StateFlags::EMPTY);
        cx.enter_children(1);
        let inner = cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        cx.end_element();
        cx.exit_children();
        cx.end_element();
        assert_ne!(outer, inner);
    }

    #[test]
    fn the_cascade_sees_the_ancestor_chain() {
        let mut cx = Cx::new(
            TextSystem::new(),
            StyleEngine::from_source(".panel button { color: #ff0000; }"),
        );
        let div = SharedString::from("div");
        let button = SharedString::from("button");
        let panel = [SharedString::from("panel")];

        cx.begin_element(None, &div, &panel, true, StateFlags::EMPTY);
        cx.enter_children(1);
        cx.begin_element(None, &button, &[], false, StateFlags::EMPTY);
        let style = cx.compute_style(&Style::new());
        assert_eq!(style.color, Rgba::hex(0xff0000));
        cx.end_element();
        cx.exit_children();
        cx.end_element();
    }

    #[test]
    fn inherited_properties_flow_down_the_style_stack() {
        let mut cx = cx();
        let div = SharedString::from("div");

        cx.begin_element(None, &div, &[], true, StateFlags::EMPTY);
        let parent = cx.compute_style(&Style::new().text_color(Rgba::hex(0x00ff00)));
        cx.push_style(parent);
        cx.enter_children(1);
        cx.begin_element(None, &div, &[], false, StateFlags::EMPTY);
        let child = cx.compute_style(&Style::new());
        cx.end_element();
        cx.exit_children();
        cx.pop_style();
        cx.end_element();

        assert_eq!(child.color, Rgba::hex(0x00ff00));
    }

    #[test]
    fn hover_state_reaches_the_cascade() {
        let mut cx = Cx::new(
            TextSystem::new(),
            StyleEngine::from_source("button:hover { color: #0000ff; }"),
        );
        let button = SharedString::from("button");

        // Discover the identity the element will get.
        let id = cx.begin_element(None, &button, &[], false, StateFlags::EMPTY);
        cx.end_element();

        cx.input.hovered.insert(id);
        cx.begin_frame(cx.size, cx.scale, 0.0);
        cx.begin_element(None, &button, &[], false, StateFlags::EMPTY);
        let style = cx.compute_style(&Style::new());
        cx.end_element();

        assert_eq!(style.color, Rgba::hex(0x0000ff));
    }

    #[test]
    fn opacity_multiplies_down_the_tree() {
        let mut cx = cx();
        assert_eq!(cx.opacity(), 1.0);
        cx.push_opacity(0.5);
        assert_eq!(cx.opacity(), 0.5);
        cx.push_opacity(0.5);
        assert_eq!(cx.opacity(), 0.25);
        cx.pop_opacity();
        assert_eq!(cx.opacity(), 0.5);
    }

    #[test]
    fn hit_testing_returns_the_topmost_region() {
        let mut cx = cx();
        let under = GlobalElementId(1);
        let over = GlobalElementId(2);
        cx.register_hit(
            under,
            Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)),
            CursorStyle::Default,
            false,
            false,
        );
        cx.register_hit(
            over,
            Bounds::from_xywh(Px(10.0), Px(10.0), Px(20.0), Px(20.0)),
            CursorStyle::Pointer,
            true,
            false,
        );

        let point = Point::new(Px(15.0), Px(15.0));
        assert_eq!(cx.topmost_at(point).unwrap().id, over);
        assert_eq!(cx.cursor_at(point), CursorStyle::Pointer);
        // Both are hovered, which is what lets a container style itself when
        // the pointer is over its child.
        assert_eq!(cx.hovered_at(point).len(), 2);
        assert_eq!(cx.focus_order, vec![over]);
    }

    #[test]
    fn clipped_regions_are_not_hit_outside_their_clip() {
        let mut cx = cx();
        cx.scene
            .push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0)));
        cx.register_hit(
            GlobalElementId(1),
            Bounds::from_xywh(Px(0.0), Px(0.0), Px(200.0), Px(200.0)),
            CursorStyle::Default,
            false,
            false,
        );
        cx.scene.pop_clip();

        assert!(cx.topmost_at(Point::new(Px(10.0), Px(10.0))).is_some());
        assert!(cx.topmost_at(Point::new(Px(100.0), Px(100.0))).is_none());
    }

    #[test]
    fn scroll_state_clamps_to_the_content() {
        let mut state = ScrollState {
            offset: Point::new(Px(0.0), Px(500.0)),
            content: Size::new(Px(100.0), Px(300.0)),
            viewport: Size::new(Px(100.0), Px(200.0)),
        };
        state.clamp();
        assert_eq!(state.offset.y, Px(100.0));
        assert!(state.can_scroll_vertically());
        assert!(!state.can_scroll_horizontally());

        state.offset.y = Px(-50.0);
        state.clamp();
        assert_eq!(state.offset.y, Px(0.0));
    }

    #[test]
    fn content_box_removes_border_then_padding() {
        let style = Style::new()
            .border(2.0)
            .p(8.0)
            .resolve(&ComputedStyle::default(), Px(16.0));
        let bounds = Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0));

        assert_eq!(padding_box(bounds, &style).size.width, Px(96.0));
        let content = content_box(bounds, &style, Px(16.0));
        assert_eq!(content.origin, Point::new(Px(10.0), Px(10.0)));
        assert_eq!(content.size.width, Px(80.0));
    }
}
