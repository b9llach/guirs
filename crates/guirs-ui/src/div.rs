//! The container element.
//!
//! `Div` is to guirs what a `<div>` is to a document: a styled box that holds
//! other elements. Every built in widget is a `Div` underneath with a different
//! type name, which is what lets a stylesheet target `button` or `select`
//! specifically while all of them share one implementation.

use guirs_core::{
    Axis, Bounds, Corners, ElementId, GlobalElementId, Paint, Point, Px, Rgba, SharedString, Size,
};
use guirs_render::{Quad, Shadow};
use guirs_style::{ComputedStyle, Overflow, StateFlags, Style, Styled};
use smallvec::SmallVec;

use crate::context::{
    content_box, padding_box, Cx, DismissHandler, DismissRegion, Handlers, ScrollState,
    ScrollbarRegion,
};
use crate::element::{AnyElement, Element, IntoElement};
use crate::event::{KeyEvent, MouseEvent, ScrollEvent, TextInputEvent};
use crate::layout::LayoutId;

/// Width of the overlay scrollbar, in logical pixels.
const SCROLLBAR_WIDTH: Px = Px(10.0);
/// Shortest a scrollbar thumb is allowed to get.
const SCROLLBAR_MIN_THUMB: Px = Px(28.0);

/// How many rows past each edge of the viewport are built anyway.
///
/// Which rows are visible is answered from the previous frame's scroll offset,
/// because a frame decides what to build before it knows how big it is. During
/// a fast scroll that answer is a frame stale, and this is the margin that
/// covers the difference.
const OVERSCAN_ROWS: usize = 3;

/// Rows a scrolling container builds on demand rather than all at once.
struct VirtualRows {
    count: usize,
    row_height: Px,
    build: Box<dyn Fn(usize) -> AnyElement>,
}

/// A styled container.
pub struct Div {
    element_type: SharedString,
    id: Option<ElementId>,
    classes: SmallVec<[SharedString; 4]>,
    style: Style,
    children: Vec<AnyElement>,
    handlers: Handlers,
    focusable: bool,
    extra_state: StateFlags,
    overlay: bool,
    /// Named so a keymap can say "only here". Claimed by everything painted
    /// inside this element, not only by the element itself.
    key_context: Option<SharedString>,
    stick_to_bottom: bool,
    on_dismiss: Option<DismissHandler>,
    virtual_rows: Option<VirtualRows>,
    autofocus: bool,
    snap_target: bool,
    /// What a screen reader should call this, where the text inside it is not
    /// the answer.
    access_label: Option<SharedString>,
    access_role: Option<crate::access::AccessRole>,
    access_checked: Option<bool>,
    access_value: Option<SharedString>,
    access_numeric: Option<[f64; 3]>,
    access_decorative: bool,

    // Scratch, valid between this frame's layout and paint passes.
    global_id: GlobalElementId,
    computed: ComputedStyle,
    child_nodes: Vec<LayoutId>,
    node: Option<LayoutId>,
}

/// A plain container.
pub fn div() -> Div {
    Div::new("div")
}

impl Div {
    /// A container reported to the stylesheet under `element_type`.
    pub fn new(element_type: &'static str) -> Self {
        Div {
            element_type: SharedString::from(element_type),
            id: None,
            classes: SmallVec::new(),
            style: Style::new(),
            children: Vec::new(),
            handlers: Handlers::default(),
            focusable: false,
            extra_state: StateFlags::EMPTY,
            overlay: false,
            key_context: None,
            stick_to_bottom: false,
            on_dismiss: None,
            virtual_rows: None,
            autofocus: false,
            snap_target: false,
            access_label: None,
            access_role: None,
            access_checked: None,
            access_value: None,
            access_numeric: None,
            access_decorative: false,
            global_id: GlobalElementId::ROOT,
            computed: ComputedStyle::default(),
            child_nodes: Vec::new(),
            node: None,
        }
    }

    /// Give this element a stable name, so its retained state and its identity
    /// survive being moved among its siblings.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one or more classes.
    ///
    /// Whitespace separates names, so `class("avatar avatar-you")` adds two,
    /// the way it reads. Passing a single name is the common case and costs
    /// nothing extra.
    pub fn class(mut self, class: impl Into<SharedString>) -> Self {
        push_classes(&mut self.classes, class.into());
        self
    }

    pub fn classes(mut self, classes: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.classes.extend(classes.into_iter().map(Into::into));
        self
    }

    /// Add a class only when `condition` holds.
    pub fn class_if(self, condition: bool, class: impl Into<SharedString>) -> Self {
        if condition {
            self.class(class)
        } else {
            self
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoElement::into_any));
        self
    }

    /// Apply a transformation only when `condition` holds.
    ///
    /// Keeps a builder chain unbroken instead of forcing a `let mut` and an
    /// `if` in the middle of otherwise declarative code.
    pub fn when(self, condition: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            f(self)
        } else {
            self
        }
    }

    /// Apply a transformation when an option has a value.
    pub fn when_some<T>(self, value: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => f(self, value),
            None => self,
        }
    }

    /// Paint this element and its children above everything else, ignoring the
    /// clip of whatever contains them.
    ///
    /// This is what a dropdown, a menu or a tooltip needs: it belongs to an
    /// element in the tree but it has to draw over the window, not inside its
    /// parent's scroll viewport.
    pub fn overlay(mut self) -> Self {
        self.overlay = true;
        self
    }

    /// Name where the keyboard is, for a keymap to bind against.
    ///
    /// The name applies to this element and everything inside it while any of
    /// it has focus, so a keymap can give one key different meanings in an
    /// editor and in a file tree without either knowing about the other.
    ///
    /// ```
    /// # use guirs_ui::div::div;
    /// div().key_context("editor");
    /// ```
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Answer a command, whether it came from a key, a menu or anywhere else.
    ///
    /// A command travels outwards from whatever has focus until something
    /// answers it, so the innermost element that cares gets it and everything
    /// else can ignore it.
    ///
    /// ```
    /// # use guirs_ui::div::div;
    /// div().on_command("file:save", |_cx| { /* save */ });
    /// ```
    pub fn on_command(
        mut self,
        command: impl Into<SharedString>,
        handler: impl Fn(&mut crate::EventContext) + 'static,
    ) -> Self {
        self.handlers
            .on_command
            .push((command.into(), std::rc::Rc::new(handler)));
        self
    }

    /// Keep a scrolling view pinned to its end as content is added.
    ///
    /// Only while the view is already at the end, so a reader who has scrolled
    /// up to look at something is not yanked back down the moment a new message
    /// arrives. This is what a chat transcript or a log tail wants.
    pub fn stick_to_bottom(mut self) -> Self {
        self.stick_to_bottom = true;
        self
    }

    /// Be told when a press lands outside this element and everything it
    /// contains, wherever those children were painted.
    ///
    /// This is how a dropdown, a menu or a popover closes itself.
    pub fn on_dismiss(mut self, f: impl Fn(&mut crate::EventContext) + 'static) -> Self {
        self.on_dismiss = Some(std::rc::Rc::new(f));
        self
    }

    /// Let this element take keyboard focus.
    pub fn focusable(mut self) -> Self {
        self.focusable = true;
        self
    }

    /// Force extra pseudo class state, as a checkbox does for `:checked`.
    pub fn with_state(mut self, flags: StateFlags) -> Self {
        self.extra_state.insert(flags);
        self
    }

    /// Set or clear extra state from a condition.
    pub fn state_if(mut self, flags: StateFlags, on: bool) -> Self {
        self.extra_state.set(flags, on);
        self
    }

    // -- events --

    /// Fired when a press and its release both land on this element.
    ///
    /// Handlers accumulate rather than replace. A widget such as a text field
    /// already has its own key handling, and silently destroying it the moment
    /// a caller adds one of their own is a trap: the field would simply stop
    /// responding to backspace with nothing to show why.
    pub fn on_click(mut self, f: impl Fn(&MouseEvent, &mut crate::EventContext) + 'static) -> Self {
        add_handler(&mut self.handlers.on_click, f);
        self
    }

    pub fn on_mouse_down(
        mut self,
        f: impl Fn(&MouseEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_mouse_down, f);
        self
    }

    pub fn on_mouse_up(
        mut self,
        f: impl Fn(&MouseEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_mouse_up, f);
        self
    }

    pub fn on_mouse_move(
        mut self,
        f: impl Fn(&MouseEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_mouse_move, f);
        self
    }

    pub fn on_scroll(mut self, f: impl Fn(&ScrollEvent, &mut crate::EventContext) + 'static) -> Self {
        add_handler(&mut self.handlers.on_scroll, f);
        self
    }

    /// Fired while this element has keyboard focus.
    pub fn on_key_down(mut self, f: impl Fn(&KeyEvent, &mut crate::EventContext) + 'static) -> Self {
        add_handler(&mut self.handlers.on_key_down, f);
        self.focusable = true;
        self
    }

    /// What a screen reader should call this.
    ///
    /// Only needed where the text inside an element is not its name: a button
    /// holding nothing but an icon, or one whose visible text is an
    /// abbreviation of what it does.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.access_label = Some(label.into());
        self
    }

    /// Leave this and everything in it out of what a screen reader is told.
    ///
    /// For the parts of a widget that are drawing rather than meaning: the
    /// tick inside a checkbox, the knob on a switch, the filled part of a
    /// progress bar. Read out, they turn a control's name into nonsense.
    pub fn decorative(mut self) -> Self {
        self.access_decorative = true;
        self
    }

    /// Describe this as something other than its element type implies.
    pub fn role(mut self, role: crate::access::AccessRole) -> Self {
        self.access_role = Some(role);
        self
    }

    /// Whether this control is on, for a checkbox, radio or switch.
    pub fn access_checked(mut self, checked: bool) -> Self {
        self.access_checked = Some(checked);
        self
    }

    /// What this control currently holds, where that is not its text.
    pub fn access_value(mut self, value: impl Into<SharedString>) -> Self {
        self.access_value = Some(value.into());
        self
    }

    /// Where this control sits in a range, for a slider or a progress bar.
    pub fn access_range(mut self, value: f64, min: f64, max: f64) -> Self {
        self.access_numeric = Some([value, min, max]);
        self
    }

    /// Mark this element as the window's maximize control.
    ///
    /// Windows shows a layout picker when the pointer rests on a maximize
    /// button, and it asks the window whether the pointer is on one. Saying so
    /// here is what lets a window that draws its own controls keep that,
    /// rather than losing it the moment it stops using the platform's.
    ///
    /// Does nothing on platforms with no such feature.
    pub fn snap_target(mut self) -> Self {
        self.snap_target = true;
        self
    }

    /// Take keyboard focus when the window opens.
    ///
    /// Only if nothing else has it, so several elements asking is not a fight
    /// and a person who has already clicked somewhere is not interrupted. A
    /// composer or a search box is the usual case.
    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self.focusable = true;
        self
    }

    /// Fired when files are dropped onto this element from the desktop.
    ///
    /// The event carries every path in the drop, not one call per file, so a
    /// handler sees a multiple file drop as the single gesture it was.
    pub fn on_file_drop(
        mut self,
        f: impl Fn(&crate::event::FileDropEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_file_drop, f);
        self
    }

    /// Fired while files are over this element, and again with an empty list
    /// when they leave, so a drop target can light up and go dark again.
    pub fn on_file_hover(
        mut self,
        f: impl Fn(&crate::event::FileDropEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_file_hover, f);
        self
    }

    /// Fired for text the platform's input method produced.
    pub fn on_text(
        mut self,
        f: impl Fn(&TextInputEvent, &mut crate::EventContext) + 'static,
    ) -> Self {
        add_handler(&mut self.handlers.on_text, f);
        self.focusable = true;
        self
    }

    // -- internals shared with the widgets --

    pub fn computed(&self) -> &ComputedStyle {
        &self.computed
    }

    pub fn global_id(&self) -> GlobalElementId {
        self.global_id
    }

    fn scrolls(&self) -> bool {
        self.computed.overflow_x.scrolls() || self.computed.overflow_y.scrolls()
    }

    /// Fill this container with `count` rows of a fixed height, building only
    /// the ones on screen.
    ///
    /// The rows a reader can see are a few dozen whatever `count` says, so this
    /// is what separates a list that opens instantly from one that lays out a
    /// hundred thousand boxes first. The container has to scroll, and every row
    /// has to be the same height, because the scroll extent is computed from
    /// the count rather than measured.
    ///
    /// ```
    /// # use guirs_core::px;
    /// # use guirs_ui::{div::div, element::IntoElement, widgets::scroll_view};
    /// scroll_view().virtual_rows(200_000, px(28.0), move |index| {
    ///     div().child(format!("line {index}")).into_any()
    /// });
    /// ```
    pub fn virtual_rows(
        mut self,
        count: usize,
        row_height: impl Into<Px>,
        build: impl Fn(usize) -> AnyElement + 'static,
    ) -> Self {
        self.virtual_rows = Some(VirtualRows {
            count,
            row_height: row_height.into(),
            build: Box::new(build),
        });
        self
    }

    /// Which rows to build this frame, from where the reader was last frame.
    ///
    /// Returned as a range so it can be tested without a frame around it.
    fn visible_rows(count: usize, row_height: Px, scroll: ScrollState) -> std::ops::Range<usize> {
        if count == 0 || row_height <= Px::ZERO {
            return 0..0;
        }
        // Before the first frame there is no viewport to go on. Building a
        // screenful is closer than building nothing, and the next frame
        // corrects it.
        let viewport = if scroll.viewport.height > Px::ZERO {
            scroll.viewport.height
        } else {
            Px(1024.0)
        };

        let first = (scroll.offset.y.0 / row_height.0).floor().max(0.0) as usize;
        let visible = (viewport.0 / row_height.0).ceil() as usize + 1;

        let start = first.saturating_sub(OVERSCAN_ROWS);
        let end = first
            .saturating_add(visible)
            .saturating_add(OVERSCAN_ROWS)
            .min(count);
        start..end.max(start)
    }

    /// Replace the children with just the rows that are on screen.
    ///
    /// The rows sit absolutely positioned inside one box as tall as the whole
    /// list, so the scrollbar and the scroll extent describe all `count` of
    /// them while only a screenful exists.
    fn materialize_virtual_rows(&mut self, cx: &Cx) {
        let Some(rows) = self.virtual_rows.take() else {
            return;
        };
        let scroll = cx.scroll_state(self.global_id);
        let range = Div::visible_rows(rows.count, rows.row_height, scroll);

        let mut content = div()
            .relative()
            .w_full()
            .h(rows.row_height * rows.count as f32);
        for index in range {
            content = content.child(
                div()
                    .id(index as u64)
                    // Keyed by row rather than by slot. A row's position among
                    // the built children changes every time the list scrolls,
                    // so without this the hover, the transitions and any other
                    // retained state would stay put while the rows moved
                    // underneath them.
                    .absolute()
                    .top(rows.row_height * index as f32)
                    .left(Px::ZERO)
                    .right(Px::ZERO)
                    .h(rows.row_height)
                    .child((rows.build)(index)),
            );
        }

        self.children.clear();
        self.children.push(content.into_any());
    }
}

impl Styled for Div {
    #[inline]
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl Element for Div {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        self.global_id = cx.begin_element(
            self.id.as_ref(),
            &self.element_type,
            &self.classes,
            !self.children.is_empty(),
            self.extra_state,
        );

        self.computed = cx.compute_style(&self.style);
        // Before the child count is fixed, because a virtualized container
        // decides how many children it has by looking at where the reader is.
        self.materialize_virtual_rows(cx);

        cx.push_style(self.computed.clone());
        cx.enter_children(self.children.len());
        self.child_nodes.clear();
        self.child_nodes.reserve(self.children.len());
        for child in &mut self.children {
            let node = child.request_layout(cx);
            self.child_nodes.push(node);
        }
        cx.exit_children();
        cx.pop_style();

        let node = cx.layout.add_node(&self.computed, &self.child_nodes);
        cx.end_element();
        self.node = Some(node);
        node
    }

    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        if !crate::context::is_visible(&self.computed) {
            return;
        }

        if self.overlay {
            cx.scene.push_overlay();
        }

        // A transform applies to everything this element draws, its own
        // background included, so it goes on first and comes off last. Layout
        // has already run by now: a transform moves what is drawn without
        // moving anything else on the screen, which is what makes it cheap
        // enough to animate every frame.
        let transformed = !self.computed.transform.is_identity();
        if transformed {
            cx.scene
                .push_transform(self.computed.transform.to_affine(bounds));
        }

        if self.snap_target {
            cx.snap_target = Some(bounds);
        }

        // Opened before the children are painted and closed after them, so the
        // tree a reader walks is the tree that was drawn.
        if self.access_decorative {
            cx.access.suppress();
        }
        let role = self.access_role.unwrap_or_else(|| {
            let named = crate::access::AccessRole::for_element(&self.element_type);
            // A plain box you can press is a button, whatever it is called.
            // Left as a group it would be dropped from the tree the platform
            // builds, taking the only way of reaching it with it, and a
            // clickable div is how most of an interface gets built here.
            if named == crate::access::AccessRole::Group && self.handlers.on_click.is_some() {
                crate::access::AccessRole::Button
            } else {
                named
            }
        });
        let described = cx.access.push(self.global_id, role, bounds);
        // Only when this element got a node of its own. Without the guard, a
        // child that was skipped would find its parent at the top of the stack
        // and overwrite the parent's state with its own empty one, which is
        // how a ticked checkbox comes out saying nothing.
        if described.is_some() {
            if let Some(node) = cx.access.current_mut() {
                node.label = self.access_label.clone();
                node.value = self.access_value.clone();
                node.checked = self.access_checked;
                node.numeric = self.access_numeric;
                node.focusable = self.focusable;
                node.clickable = self.handlers.on_click.is_some();
                node.disabled = self.extra_state.contains(StateFlags::DISABLED);
            }
        }

        // Claimed during paint, because that is when this element's identity
        // and the rest of the frame's focus state are both known.
        if self.autofocus && cx.input.focused.is_none() {
            cx.input.focused = Some(self.global_id);
            cx.input.focus_visible = false;
        }

        cx.push_opacity(self.computed.opacity);
        let opacity = cx.opacity();
        let radii = self.computed.border_radius.clamp_to(bounds.size);

        // Drop shadows go under the box, inset shadows over it.
        for shadow in &self.computed.box_shadows {
            if !shadow.inset {
                cx.scene.push_shadow(Shadow {
                    bounds,
                    corner_radii: radii,
                    shadow: *shadow,
                    opacity,
                });
            }
        }

        cx.scene.push_quad(Quad {
            bounds,
            corner_radii: radii,
            background: self.computed.background.clone(),
            border_widths: if self.computed.border_style.draws() {
                self.computed.border_widths
            } else {
                guirs_core::Edges::zero()
            },
            border_color: self.computed.border_color,
            opacity,
        });

        for shadow in &self.computed.box_shadows {
            if shadow.inset {
                cx.scene.push_shadow(Shadow {
                    bounds,
                    corner_radii: radii,
                    shadow: *shadow,
                    opacity,
                });
            }
        }

        // Register interactivity before clipping, so this element is clipped by
        // its ancestors but not by its own overflow rule.
        if self.computed.pointer_events {
            cx.register_hit(
                self.global_id,
                bounds,
                self.computed.cursor,
                self.focusable,
                self.scrolls(),
            );
            cx.register_handlers(self.global_id, &self.handlers);
        }
        if let Some(handler) = &self.on_dismiss {
            cx.dismiss.push(DismissRegion {
                id: self.global_id,
                handler: handler.clone(),
            });
        }

        // Work out how far the content is scrolled, and how far it could be.
        let viewport = content_box(bounds, &self.computed, cx.root_font_size);
        let mut scroll = ScrollState::default();
        if self.scrolls() {
            if let Some(node) = self.node {
                scroll = cx.scroll_state(self.global_id);
                // Whether the reader was at the end has to be decided against
                // the previous frame's extent, before this frame's content
                // replaces it.
                let was_at_end = scroll.offset.y >= scroll.max_offset().y - Px(8.0);

                scroll.content = cx.layout.content_size(node);
                scroll.viewport = viewport.size;
                if self.stick_to_bottom && was_at_end {
                    scroll.offset.y = scroll.max_offset().y;
                }
                scroll.clamp();
                cx.set_scroll_state(self.global_id, scroll);
            }
        }

        let clips = self.computed.clips_content();
        if clips {
            // Clip to the padding box, and to the same corners the border
            // draws. Without the corners a scrolling list inside a rounded
            // card squares off its own top edge against the card's curve.
            let inner = padding_box(bounds, &self.computed);
            let radii = inner_radii(&self.computed, bounds);
            cx.scene.push_rounded_clip(inner, radii);
        }

        let child_origin = Point::new(
            bounds.origin.x - scroll.offset.x,
            bounds.origin.y - scroll.offset.y,
        );
        // Claimed around the children, so everything inside is in this
        // context and the element itself is too.
        if let Some(context) = &self.key_context {
            cx.push_key_context(context.clone());
        }
        // Recorded before the children paint, so an element that is itself
        // focused captures the contexts enclosing it including its own.
        cx.note_focus_path(self.global_id);

        cx.begin_paint(self.global_id);
        for (child, node) in self.children.iter_mut().zip(self.child_nodes.iter()) {
            let child_bounds = cx.layout.absolute_bounds(*node, child_origin);
            child.paint(child_bounds, cx);
        }
        cx.end_paint();

        if self.key_context.is_some() {
            cx.pop_key_context();
        }

        if clips {
            cx.scene.pop_clip();
        }

        if self.scrolls() {
            paint_scrollbars(cx, self.global_id, bounds, &self.computed, scroll, opacity);
        }

        if described.is_some() {
            cx.access.pop();
        }
        if self.access_decorative {
            cx.access.resume();
        }

        cx.pop_opacity();
        if transformed {
            cx.scene.pop_transform();
        }
        if self.overlay {
            cx.scene.pop_layer();
        }
    }
}

/// The corner radii of the padding box, inside the border.
///
/// A border drawn a few pixels thick leaves a curve that much tighter inside
/// it. Clipping to the outer radius instead would leave a hairline of the
/// child showing through the border, which is exactly the artifact the clip
/// exists to prevent.
fn inner_radii(style: &ComputedStyle, bounds: Bounds<Px>) -> Corners<Px> {
    let radii = style.border_radius.clamp_to(bounds.size);
    let edges = style.border_widths;
    let inside = |radius: Px, first: Px, second: Px| (radius - first.max(second)).max(Px::ZERO);
    Corners {
        top_left: inside(radii.top_left, edges.top, edges.left),
        top_right: inside(radii.top_right, edges.top, edges.right),
        bottom_right: inside(radii.bottom_right, edges.bottom, edges.right),
        bottom_left: inside(radii.bottom_left, edges.bottom, edges.left),
    }
}

crate::impl_into_element!(Div);

/// Draw overlay scrollbars for a scrollable element.
///
/// They are drawn after the children and outside the clip, so they float above
/// the content the way a modern scrollbar does rather than taking layout space.
fn paint_scrollbars(
    cx: &mut Cx,
    owner: GlobalElementId,
    bounds: Bounds<Px>,
    style: &ComputedStyle,
    scroll: ScrollState,
    opacity: f32,
) {
    let thumb_color = cx
        .styles
        .color_token("scrollbar-thumb")
        .unwrap_or_else(|| Rgba::rgba8(255, 255, 255, 60));
    let track_color = cx
        .styles
        .color_token("scrollbar-track")
        .unwrap_or(Rgba::TRANSPARENT);

    let max = scroll.max_offset();
    let inset = Px(2.0);

    if style.overflow_y.scrolls() && scroll.can_scroll_vertically() {
        let track = Bounds::from_xywh(
            bounds.right() - SCROLLBAR_WIDTH - inset,
            bounds.top() + inset,
            SCROLLBAR_WIDTH,
            bounds.height() - inset * 2.0,
        );
        let ratio = (scroll.viewport.height / scroll.content.height).clamp(0.0, 1.0);
        let thumb_height = (track.height() * ratio).max(SCROLLBAR_MIN_THUMB).min(track.height());
        let progress = if max.y > Px::ZERO {
            (scroll.offset.y / max.y).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb = Bounds::from_xywh(
            track.left(),
            track.top() + (track.height() - thumb_height) * progress,
            track.width(),
            thumb_height,
        );
        push_bar(cx, track, track_color, opacity);
        push_bar(cx, thumb, thumb_color, opacity);
        cx.scrollbars.push(ScrollbarRegion {
            owner,
            axis: Axis::Vertical,
            // The grabbable area is wider than the drawn bar, because a ten
            // pixel target is a miserable thing to hit with a pointer.
            track: track.outset(guirs_core::Edges::xy(Px(4.0), Px::ZERO)),
            thumb: thumb.outset(guirs_core::Edges::xy(Px(4.0), Px::ZERO)),
        });
    }

    if style.overflow_x.scrolls() && scroll.can_scroll_horizontally() {
        let track = Bounds::from_xywh(
            bounds.left() + inset,
            bounds.bottom() - SCROLLBAR_WIDTH - inset,
            bounds.width() - inset * 2.0,
            SCROLLBAR_WIDTH,
        );
        let ratio = (scroll.viewport.width / scroll.content.width).clamp(0.0, 1.0);
        let thumb_width = (track.width() * ratio).max(SCROLLBAR_MIN_THUMB).min(track.width());
        let progress = if max.x > Px::ZERO {
            (scroll.offset.x / max.x).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb = Bounds::from_xywh(
            track.left() + (track.width() - thumb_width) * progress,
            track.top(),
            thumb_width,
            track.height(),
        );
        push_bar(cx, track, track_color, opacity);
        push_bar(cx, thumb, thumb_color, opacity);
        cx.scrollbars.push(ScrollbarRegion {
            owner,
            axis: Axis::Horizontal,
            track: track.outset(guirs_core::Edges::xy(Px::ZERO, Px(4.0))),
            thumb: thumb.outset(guirs_core::Edges::xy(Px::ZERO, Px(4.0))),
        });
    }
}

fn push_bar(cx: &mut Cx, bounds: Bounds<Px>, color: Rgba, opacity: f32) {
    if color.is_transparent() {
        return;
    }
    let radius = (bounds.width().min(bounds.height())) * 0.5;
    cx.scene.push_quad(Quad {
        bounds,
        corner_radii: Corners::all(radius),
        background: Paint::Solid(color),
        border_widths: guirs_core::Edges::zero(),
        border_color: Rgba::TRANSPARENT,
        opacity,
    });
}

/// One element's callback for a given kind of event.
type HandlerSlot<E> = std::rc::Rc<dyn Fn(&E, &mut crate::EventContext)>;

/// Add a handler to a slot, keeping whatever was already there.
///
/// Both run, in the order they were added, so a widget's own behaviour happens
/// before the caller's addition to it.
fn add_handler<E: 'static>(
    slot: &mut Option<HandlerSlot<E>>,
    f: impl Fn(&E, &mut crate::EventContext) + 'static,
) {
    let added = std::rc::Rc::new(f);
    *slot = Some(match slot.take() {
        Some(existing) => std::rc::Rc::new(move |event: &E, cx: &mut crate::EventContext| {
            existing(event, cx);
            added(event, cx);
        }),
        None => added,
    });
}

/// Split a class attribute on whitespace, avoiding the allocation when there
/// is only one name, which there almost always is.
pub(crate) fn push_classes(out: &mut SmallVec<[SharedString; 4]>, value: SharedString) {
    if !value.as_str().contains(char::is_whitespace) {
        if !value.is_empty() {
            out.push(value);
        }
        return;
    }
    for name in value.as_str().split_whitespace() {
        out.push(SharedString::from(name));
    }
}

/// A fixed size spacer.
pub fn spacer(size: Px) -> Div {
    div().w(size).h(size)
}

/// A spacer that eats all remaining space along the main axis.
pub fn flex_spacer() -> Div {
    div().flex_1()
}

/// A one pixel separator line.
pub fn separator() -> Div {
    div().h(Px(1.0)).w_full()
}

/// Whether a style asks for a scrollable box on either axis.
pub fn is_scrollable(style: &ComputedStyle) -> bool {
    style.overflow_x.scrolls() || style.overflow_y.scrolls()
}

/// Whether a style hides overflow without allowing scrolling.
pub fn clips_only(style: &ComputedStyle) -> bool {
    style.overflow_x == Overflow::Hidden && style.overflow_y == Overflow::Hidden
}

/// The intrinsic size a scrollable element reports to its parent.
pub fn scroll_viewport(bounds: Bounds<Px>, style: &ComputedStyle, root_font_size: Px) -> Size<Px> {
    content_box(bounds, style, root_font_size).size
}

#[cfg(test)]
mod tests {
    mod virtual_rows_tests {
        use crate::context::{Cx, ScrollState};
        use crate::div::{div, Div, OVERSCAN_ROWS};
        use crate::element::{Element, IntoElement};
        use guirs_core::{Point, Px, ScaleFactor, Size};
        use guirs_style::{Overflow, StyleEngine, Styled};

        fn scrolled_to(offset: f32, viewport: f32) -> ScrollState {
            ScrollState {
                offset: Point::new(Px::ZERO, Px(offset)),
                content: Size::new(Px(300.0), Px(1_000_000.0)),
                viewport: Size::new(Px(300.0), Px(viewport)),
            }
        }

        #[test]
        fn only_a_window_of_a_huge_list_is_built() {
            let range = Div::visible_rows(100_000, Px(20.0), scrolled_to(0.0, 400.0));
            // Twenty rows fit, plus one straddling the edge, plus overscan.
            assert_eq!(range.start, 0);
            assert!(range.end <= 21 + OVERSCAN_ROWS);
            assert!(range.len() < 100);
        }

        #[test]
        fn the_window_follows_the_scroll_offset() {
            let range = Div::visible_rows(100_000, Px(20.0), scrolled_to(10_000.0, 400.0));
            // Row 500 is at the top of the viewport.
            assert_eq!(range.start, 500 - OVERSCAN_ROWS);
            assert!(range.contains(&500));
            assert!(range.contains(&520));
            assert!(!range.contains(&600));
        }

        #[test]
        fn the_window_is_clamped_at_both_ends() {
            // At the very top there is nothing before row zero to overscan into.
            let top = Div::visible_rows(100, Px(20.0), scrolled_to(0.0, 400.0));
            assert_eq!(top.start, 0);

            // And at the very bottom it stops at the last row.
            let bottom = Div::visible_rows(100, Px(20.0), scrolled_to(1600.0, 400.0));
            assert_eq!(bottom.end, 100);
            assert!(bottom.start < bottom.end);
        }

        #[test]
        fn an_empty_or_degenerate_list_builds_nothing() {
            assert!(Div::visible_rows(0, Px(20.0), scrolled_to(0.0, 400.0)).is_empty());
            assert!(Div::visible_rows(10, Px::ZERO, scrolled_to(0.0, 400.0)).is_empty());
            assert!(Div::visible_rows(10, Px(-5.0), scrolled_to(0.0, 400.0)).is_empty());
        }

        #[test]
        fn the_first_frame_builds_a_screenful_rather_than_nothing() {
            // No viewport has been measured yet, so there is nothing to go on.
            let cold = ScrollState::default();
            let range = Div::visible_rows(100_000, Px(20.0), cold);
            assert!(!range.is_empty(), "a cold list would have painted blank");
            assert!(range.len() < 200);
        }

        /// Lay out one tree and hand back the frame.
        fn frame(mut root: Div, size: Size<Px>) -> (Cx, usize) {
            let mut cx = Cx::new(guirs_text::TextSystem::new(), StyleEngine::default());
            cx.begin_frame(size, ScaleFactor(1.0), 0.0);
            let node = root.request_layout(&mut cx);
            cx.layout.compute(node, size, &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);
            let nodes = cx.layout.node_count();
            cx.end_frame();
            (cx, nodes)
        }

        #[test]
        fn a_hundred_thousand_rows_lay_out_a_handful_of_boxes() {
            let (_, nodes) = frame(
                div()
                    .w(Px(300.0))
                    .h(Px(400.0))
                    .overflow_y(Overflow::Scroll)
                    .virtual_rows(100_000, Px(20.0), |index| {
                        div().h(Px(20.0)).child(format!("row {index}")).into_any()
                    }),
                Size::new(Px(300.0), Px(400.0)),
            );

            // The list, the content box, and three nodes per visible row. What
            // matters is that it is a function of the viewport rather than of
            // the hundred thousand.
            assert!(nodes < 300, "laid out {nodes} nodes for a screenful");
        }

        #[test]
        fn the_scroll_extent_describes_every_row_not_just_the_built_ones() {
            let mut root = div()
                .w(Px(300.0))
                .h(Px(400.0))
                .overflow_y(Overflow::Scroll)
                .virtual_rows(1_000, Px(20.0), |_| div().h(Px(20.0)).into_any());

            let mut cx = Cx::new(guirs_text::TextSystem::new(), StyleEngine::default());
            cx.begin_frame(Size::new(Px(300.0), Px(400.0)), ScaleFactor(1.0), 0.0);
            let node = root.request_layout(&mut cx);
            cx.layout
                .compute(node, Size::new(Px(300.0), Px(400.0)), &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);

            let scroll = cx.scroll_state(root.global_id());
            // A thousand rows of twenty pixels, whether or not they exist yet.
            assert_eq!(scroll.content.height, Px(20_000.0));
            cx.end_frame();
        }

        #[test]
        fn a_row_keeps_its_identity_when_the_list_scrolls() {
            let size = Size::new(Px(300.0), Px(400.0));
            const ROW: Px = Px(20.0);

            // Rows are the full width; what a row contains is narrower, so the
            // two can be told apart in the hit list.
            fn draw(
                cx: &mut Cx,
                size: Size<Px>,
            ) -> (guirs_core::GlobalElementId, Vec<crate::context::HitRegion>) {
                let mut root = div()
                    .w(Px(300.0))
                    .h(Px(400.0))
                    .overflow_y(Overflow::Scroll)
                    .virtual_rows(10_000, ROW, |index| {
                        div()
                            .h(ROW)
                            .w(Px(50.0))
                            .child(format!("row {index}"))
                            .into_any()
                    });
                cx.begin_frame(size, ScaleFactor(1.0), 0.0);
                let node = root.request_layout(cx);
                cx.layout.compute(node, size, &mut cx.text);
                root.paint(cx.layout.bounds(node), cx);
                let hits = cx.hits.clone();
                cx.end_frame();
                (root.global_id(), hits)
            }

            /// The identity of the row whose top edge sits at `y` on screen.
            fn row_at(hits: &[crate::context::HitRegion], y: Px) -> guirs_core::GlobalElementId {
                hits.iter()
                    .find(|region| {
                        region.bounds.size.width == Px(300.0)
                            && region.bounds.size.height == ROW
                            && (region.bounds.origin.y - y).0.abs() < 0.01
                    })
                    .unwrap_or_else(|| panic!("no row found at y = {y:?}"))
                    .id
            }

            fn scroll_to(cx: &mut Cx, list: guirs_core::GlobalElementId, offset: f32, size: Size<Px>) {
                cx.set_scroll_state(
                    list,
                    ScrollState {
                        offset: Point::new(Px::ZERO, Px(offset)),
                        content: Size::new(Px(300.0), Px(200_000.0)),
                        viewport: size,
                    },
                );
            }

            let mut cx = Cx::new(guirs_text::TextSystem::new(), StyleEngine::default());
            let (list, _) = draw(&mut cx, size);

            // Row 500 sits at the top of the viewport.
            scroll_to(&mut cx, list, 10_000.0, size);
            let (_, before) = draw(&mut cx, size);
            let five_hundred = row_at(&before, Px::ZERO);

            // Scroll by exactly one row. Row 500 has moved up by one, and now
            // occupies a different slot among the built children. Its identity
            // has to follow the row, or the hover and every transition would
            // stay with the slot while the rows slid underneath.
            scroll_to(&mut cx, list, 10_020.0, size);
            let (_, after) = draw(&mut cx, size);
            assert_eq!(
                row_at(&after, -ROW),
                five_hundred,
                "row 500 was renamed when the list scrolled"
            );

            // And the row that took its place on screen is a different one.
            assert_ne!(row_at(&after, Px::ZERO), five_hundred);
        }
    }

    mod transform_tests {
        use crate::context::Cx;
        use crate::div::div;
        use crate::element::Element;
        use guirs_core::{Point, Px, Rgba, ScaleFactor, Size};
        use guirs_style::{StyleEngine, Styled};

        /// Paint one tree and hand back the frame it produced.
        fn frame(mut root: crate::div::Div, size: Size<Px>) -> Cx {
            let mut cx = Cx::new(guirs_text::TextSystem::new(), StyleEngine::default());
            cx.begin_frame(size, ScaleFactor(1.0), 0.0);
            let node = root.request_layout(&mut cx);
            cx.layout.compute(node, size, &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);
            cx.end_frame();
            cx
        }

        fn hit(cx: &Cx, x: f32, y: f32) -> bool {
            cx.hits
                .iter()
                .any(|region| region.contains(Point::new(Px(x), Px(y))))
        }

        #[test]
        fn a_translated_element_is_clickable_where_it_appears() {
            let cx = frame(
                div()
                    .w(Px(50.0))
                    .h(Px(50.0))
                    .bg(Rgba::hex(0x336699))
                    .translate(Px(100.0), Px(0.0)),
                Size::new(Px(400.0), Px(200.0)),
            );

            // Where it was drawn.
            assert!(hit(&cx, 120.0, 25.0));
            // Where it was laid out, which is now empty.
            assert!(!hit(&cx, 20.0, 25.0));
        }

        #[test]
        fn a_scaled_element_is_clickable_over_the_size_it_grew_to() {
            let cx = frame(
                div()
                    .w(Px(50.0))
                    .h(Px(50.0))
                    .bg(Rgba::hex(0x336699))
                    .scale(2.0),
                Size::new(Px(400.0), Px(200.0)),
            );

            // Scaling is about the center, so the box now spans -25..75.
            assert!(hit(&cx, 70.0, 25.0));
            assert!(!hit(&cx, 80.0, 25.0));
        }

        #[test]
        fn an_element_scaled_to_nothing_cannot_be_clicked() {
            let cx = frame(
                div()
                    .w(Px(50.0))
                    .h(Px(50.0))
                    .bg(Rgba::hex(0x336699))
                    .scale(0.0),
                Size::new(Px(400.0), Px(200.0)),
            );
            assert!(!hit(&cx, 25.0, 25.0));
        }

        #[test]
        fn a_rotated_element_is_only_clickable_where_it_actually_is() {
            let cx = frame(
                div()
                    .w(Px(100.0))
                    .h(Px(20.0))
                    .bg(Rgba::hex(0x336699))
                    .rotate(90.0),
                Size::new(Px(400.0), Px(400.0)),
            );

            // A wide, short bar turned upright: tall and narrow about its
            // center at (50, 10).
            assert!(hit(&cx, 50.0, 55.0));
            // The corner of the box it was laid out in is no longer covered.
            assert!(!hit(&cx, 95.0, 5.0));
        }

        #[test]
        fn an_untransformed_element_hit_tests_exactly_as_before() {
            let cx = frame(
                div().w(Px(50.0)).h(Px(50.0)).bg(Rgba::hex(0x336699)),
                Size::new(Px(400.0), Px(200.0)),
            );
            assert!(cx.hits.iter().all(|region| region.transform.is_none()));
            assert!(hit(&cx, 25.0, 25.0));
            assert!(!hit(&cx, 75.0, 25.0));
        }

        #[test]
        fn a_transform_moves_the_children_with_it() {
            let cx = frame(
                div()
                    .w(Px(200.0))
                    .h(Px(100.0))
                    .translate(Px(100.0), Px(0.0))
                    .child(div().w(Px(40.0)).h(Px(40.0)).bg(Rgba::hex(0xff0000))),
                Size::new(Px(400.0), Px(200.0)),
            );

            let child = cx
                .hits
                .iter()
                .find(|region| region.bounds.size.width == Px(40.0))
                .expect("the child registered no hit region");
            assert!(child.transform.is_some());
            assert!(child.contains(Point::new(Px(120.0), Px(20.0))));
            assert!(!child.contains(Point::new(Px(20.0), Px(20.0))));
        }
    }

    use super::*;
    use crate::context::Cx;
    use guirs_style::StyleEngine;
    use guirs_text::TextSystem;

    fn frame(mut root: Div, size: Size<Px>, styles: StyleEngine) -> (Cx, Div) {
        let mut cx = Cx::new(TextSystem::new(), styles);
        cx.begin_frame(size, guirs_core::ScaleFactor(1.0), 0.0);
        let node = root.request_layout(&mut cx);
        cx.layout.compute(node, size, &mut cx.text);
        let bounds = cx.layout.bounds(node);
        root.paint(bounds, &mut cx);
        cx.end_frame();
        (cx, root)
    }

    #[test]
    fn a_filled_div_emits_one_quad() {
        let (cx, _) = frame(
            div().size(100.0).bg(Rgba::hex(0x336699)),
            Size::new(Px(200.0), Px(200.0)),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().quads, 1);
    }

    #[test]
    fn an_unstyled_div_paints_nothing_but_is_still_hit_testable() {
        let (cx, _) = frame(
            div().size(100.0),
            Size::new(Px(200.0), Px(200.0)),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().quads, 0);
        assert_eq!(cx.hits.len(), 1);
    }

    #[test]
    fn stylesheet_rules_reach_the_element() {
        let (cx, root) = frame(
            div().class("card").size(80.0),
            Size::new(Px(200.0), Px(200.0)),
            StyleEngine::from_source(".card { background: #112233; border-radius: 6px; }"),
        );
        assert_eq!(
            root.computed().background,
            Paint::Solid(Rgba::hex(0x112233))
        );
        assert_eq!(root.computed().border_radius.top_left, Px(6.0));
        assert_eq!(cx.scene.stats().quads, 1);
    }

    #[test]
    fn children_are_positioned_by_layout() {
        let root = div()
            .row()
            .size_full()
            .gap(10.0)
            .child(div().size(20.0).bg(Rgba::BLACK))
            .child(div().size(20.0).bg(Rgba::BLACK));

        let (cx, _) = frame(root, Size::new(Px(200.0), Px(50.0)), StyleEngine::default());
        assert_eq!(cx.scene.stats().quads, 2);
        // Parent, then the two children.
        assert_eq!(cx.hits.len(), 3);
        assert_eq!(cx.hits[1].bounds.origin.x, Px(0.0));
        assert_eq!(cx.hits[2].bounds.origin.x, Px(30.0));
    }

    #[test]
    fn a_shadow_adds_a_second_quad_behind_the_box() {
        let root = div().size(50.0).bg(Rgba::BLACK).shadow(guirs_core::BoxShadow::new(
            Point::new(Px(0.0), Px(4.0)),
            Px(12.0),
            Px(0.0),
            Rgba::rgba8(0, 0, 0, 128),
        ));
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());
        assert_eq!(cx.scene.stats().quads, 2);
    }

    #[test]
    fn hidden_elements_paint_nothing() {
        let root = div().hidden().size(50.0).bg(Rgba::BLACK);
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());
        assert_eq!(cx.scene.stats().quads, 0);
        assert!(cx.hits.is_empty());
    }

    #[test]
    fn overflow_hidden_clips_children() {
        let root = div()
            .size(50.0)
            .overflow_hidden()
            .child(div().size(500.0).bg(Rgba::BLACK));
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());

        let child_hit = &cx.hits[1];
        let clip = child_hit.clip.expect("the child should be clipped");
        assert!(clip.size.width <= Px(50.0));
    }

    #[test]
    fn a_scrollable_element_records_its_extent() {
        let root = div()
            .size(50.0)
            .overflow_scroll()
            .child(div().w(40.0).h(500.0).bg(Rgba::BLACK));
        let (cx, root) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());

        let scroll = cx.scroll_state(root.global_id());
        assert!(scroll.content.height > scroll.viewport.height);
        assert!(scroll.can_scroll_vertically());
        // The overlay scrollbar draws on top of the content.
        assert!(cx.scene.stats().quads >= 2);
    }

    #[test]
    fn focusable_elements_join_the_focus_order() {
        let root = div()
            .child(div().focusable().size(10.0))
            .child(div().size(10.0))
            .child(div().focusable().size(10.0));
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());
        assert_eq!(cx.focus_order.len(), 2);
    }

    #[test]
    fn handlers_are_registered_only_where_present() {
        let root = div()
            .child(div().size(10.0).on_click(|_, _| {}))
            .child(div().size(10.0));
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());
        assert_eq!(cx.handlers.len(), 1);
    }

    #[test]
    fn a_class_attribute_may_carry_several_names() {
        // No inline size here: an inline style beats the stylesheet, which
        // would hide whether the class was registered at all.
        let (_, root) = frame(
            div().class("avatar avatar-you"),
            Size::new(Px(50.0), Px(50.0)),
            StyleEngine::from_source(".avatar { width: 30px; } .avatar-you { background: #ff0000; }"),
        );
        // Both rules applied, so both names were registered.
        assert_eq!(root.computed().size.width, guirs_core::Length::Px(Px(30.0)));
        assert_eq!(
            root.computed().background,
            Paint::Solid(Rgba::hex(0xff0000))
        );
    }

    #[test]
    fn conditional_builders_do_not_break_the_chain() {
        let styled = div().size(40.0).when(true, |d| d.bg(Rgba::BLACK)).class_if(true, "on");
        let plain = div().size(40.0).when(false, |d| d.bg(Rgba::BLACK)).class_if(false, "on");

        let (cx_styled, _) = frame(styled, Size::new(Px(50.0), Px(50.0)), StyleEngine::default());
        let (cx_plain, _) = frame(plain, Size::new(Px(50.0), Px(50.0)), StyleEngine::default());
        assert_eq!(cx_styled.scene.stats().quads, 1);
        assert_eq!(cx_plain.scene.stats().quads, 0);
    }

    #[test]
    fn opacity_compounds_through_nesting() {
        let root = div()
            .opacity(0.5)
            .size(100.0)
            .bg(Rgba::BLACK)
            .child(div().opacity(0.5).size(50.0).bg(Rgba::BLACK));
        let (cx, _) = frame(root, Size::new(Px(200.0), Px(200.0)), StyleEngine::default());
        assert_eq!(cx.scene.stats().quads, 2);
    }
}
