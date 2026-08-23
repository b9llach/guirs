//! Input routing.
//!
//! Split out from the window so it can be tested without a GPU, because this is
//! where the subtle bugs live. Nothing here touches the renderer: it reads the
//! hit regions the last paint recorded and drives the retained input state.
//!
//! The central idea is that an event travels along a **chain**, not to a single
//! element. A press on a button lands on the button's label, because the label
//! is the innermost thing under the pointer. Treating that label as "the
//! target" breaks focus, breaks `:active` styling, breaks dragging, and drops
//! most clicks. Every routing decision here works on the chain instead.

use guirs_core::{Axis, GlobalElementId, Point, Px};
use guirs_style::CursorStyle;
use smallvec::SmallVec;

use crate::context::{Cx, EventContext, HitRegion, ScrollbarRegion, TextSelection};
use crate::event::{
    ClickTracker, FileDropEvent, ImeEvent, InputEvent, Key, MouseButton, MouseEvent,
};

/// Which callback a pointer event should reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Move,
    Down,
    Up,
    Click,
}

/// A scrollbar drag in progress.
#[derive(Clone, Copy, Debug)]
struct ScrollbarGrab {
    owner: GlobalElementId,
    axis: Axis,
    track_start: Px,
    track_len: Px,
    thumb_len: Px,
    /// Distance from the start of the thumb to where it was grabbed.
    grab: Px,
}

/// What the window should do after an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub redraw: bool,
    pub quit: bool,
}

/// Routes platform input into the element tree.
#[derive(Default)]
pub struct InputRouter {
    clicks: ClickTracker,
    /// Everything under the pointer when a button went down, innermost first.
    /// Held for the duration of the press, which is what gives drags their
    /// pointer capture.
    press_chain: SmallVec<[HitRegion; 8]>,
    /// Where the caret was horizontally when vertical movement started.
    ///
    /// Walking down through a short line and back up should return to the
    /// column it set out from rather than to the end of the short line, so the
    /// column outlives the lines it passes through. Cleared by anything that
    /// moves the caret some other way.
    caret_goal: Option<Px>,
    scrollbar_grab: Option<ScrollbarGrab>,
    cursor: CursorStyle,
    /// The element whose text the pointer is currently sweeping over.
    selecting: Option<GlobalElementId>,
    /// The field whose selection the pointer is currently sweeping out.
    editing: Option<GlobalElementId>,
    /// Created on first use: opening the clipboard is not free, and most
    /// applications never touch it.
    clipboard: Option<arboard::Clipboard>,
}

impl std::fmt::Debug for InputRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputRouter")
            .field("pressed", &self.is_pressed())
            .field("selecting", &self.selecting)
            .finish()
    }
}

impl InputRouter {
    pub fn new() -> Self {
        InputRouter::default()
    }

    /// Put text on the system clipboard.
    pub fn copy_to_clipboard(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => self.clipboard = Some(clipboard),
                Err(error) => {
                    log::warn!("clipboard unavailable: {error}");
                    return;
                }
            }
        }
        if let Some(clipboard) = &mut self.clipboard {
            if let Err(error) = clipboard.set_text(text.to_owned()) {
                log::warn!("could not copy: {error}");
            }
        }
    }

    /// Read text from the system clipboard.
    pub fn paste_from_clipboard(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()?.get_text().ok()
    }

    /// The pointer shape for whatever is currently under the pointer.
    #[inline]
    pub fn cursor(&self) -> CursorStyle {
        self.cursor
    }

    /// Whether a press or a scrollbar drag is in progress.
    #[inline]
    pub fn is_pressed(&self) -> bool {
        !self.press_chain.is_empty() || self.scrollbar_grab.is_some()
    }

    /// Record a press for multi click detection, returning the click count.
    pub fn track_click(&mut self, position: Point<Px>, now: f64) -> u32 {
        self.clicks.press(position, now)
    }

    /// Re-evaluate hover and the cursor after a repaint moved things around.
    pub fn refresh_hover(&mut self, cx: &mut Cx) -> bool {
        if !cx.input.mouse_inside {
            return false;
        }
        let position = cx.input.mouse_position;
        let hovered = cx.hovered_at(position);
        let changed = hovered != cx.input.hovered;
        if changed {
            cx.input.hovered = hovered;
        }
        self.cursor = cursor_for(cx, position);
        changed
    }

    /// Route one event.
    pub fn handle(&mut self, cx: &mut Cx, event: &InputEvent) -> Outcome {
        let mut out = Outcome::default();

        match event {
            InputEvent::MouseMove(mouse) => {
                cx.input.mouse_position = mouse.position;
                cx.input.mouse_inside = true;
                cx.input.modifiers = mouse.modifiers;

                // Follow whatever is being carried, and work out where it
                // would land. Done before anything else claims the move: a
                // drag under way owns the pointer until it is let go.
                if cx.input.drag.moved(mouse.position) {
                    let target = drop_target_at(cx, mouse.position);
                    if cx.input.drop_target != target {
                        cx.input.drop_target = target;
                    }
                    out.redraw = true;
                    return out;
                }

                if self.scrollbar_grab.is_some() {
                    self.drag_scrollbar(cx, mouse.position, &mut out);
                } else if let Some(id) = self.editing {
                    // Sweeping out a selection inside a field. The anchor stays
                    // where the press landed and the caret follows the pointer.
                    if let Some(editor) = cx.editors.get(&id).cloned() {
                        let index = cx.editor_index_at(&editor, mouse.position);
                        editor.state.update(|state| {
                            // The layout covers the displayed text, which while
                            // an input method is composing is not the value.
                            let index = state.value_offset(index);
                            if state.anchor.is_none() {
                                state.anchor = Some(state.cursor);
                            }
                            if state.cursor != index {
                                state.cursor = index;
                            }
                        });
                        out.redraw = true;
                    }
                } else if self.selecting.is_some() {
                    // Sweeping out a selection. The end moves, the start does
                    // not, and the pointer may wander out of the run it began
                    // in and into another one entirely.
                    if let Some((run, index)) = cx.selection_point_at(mouse.position) {
                        if let Some(selection) = &mut cx.input.text_selection {
                            if selection.head_run != run || selection.head != index {
                                selection.head_run = run;
                                selection.head = index;
                                out.redraw = true;
                            }
                        }
                    }
                } else if !self.press_chain.is_empty() {
                    // Captured: moves belong to whatever was pressed, all the
                    // way up its chain, even once the pointer has left it.
                    let chain = self.press_chain.clone();
                    dispatch(cx, &chain, mouse, Phase::Move, &mut out);
                } else {
                    let hovered = cx.hovered_at(mouse.position);
                    if hovered != cx.input.hovered {
                        cx.input.hovered = hovered;
                        out.redraw = true;
                    }
                    // The pointer moved, so whatever it was resting on it is
                    // no longer resting on. Restarted rather than kept, or a
                    // tooltip would appear over something the pointer has
                    // merely passed across.
                    let innermost = cx.chain_at(mouse.position).first().map(|r| r.id);
                    let now = cx.now;
                    match (&mut cx.input.resting, innermost) {
                        (Some((id, _)), Some(under)) if *id == under => {}
                        (_, Some(under)) => cx.input.resting = Some((under, now)),
                        (_, None) => cx.input.resting = None,
                    }
                    self.cursor = cursor_for(cx, mouse.position);
                    let chain = cx.chain_at(mouse.position);
                    dispatch(cx, &chain, mouse, Phase::Move, &mut out);
                }
            }

            InputEvent::MouseDown(mouse) => {
                if let Some(button) = mouse.button {
                    if !cx.input.buttons.contains(&button) {
                        cx.input.buttons.push(button);
                    }
                }
                cx.input.modifiers = mouse.modifiers;
                cx.input.mouse_position = mouse.position;
                // Pressing anything puts a tooltip away: it has been read, or
                // it is in the way of whatever is being done now.
                cx.input.resting = None;
                out.redraw = true;

                // Anything open that this press did not land inside gets closed
                // first, so a dropdown does not stay open behind whatever was
                // clicked next.
                self.dismiss_elsewhere(cx, mouse.position, &mut out);

                // Scrollbars float over their content, so they are consulted
                // before anything the pointer is nominally over.
                if mouse.button == Some(MouseButton::Left) {
                    if let Some(bar) = cx.scrollbar_at(mouse.position) {
                        self.grab_scrollbar(cx, bar, mouse.position, &mut out);
                        return out;
                    }
                }

                let chain = cx.chain_at(mouse.position);
                self.press_chain = chain.clone();
                cx.input.active = chain.iter().map(|region| region.id).collect();

                // Noted rather than started. Whether this press is a drag or a
                // click is not known until the pointer either moves or does
                // not, so both stay possible until then.
                if mouse.button == Some(MouseButton::Left) {
                    if let Some((id, kind, value)) = chain.iter().find_map(|region| {
                        let handlers = cx.handlers.get(&region.id)?;
                        let (kind, value) = handlers.draggable.as_ref()?;
                        Some((region.id, kind.clone(), value.clone()))
                    }) {
                        cx.input.drag.press(kind, value, id, mouse.position);
                    }
                }

                // Focus goes to the nearest focusable element in the chain, not
                // to whatever happens to be innermost. A press on a text
                // field's own text must focus the field, not blur it.
                match chain.iter().find(|region| region.focusable) {
                    Some(region) => {
                        cx.input.focused = Some(region.id);
                        cx.input.focus_visible = false;
                    }
                    None => cx.input.focused = None,
                }

                self.selecting = None;
                self.editing = None;

                if let Some((id, editor)) = cx.editor_at(mouse.position) {
                    // A press inside a field places its caret. Fields are
                    // checked before selectable text, because a field's own
                    // text is selectable too and it is the field's state that
                    // has to move.
                    let index = cx.editor_index_at(&editor, mouse.position);
                    editor.state.update(|state| {
                        // A press elsewhere abandons whatever was being
                        // composed, the same as it does in a native field.
                        state.clear_preedit();
                        let index = state.value_offset(index);
                        match mouse.click_count {
                            0 | 1 => {
                                state.anchor = None;
                                state.cursor = index;
                            }
                            2 => {
                                let word = editor.layout.word_at(index);
                                state.anchor = Some(state.value_offset(word.start));
                                state.cursor = state.value_offset(word.end);
                            }
                            _ => state.select_all(),
                        }
                    });
                    self.editing = Some(id);
                    cx.input.text_selection = None;
                } else if let Some((id, text)) = cx.selectable_at(mouse.position) {
                    let local = Point::new(
                        mouse.position.x - text.origin.x,
                        mouse.position.y - text.origin.y,
                    );
                    let index = text.layout.index_for_position(local);
                    // One click places the caret, two take the word, three take
                    // the line.
                    let (anchor, head) = match mouse.click_count {
                        0 | 1 => (index, index),
                        2 => {
                            let word = text.layout.word_at(index);
                            (word.start, word.end)
                        }
                        _ => {
                            let line = text.layout.line_range_at(index);
                            (line.start, line.end)
                        }
                    };
                    let run = cx.selectable.get(&id).map(|text| text.order).unwrap_or(0);
                    cx.input.text_selection = Some(TextSelection {
                        id,
                        anchor,
                        head,
                        anchor_run: run,
                        head_run: run,
                    });
                    self.selecting = Some(id);
                } else {
                    cx.input.text_selection = None;
                }

                dispatch(cx, &chain, mouse, Phase::Down, &mut out);
            }

            InputEvent::MouseUp(mouse) => {
                if let Some(button) = mouse.button {
                    cx.input.buttons.retain(|b| *b != button);
                }
                cx.input.mouse_position = mouse.position;
                out.redraw = true;

                if self.scrollbar_grab.take().is_some() {
                    return out;
                }

                // A drag that was under way ends here rather than becoming a
                // click. Releasing over nothing that accepts it drops nothing,
                // which is how a drag is cancelled.
                if let Some(dropped) = cx.input.drag.release() {
                    let target = cx.input.drop_target.take();
                    if let Some(target) = target {
                        deliver_drop(cx, target, &dropped, &mut out);
                    }
                    self.press_chain.clear();
                    cx.input.active.clear();
                    out.redraw = true;
                    return out;
                }
                cx.input.drag.cancel();
                cx.input.drop_target = None;

                let release = cx.chain_at(mouse.position);
                dispatch(cx, &release, mouse, Phase::Up, &mut out);

                // A click fires on the deepest element common to the press and
                // the release, then bubbles outward from there. Requiring the
                // innermost pressed element to still be under the pointer would
                // drop most real clicks: a press usually lands on a label, and
                // a release a pixel later can land on its parent's padding.
                if mouse.button == Some(MouseButton::Left) {
                    let press = std::mem::take(&mut self.press_chain);
                    if let Some(index) = press
                        .iter()
                        .position(|p| release.iter().any(|r| r.id == p.id))
                    {
                        dispatch(cx, &press[index..], mouse, Phase::Click, &mut out);
                    }
                }

                self.press_chain.clear();
                cx.input.active.clear();
                self.selecting = None;
                self.editing = None;
            }

            InputEvent::MouseLeave => {
                cx.input.mouse_inside = false;
                if !cx.input.hovered.is_empty() {
                    cx.input.hovered.clear();
                    cx.input.resting = None;
                    out.redraw = true;
                }
                // A press in progress keeps its capture: leaving the window
                // mid drag must not abandon the drag.
                if self.press_chain.is_empty() {
                    cx.input.active.clear();
                }
                self.cursor = CursorStyle::Default;
            }

            InputEvent::Scroll(scroll) => {
                cx.input.mouse_position = scroll.position;
                wheel(cx, scroll.position, scroll.delta, &mut out);

                for region in cx.chain_at(scroll.position) {
                    let Some(handler) =
                        cx.handlers.get(&region.id).and_then(|h| h.on_scroll.clone())
                    else {
                        continue;
                    };
                    let mut context = EventContext::new(
                        &mut cx.input,
                        &mut cx.state,
                        &mut out.redraw,
                        &mut out.quit,
                    )
                    .for_target(region.id);
                    handler(scroll, &mut context);
                    if !context.propagating() {
                        break;
                    }
                }
            }

            InputEvent::KeyDown(key) => {
                cx.input.modifiers = key.modifiers;

                // Clipboard work belongs to the window rather than to any one
                // element: it needs the focused field, the system clipboard,
                // and in the case of cut, both at once.
                if key.modifiers.accelerator() {
                    if let Some(editor) = cx.focused_editor() {
                        if self.handle_editor_shortcut(key, &editor, &mut out) {
                            return out;
                        }
                    }
                    // Nothing focused that edits, so fall back to copying the
                    // selection in whatever text is on screen.
                    if key.is_shortcut('c') {
                        if let Some(text) = cx.selected_text() {
                            self.copy_to_clipboard(&text);
                            return out;
                        }
                    }
                }

                // Moving between lines needs to know where the lines are,
                // which only the laid out text knows and no element has during
                // an event. So the window does it, the same way it turns a
                // click into a caret.
                if let Some(editor) = cx.focused_editor() {
                    if self.handle_editor_navigation(key, &editor, &mut out) {
                        return out;
                    }
                }

                if key.key == Key::Tab {
                    move_focus(cx, !key.modifiers.shift);
                    out.redraw = true;
                } else if let Some(focused) = cx.input.focused {
                    let handler = cx.handlers.get(&focused).and_then(|h| h.on_key_down.clone());
                    if let Some(handler) = handler {
                        let mut context = EventContext::new(
                            &mut cx.input,
                            &mut cx.state,
                            &mut out.redraw,
                            &mut out.quit,
                        )
                        .for_target(focused);
                        handler(key, &mut context);
                    }
                    out.redraw = true;
                }
            }

            InputEvent::KeyUp(key) => {
                cx.input.modifiers = key.modifiers;
            }

            InputEvent::TextInput(input) => {
                if let Some(focused) = cx.input.focused {
                    let handler = cx.handlers.get(&focused).and_then(|h| h.on_text.clone());
                    if let Some(handler) = handler {
                        let mut context = EventContext::new(
                            &mut cx.input,
                            &mut cx.state,
                            &mut out.redraw,
                            &mut out.quit,
                        )
                        .for_target(focused);
                        handler(input, &mut context);
                        out.redraw = true;
                    }
                }
            }

            InputEvent::Ime(ime) => {
                // Composition belongs to whatever has the keyboard, the same
                // as a keystroke does.
                let Some(id) = cx.input.focused else {
                    return out;
                };
                let Some(editor) = cx.editors.get(&id).cloned() else {
                    return out;
                };

                editor.state.update(|state| match ime {
                    ImeEvent::Enabled => {}
                    ImeEvent::Preedit { text, cursor } => state.set_preedit(text, *cursor),
                    ImeEvent::Commit(text) => state.commit(text),
                    // Whatever was half composed never happened.
                    ImeEvent::Disabled => state.clear_preedit(),
                });
                out.redraw = true;
            }

            InputEvent::FileDrop(drop) => {
                cx.input.mouse_position = drop.position;
                cx.input.hovering_files = false;
                let chain = cx.chain_at(drop.position);
                dispatch_files(cx, &chain, drop, false, &mut out);
                out.redraw = true;
            }

            InputEvent::FileHover(drop) => {
                cx.input.mouse_position = drop.position;
                cx.input.hovering_files = true;
                let chain = cx.chain_at(drop.position);
                dispatch_files(cx, &chain, drop, true, &mut out);
                out.redraw = true;
            }

            InputEvent::FileHoverCancelled => {
                if cx.input.hovering_files {
                    cx.input.hovering_files = false;
                    // Everything that was told files were coming is told they
                    // are not, so a lit up drop target goes dark again.
                    let chain = cx.chain_at(cx.input.mouse_position);
                    let empty = FileDropEvent {
                        position: cx.input.mouse_position,
                        ..Default::default()
                    };
                    dispatch_files(cx, &chain, &empty, true, &mut out);
                }
                out.redraw = true;
            }

            InputEvent::WindowFocus(focused) => {
                cx.input.window_focused = *focused;
                if !focused {
                    self.press_chain.clear();
                    self.scrollbar_grab = None;
                    cx.input.active.clear();
                    cx.input.hovered.clear();
                }
                out.redraw = true;
            }
        }

        out
    }

    /// Apply a clipboard or selection shortcut to the focused field.
    ///
    /// Returns whether the key was consumed.
    /// Caret movement that depends on how the text was laid out.
    ///
    /// Returns whether the key was consumed, in which case the field's own
    /// handler does not also see it.
    fn handle_editor_navigation(
        &mut self,
        key: &crate::event::KeyEvent,
        editor: &crate::context::TextEditor,
        out: &mut Outcome,
    ) -> bool {
        use crate::event::Key;

        let layout = &editor.layout;
        let extend = key.modifiers.shift;

        // Any movement that is not up or down abandons the remembered column.
        if !matches!(key.key, Key::Up | Key::Down) || !editor.multiline {
            self.caret_goal = None;
        }

        match key.key {
            // Only where there are lines to move between. A single line field
            // leaves the arrows alone, because an application is entitled to
            // use them for something else, a history of what was typed being
            // the usual thing.
            Key::Up | Key::Down if editor.multiline => {
                let composing = editor.state.with(|state| state.is_composing());
                if composing {
                    // The input method is using the arrows to move through its
                    // own candidates.
                    return true;
                }

                let cursor = editor.state.with(|state| state.display_cursor());
                let here = layout.position_for_index(cursor);
                let goal = *self.caret_goal.get_or_insert(here.x);

                let line = layout.line_at(here.y);
                let target = match key.key {
                    Key::Up => {
                        if line == 0 {
                            // Already on the first line: go to its start, which
                            // is what every editor does.
                            editor.state.update(|state| state.move_home(extend));
                            out.redraw = true;
                            return true;
                        }
                        line - 1
                    }
                    _ => {
                        if line + 1 >= layout.line_count() {
                            editor.state.update(|state| state.move_end(extend));
                            out.redraw = true;
                            return true;
                        }
                        line + 1
                    }
                };

                let Some(target_line) = layout.lines.get(target) else {
                    return true;
                };
                // Aim at the remembered column on the target line's own row.
                let index = layout.index_for_position(guirs_core::Point::new(
                    goal,
                    target_line.origin.y + target_line.height * 0.5,
                ));
                editor.state.update(|state| {
                    let index = state.value_offset(index);
                    state.set_caret(index, extend);
                });
                out.redraw = true;
                true
            }

            // Home and End are per line once there is more than one.
            Key::Home | Key::End if layout.line_count() > 1 => {
                let cursor = editor.state.with(|state| state.display_cursor());
                let range = layout.line_range_at(cursor);
                let target = if key.key == Key::Home {
                    range.start
                } else {
                    // A wrapped line's end includes the space it broke at, and
                    // putting the caret past it would land on the next line.
                    trim_line_end(&layout.text, range.clone())
                };
                editor.state.update(|state| {
                    let target = state.value_offset(target);
                    state.set_caret(target, extend);
                });
                out.redraw = true;
                true
            }

            _ => false,
        }
    }

    fn handle_editor_shortcut(
        &mut self,
        key: &crate::event::KeyEvent,
        editor: &crate::context::TextEditor,
        out: &mut Outcome,
    ) -> bool {
        let selected = |state: &crate::widgets::TextInputState| -> Option<String> {
            let range = state.selection()?;
            state.value.get(range).map(|text| text.to_string())
        };

        if key.is_shortcut('a') {
            editor.state.update(|state| state.select_all());
            out.redraw = true;
            return true;
        }

        if key.is_shortcut('c') {
            if let Some(text) = editor.state.with(selected) {
                self.copy_to_clipboard(&text);
            }
            return true;
        }

        if key.is_shortcut('x') {
            let cut = editor.state.with(selected);
            if let Some(text) = cut {
                self.copy_to_clipboard(&text);
                editor.state.update(|state| {
                    state.delete_selection();
                });
                out.redraw = true;
            }
            return true;
        }

        if key.is_shortcut('v') {
            if let Some(text) = self.paste_from_clipboard() {
                // A single line field cannot hold a newline, so pasting one
                // raw would put a character in the value that never renders
                // and that the caret cannot get past. An area keeps them.
                let text = if editor.multiline {
                    text.replace("\r\n", "\n").replace('\r', "\n")
                } else {
                    text.replace(['\n', '\r'], " ")
                };
                if !text.is_empty() {
                    editor.state.update(|state| state.insert(&text));
                    out.redraw = true;
                }
            }
            return true;
        }

        false
    }

    /// Tell every registered element whose subtree this press missed.
    fn dismiss_elsewhere(&mut self, cx: &mut Cx, point: Point<Px>, out: &mut Outcome) {
        if cx.dismiss.is_empty() {
            return;
        }
        let involved = cx.ancestry_at(point);
        let pending: SmallVec<[crate::context::DismissHandler; 4]> = cx
            .dismiss
            .iter()
            .filter(|region| !involved.contains(&region.id))
            .map(|region| region.handler.clone())
            .collect();

        for handler in pending {
            let mut context =
                EventContext::new(&mut cx.input, &mut cx.state, &mut out.redraw, &mut out.quit);
            handler(&mut context);
            out.redraw = true;
        }
    }

    /// Begin dragging a scrollbar.
    ///
    /// Pressing the thumb picks it up where it was grabbed. Pressing the track
    /// jumps the thumb to the pointer and drags from its middle, which is what
    /// a modern scrollbar does and saves a separate press and drag.
    fn grab_scrollbar(
        &mut self,
        cx: &mut Cx,
        bar: ScrollbarRegion,
        point: Point<Px>,
        out: &mut Outcome,
    ) {
        let (track_start, track_len, thumb_start, thumb_len) = match bar.axis {
            Axis::Vertical => (
                bar.track.top(),
                bar.track.height(),
                bar.thumb.top(),
                bar.thumb.height(),
            ),
            Axis::Horizontal => (
                bar.track.left(),
                bar.track.width(),
                bar.thumb.left(),
                bar.thumb.width(),
            ),
        };

        let grab = if bar.thumb.contains(point) {
            point.along(bar.axis) - thumb_start
        } else {
            thumb_len * 0.5
        };

        self.scrollbar_grab = Some(ScrollbarGrab {
            owner: bar.owner,
            axis: bar.axis,
            track_start,
            track_len,
            thumb_len,
            grab,
        });
        self.drag_scrollbar(cx, point, out);
    }

    fn drag_scrollbar(&mut self, cx: &mut Cx, point: Point<Px>, out: &mut Outcome) {
        let Some(grab) = self.scrollbar_grab else {
            return;
        };
        let travel = grab.track_len - grab.thumb_len;
        if travel <= Px::ZERO {
            return;
        }

        let offset = point.along(grab.axis) - grab.track_start - grab.grab;
        let fraction = (offset / travel).clamp(0.0, 1.0);

        let mut state = cx.scroll_state(grab.owner);
        let max = state.max_offset();
        match grab.axis {
            Axis::Vertical => state.offset.y = max.y * fraction,
            Axis::Horizontal => state.offset.x = max.x * fraction,
        }
        state.clamp();
        cx.set_scroll_state(grab.owner, state);
        out.redraw = true;
    }
}

/// Send a pointer event along a chain, innermost first, until something stops
/// it.
/// The nearest element under the pointer that accepts what is being carried.
///
/// Innermost first, so a target inside another gets the drop. A target that
/// does not accept this kind is passed over entirely rather than blocking the
/// one behind it, which is what lets a list of tabs sit inside a pane that
/// also accepts drops.
fn drop_target_at(cx: &Cx, position: Point<Px>) -> Option<GlobalElementId> {
    let kind = match cx.input.drag.dragged() {
        Some(dragged) => dragged.kind.clone(),
        None => return None,
    };
    cx.chain_at(position).iter().find_map(|region| {
        let handlers = cx.handlers.get(&region.id)?;
        handlers
            .on_drop
            .iter()
            .any(|(accepts, _)| *accepts == kind)
            .then_some(region.id)
    })
}

/// Hand what was carried to the element it was dropped on.
fn deliver_drop(
    cx: &mut Cx,
    target: GlobalElementId,
    dropped: &crate::drag::Dragged,
    out: &mut Outcome,
) {
    let Some(handlers) = cx.handlers.get(&target) else {
        return;
    };
    let handler = handlers
        .on_drop
        .iter()
        .find(|(accepts, _)| *accepts == dropped.kind)
        .map(|(_, handler)| handler.clone());
    let Some(handler) = handler else {
        return;
    };

    let mut context = EventContext::new(
        &mut cx.input,
        &mut cx.state,
        &mut out.redraw,
        &mut out.quit,
    );
    handler(dropped, &mut context);
}

fn dispatch(
    cx: &mut Cx,
    chain: &[HitRegion],
    mouse: &MouseEvent,
    phase: Phase,
    out: &mut Outcome,
) {
    for region in chain {
        let handler = cx.handlers.get(&region.id).and_then(|h| match phase {
            Phase::Move => h.on_mouse_move.clone(),
            Phase::Down => h.on_mouse_down.clone(),
            Phase::Up => h.on_mouse_up.clone(),
            Phase::Click => h.on_click.clone(),
        });
        let Some(handler) = handler else {
            continue;
        };
        let event = mouse.relative_to(region.bounds);
        let mut context =
            EventContext::new(&mut cx.input, &mut cx.state, &mut out.redraw, &mut out.quit)
                .for_target(region.id);
        handler(&event, &mut context);
        if !context.propagating() {
            break;
        }
    }
}

/// Hand a file drop or hover down the chain under the pointer.
fn dispatch_files(
    cx: &mut Cx,
    chain: &[HitRegion],
    drop: &FileDropEvent,
    hovering: bool,
    out: &mut Outcome,
) {
    for region in chain {
        let handler = cx.handlers.get(&region.id).and_then(|handlers| {
            if hovering {
                handlers.on_file_hover.clone()
            } else {
                handlers.on_file_drop.clone()
            }
        });
        let Some(handler) = handler else {
            continue;
        };
        let mut context =
            EventContext::new(&mut cx.input, &mut cx.state, &mut out.redraw, &mut out.quit)
                .for_target(region.id);
        handler(drop, &mut context);
        if !context.propagating() {
            break;
        }
    }
}

/// A line's end, without the whitespace the wrap swallowed.
///
/// A line that broke at a space owns that space, and a caret placed after it
/// draws on the next line, which is not where End should put it.
fn trim_line_end(text: &str, range: std::ops::Range<usize>) -> usize {
    let slice = &text[range.clone()];
    range.start + slice.trim_end_matches([' ', '\t', '\n', '\r']).len()
}

fn cursor_for(cx: &Cx, point: Point<Px>) -> CursorStyle {
    if cx.scrollbar_at(point).is_some() {
        return CursorStyle::Default;
    }
    cx.cursor_at(point)
}

/// Move a scrollable ancestor, choosing the innermost one that can actually
/// move in the requested direction.
///
/// Without that check, an inner list already at its end would swallow the
/// gesture and the page behind it would refuse to move.
fn wheel(cx: &mut Cx, point: Point<Px>, delta: Point<Px>, out: &mut Outcome) {
    let candidates: SmallVec<[GlobalElementId; 4]> = cx
        .chain_at(point)
        .into_iter()
        .filter(|region| region.scrollable)
        .map(|region| region.id)
        .collect();

    for id in candidates {
        let mut state = cx.scroll_state(id);
        let max = state.max_offset();
        let can_move_y = (delta.y > Px::ZERO && state.offset.y < max.y)
            || (delta.y < Px::ZERO && state.offset.y > Px::ZERO);
        let can_move_x = (delta.x > Px::ZERO && state.offset.x < max.x)
            || (delta.x < Px::ZERO && state.offset.x > Px::ZERO);
        if !can_move_x && !can_move_y {
            continue;
        }

        state.offset.x += delta.x;
        state.offset.y += delta.y;
        state.clamp();
        cx.set_scroll_state(id, state);
        out.redraw = true;
        return;
    }
}

/// Move keyboard focus to the next or previous focusable element.
fn move_focus(cx: &mut Cx, forward: bool) {
    let order = cx.focus_order.clone();
    if order.is_empty() {
        return;
    }
    let current = cx
        .input
        .focused
        .and_then(|focused| order.iter().position(|id| *id == focused));
    let next = match current {
        Some(index) => {
            if forward {
                (index + 1) % order.len()
            } else {
                (index + order.len() - 1) % order.len()
            }
        }
        None => {
            if forward {
                0
            } else {
                order.len() - 1
            }
        }
    };
    cx.input.focused = Some(order[next]);
    // Focus that arrived from the keyboard should be visibly ringed.
    cx.input.focus_visible = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::div::div;
    use crate::element::{AnyElement, Element, IntoElement};
    use crate::model::Model;
    use crate::text::text;
    use crate::widgets::{
        button, checkbox, scroll_view, slider, text_area, text_input, TextInputState,
    };
    use guirs_core::{ScaleFactor, Size};
    use guirs_style::{StyleEngine, Styled};
    use guirs_text::TextSystem;

    /// Drives a real element tree and a real router with no GPU.
    struct Harness {
        cx: Cx,
        router: InputRouter,
        size: Size<Px>,
    }

    impl Harness {
        fn new() -> Self {
            Harness::styled(StyleEngine::default())
        }

        fn styled(styles: StyleEngine) -> Self {
            Harness {
                cx: Cx::new(TextSystem::new(), styles),
                router: InputRouter::new(),
                size: Size::new(Px(400.0), Px(400.0)),
            }
        }

        fn frame(&mut self, root: AnyElement) {
            self.frame_at(0.0, root);
        }

        /// A frame at a particular moment, for anything that waits.
        fn frame_at(&mut self, now: f64, mut root: AnyElement) {
            self.cx.begin_frame(self.size, ScaleFactor(1.0), now);
            let node = root.request_layout(&mut self.cx);
            self.cx.layout.compute(node, self.size, &mut self.cx.text);
            let bounds = self.cx.layout.bounds(node);
            root.paint(bounds, &mut self.cx);
            self.cx.end_frame();
        }

        fn at(x: f32, y: f32) -> MouseEvent {
            MouseEvent {
                position: Point::new(Px(x), Px(y)),
                button: Some(MouseButton::Left),
                ..Default::default()
            }
        }

        fn press(&mut self, x: f32, y: f32) {
            self.router
                .handle(&mut self.cx, &InputEvent::MouseDown(Self::at(x, y)));
        }

        fn release(&mut self, x: f32, y: f32) {
            self.router
                .handle(&mut self.cx, &InputEvent::MouseUp(Self::at(x, y)));
        }

        fn drag_to(&mut self, x: f32, y: f32) {
            self.router.handle(
                &mut self.cx,
                &InputEvent::MouseMove(MouseEvent {
                    position: Point::new(Px(x), Px(y)),
                    button: None,
                    ..Default::default()
                }),
            );
        }

        fn click(&mut self, x: f32, y: f32) {
            self.press(x, y);
            self.release(x, y);
        }

        fn drop_files(&mut self, x: f32, y: f32, paths: &[&str]) {
            self.router.handle(
                &mut self.cx,
                &InputEvent::FileDrop(FileDropEvent {
                    paths: paths.iter().map(std::path::PathBuf::from).collect(),
                    position: Point::new(Px(x), Px(y)),
                    ..Default::default()
                }),
            );
        }

        fn hover_files(&mut self, x: f32, y: f32, paths: &[&str]) {
            self.router.handle(
                &mut self.cx,
                &InputEvent::FileHover(FileDropEvent {
                    paths: paths.iter().map(std::path::PathBuf::from).collect(),
                    position: Point::new(Px(x), Px(y)),
                    ..Default::default()
                }),
            );
        }
    }

    #[test]
    fn a_drop_reaches_the_element_under_the_pointer() {
        let dropped: Model<Vec<String>> = Model::new(Vec::new());
        let record = dropped.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .w(px_(100.0))
                        .h(px_(100.0))
                        .on_file_drop(move |event, _| {
                            record.update(|names| {
                                names.extend(
                                    event
                                        .paths
                                        .iter()
                                        .map(|path| path.display().to_string()),
                                )
                            })
                        }),
                )
                .into_any(),
        );

        harness.drop_files(50.0, 50.0, &["a.txt", "b.txt"]);
        // Both files arrive together, because a drop of two files is one
        // gesture rather than two.
        assert_eq!(dropped.read().len(), 2);

        // And a drop outside the target is not its business.
        harness.drop_files(300.0, 300.0, &["c.txt"]);
        assert_eq!(dropped.read().len(), 2);
    }

    #[test]
    fn a_drop_travels_up_the_chain_it_landed_in() {
        let inner_hits = Model::new(0i32);
        let outer_hits = Model::new(0i32);
        let inner = inner_hits.clone();
        let outer = outer_hits.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_file_drop(move |_, _| outer.update(|n| *n += 1))
                .child(
                    div()
                        .w(px_(100.0))
                        .h(px_(100.0))
                        .on_file_drop(move |_, _| inner.update(|n| *n += 1)),
                )
                .into_any(),
        );

        harness.drop_files(50.0, 50.0, &["a.txt"]);
        // The innermost element handles it, and the container behind it hears
        // about it too unless the handler says otherwise.
        assert_eq!(inner_hits.get(), 1);
        assert_eq!(outer_hits.get(), 1);
    }

    #[test]
    fn a_handler_can_keep_a_drop_to_itself() {
        let outer_hits = Model::new(0i32);
        let outer = outer_hits.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_file_drop(move |_, _| outer.update(|n| *n += 1))
                .child(
                    div()
                        .w(px_(100.0))
                        .h(px_(100.0))
                        .on_file_drop(|_, cx| cx.stop_propagation()),
                )
                .into_any(),
        );

        harness.drop_files(50.0, 50.0, &["a.txt"]);
        assert_eq!(outer_hits.get(), 0);
    }

    #[test]
    fn hovering_files_is_announced_and_then_withdrawn() {
        let hovering = Model::new(false);
        let lit = hovering.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .w(px_(100.0))
                        .h(px_(100.0))
                        .on_file_hover(move |event, _| lit.set(!event.paths.is_empty())),
                )
                .into_any(),
        );

        harness.hover_files(50.0, 50.0, &["a.txt"]);
        assert!(hovering.get());
        assert!(harness.cx.input.hovering_files);

        harness
            .router
            .handle(&mut harness.cx, &InputEvent::FileHoverCancelled);
        // The target is told the files are gone, so it can go dark again.
        assert!(!hovering.get());
        assert!(!harness.cx.input.hovering_files);
    }

    #[test]
    fn a_drop_clears_the_hover_state() {
        let mut harness = Harness::new();
        harness.frame(div().size_full().into_any());

        harness.hover_files(10.0, 10.0, &["a.txt"]);
        assert!(harness.cx.input.hovering_files);
        harness.drop_files(10.0, 10.0, &["a.txt"]);
        assert!(!harness.cx.input.hovering_files);
    }

    #[test]
    fn opening_a_window_is_queued_like_any_other_window_request() {
        use crate::app::WindowOptions;

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_click(|_, cx| {
                    cx.open_window(WindowOptions::new("Preferences").sized(420.0, 520.0), |_| {
                        div().into_any()
                    })
                })
                .into_any(),
        );

        harness.click(50.0, 50.0);
        let actions = &harness.cx.input.window_actions;
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            crate::event::WindowAction::Open(spec) => {
                assert_eq!(spec.options.title, "Preferences");
                assert_eq!(spec.options.width, 420.0);
            }
            other => panic!("expected a window to be opened, got {other:?}"),
        }
    }

    #[test]
    fn renaming_a_window_is_queued_too() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_click(|_, cx| cx.set_window_title("Untitled 2"))
                .into_any(),
        );

        harness.click(50.0, 50.0);
        assert_eq!(
            harness.cx.input.window_actions,
            vec![crate::event::WindowAction::SetTitle("Untitled 2".into())]
        );
    }

    #[test]
    fn a_press_on_a_label_still_clicks_its_button() {
        // This is the bug that made buttons feel dead: the press lands on the
        // button's text, and the release a pixel later lands on the button's
        // padding, so nothing shared the innermost element.
        let clicks = Model::new(0i32);
        let counter = clicks.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    button("Save")
                        .w(px_(120.0))
                        .h(px_(40.0))
                        .on_click(move |_, _| counter.update(|n| *n += 1)),
                )
                .into_any(),
        );

        // Press in the middle, where the label is, and release near the edge,
        // where only the button is.
        harness.press(60.0, 20.0);
        harness.release(112.0, 36.0);
        assert_eq!(clicks.get(), 1);
    }

    #[test]
    fn releasing_outside_the_pressed_element_does_not_click() {
        let clicks = Model::new(0i32);
        let counter = clicks.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    button("Save")
                        .w(px_(120.0))
                        .h(px_(40.0))
                        .on_click(move |_, _| counter.update(|n| *n += 1)),
                )
                .into_any(),
        );

        harness.press(60.0, 20.0);
        harness.release(300.0, 300.0);
        assert_eq!(clicks.get(), 0, "a release off the button is not a click");
    }

    #[test]
    fn pressing_a_label_makes_the_whole_chain_active() {
        // `button:active` has to match even though the press landed on the
        // label inside it.
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(button("Save").w(px_(120.0)).h(px_(40.0)))
                .into_any(),
        );

        harness.press(60.0, 20.0);
        assert!(
            harness.cx.input.active.len() >= 2,
            "expected the label and its button, got {:?}",
            harness.cx.input.active
        );
    }

    #[test]
    fn clicking_a_field_focuses_it_rather_than_blurring_it() {
        // Focus used to go to whatever was innermost, so clicking a field's own
        // text blurred it and only a click to the left of the text worked.
        let state = Model::new(TextInputState::new("hello"));
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(text_input(state, "Search").w(px_(200.0)).h(px_(36.0)))
                .into_any(),
        );

        harness.press(100.0, 18.0);
        assert!(
            harness.cx.input.focused.is_some(),
            "clicking a text field must focus it"
        );
    }

    #[test]
    fn a_drag_keeps_reaching_the_slider_it_started_on() {
        // Pressing the thumb captured the thumb, which has no handler, so the
        // drag went nowhere. Capture has to follow the whole press chain.
        let value = Model::new(0.0f32);
        let setter = value.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    slider(0.0, move |v| setter.set(v))
                        .w(px_(200.0))
                        .h(px_(22.0)),
                )
                .into_any(),
        );

        // Press on the very left, then drag right, past the element entirely.
        harness.press(2.0, 11.0);
        harness.drag_to(150.0, 11.0);
        assert!(value.get() > 0.5, "drag produced {}", value.get());

        harness.drag_to(900.0, 400.0);
        assert_eq!(value.get(), 1.0, "a drag past the end clamps");

        harness.release(900.0, 400.0);
        harness.drag_to(10.0, 11.0);
        assert_eq!(value.get(), 1.0, "moving after release must not drag");
    }

    #[test]
    fn a_scrollbar_can_be_grabbed_and_dragged() {
        let mut harness = Harness::new();
        let build = || {
            scroll_view()
                .id("list")
                .w(px_(200.0))
                .h(px_(100.0))
                .child(div().w(px_(180.0)).h(px_(1000.0)).shrink(0.0))
                .into_any()
        };
        harness.frame(build());

        let owner = harness.cx.scrollbars[0].owner;
        assert_eq!(harness.cx.scroll_state(owner).offset.y, Px(0.0));

        // Press on the track well below the thumb, which should jump to it.
        let track = harness.cx.scrollbars[0].track;
        harness.press(track.center().x.0, track.top().0 + 80.0);
        assert!(
            harness.cx.scroll_state(owner).offset.y > Px(0.0),
            "pressing the track should move the view"
        );

        // Then drag to the very bottom.
        harness.drag_to(track.center().x.0, track.bottom().0 + 50.0);
        let state = harness.cx.scroll_state(owner);
        assert_eq!(state.offset.y, state.max_offset().y);
    }

    #[test]
    fn a_scrollbar_wins_the_press_over_the_row_beneath_it() {
        let clicks = Model::new(0i32);
        let counter = clicks.clone();

        let mut harness = Harness::new();
        harness.frame(
            scroll_view()
                .id("list")
                .w(px_(200.0))
                .h(px_(100.0))
                .child(
                    div()
                        .w(px_(200.0))
                        .h(px_(1000.0))
                        .shrink(0.0)
                        .on_click(move |_, _| counter.update(|n| *n += 1)),
                )
                .into_any(),
        );

        let thumb = harness.cx.scrollbars[0].thumb;
        harness.click(thumb.center().x.0, thumb.center().y.0);
        assert_eq!(clicks.get(), 0, "the row under the scrollbar must not click");
    }

    // -- selection across runs ---------------------------------------------

    fn selection(anchor_run: u32, anchor: usize, head_run: u32, head: usize) -> TextSelection {
        TextSelection {
            id: GlobalElementId(0),
            anchor,
            head,
            anchor_run,
            head_run,
        }
    }

    #[test]
    fn a_selection_inside_one_run_covers_only_that_run() {
        let sel = selection(1, 2, 1, 6);
        assert_eq!(sel.range_in(1, 20), Some(2..6));
        assert_eq!(sel.range_in(0, 20), None);
        assert_eq!(sel.range_in(2, 20), None);
    }

    #[test]
    fn a_run_between_the_ends_is_taken_whole() {
        // The first run gives up its beginning, the last gives up its end,
        // and everything in between is selected entirely.
        let sel = selection(0, 4, 2, 3);
        assert_eq!(sel.range_in(0, 10), Some(4..10), "the first run");
        assert_eq!(sel.range_in(1, 10), Some(0..10), "a run in the middle");
        assert_eq!(sel.range_in(2, 10), Some(0..3), "the last run");
        assert_eq!(sel.range_in(3, 10), None, "past the end");
    }

    #[test]
    fn sweeping_backwards_selects_the_same_thing() {
        // A selection is swept out from wherever it started, so the pointer is
        // often before the anchor. It has to mean the same either way.
        let forwards = selection(0, 4, 2, 3);
        let backwards = selection(2, 3, 0, 4);
        for run in 0..3 {
            assert_eq!(
                forwards.range_in(run, 10),
                backwards.range_in(run, 10),
                "run {run} differs by direction"
            );
        }
    }

    #[test]
    fn an_offset_past_the_end_of_a_run_is_pulled_back() {
        // Runs are different lengths, and an offset taken from a long one
        // must not index past the end of a short one.
        let sel = selection(0, 2, 1, 900);
        assert_eq!(sel.range_in(1, 10), Some(0..10));
    }

    #[test]
    fn an_empty_selection_covers_nothing() {
        assert_eq!(selection(1, 5, 1, 5).range_in(1, 20), None);
        assert!(selection(1, 5, 1, 5).is_empty());
    }

    #[test]
    fn a_selection_that_reaches_another_run_is_not_empty() {
        // Same offset in two different runs is a real selection: everything
        // between them is in it.
        let sel = selection(0, 5, 2, 5);
        assert!(!sel.is_empty());
        assert!(sel.spans_runs());
        assert_eq!(sel.range_in(1, 10), Some(0..10));
    }

    #[test]
    fn copying_across_runs_keeps_them_in_reading_order() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(text("first line").selectable())
                .child(text("second line").selectable())
                .child(text("third line").selectable())
                .into_any(),
        );

        // From part way through the first to part way through the third.
        harness.cx.input.text_selection = Some(selection(0, 6, 2, 5));
        assert_eq!(
            harness.cx.selected_text().as_deref(),
            Some("line
second line
third")
        );
    }

    #[test]
    fn copying_a_selection_that_never_left_one_run_yields_that_run() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(text("first line").selectable())
                .child(text("second line").selectable())
                .into_any(),
        );
        harness.cx.input.text_selection = Some(selection(1, 0, 1, 6));
        assert_eq!(harness.cx.selected_text().as_deref(), Some("second"));
    }

    #[test]
    fn runs_are_numbered_in_the_order_they_are_painted() {
        // Reading order is what a selection spanning runs is defined against,
        // and a hash map has no order of its own.
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(text("alpha").selectable())
                .child(text("bravo").selectable())
                .child(text("charlie").selectable())
                .into_any(),
        );

        let mut runs: Vec<(u32, String)> = harness
            .cx
            .selectable
            .values()
            .map(|text| (text.order, text.layout.text.to_string()))
            .collect();
        runs.sort_by_key(|(order, _)| *order);
        let words: Vec<&str> = runs.iter().map(|(_, word)| word.as_str()).collect();
        assert_eq!(words, ["alpha", "bravo", "charlie"]);
    }

    // -- grid --------------------------------------------------------------

    use guirs_style::TrackSize;

    #[test]
    fn a_grid_places_children_into_its_columns() {
        // Three columns: a fixed one and two shares of what is left. In a
        // 500 wide grid that is 200, then 100 and 200 of the remaining 300.
        let mut harness = Harness::new();
        harness.size = Size::new(px_(500.0), px_(300.0));
        harness.frame(
            div()
                .size_full()
                .grid()
                .grid_columns([
                    TrackSize::Px(px_(200.0)),
                    TrackSize::Fr(1.0),
                    TrackSize::Fr(2.0),
                ])
                .child(div().h(px_(20.0)).id("a"))
                .child(div().h(px_(20.0)).id("b"))
                .child(div().h(px_(20.0)).id("c"))
                .into_any(),
        );

        let widths: Vec<f32> = harness
            .cx
            .hits
            .iter()
            .filter(|region| region.bounds.size.height.0 == 20.0)
            .map(|region| region.bounds.size.width.0)
            .collect();
        assert_eq!(widths.len(), 3, "not three cells: {widths:?}");
        assert!((widths[0] - 200.0).abs() < 0.5, "{widths:?}");
        assert!((widths[1] - 100.0).abs() < 0.5, "{widths:?}");
        assert!((widths[2] - 200.0).abs() < 0.5, "{widths:?}");
    }

    #[test]
    fn a_cell_can_take_more_than_one_column() {
        let mut harness = Harness::new();
        harness.size = Size::new(px_(600.0), px_(300.0));
        harness.frame(
            div()
                .size_full()
                .grid()
                .grid_columns([TrackSize::Fr(1.0), TrackSize::Fr(1.0), TrackSize::Fr(1.0)])
                .child(div().h(px_(20.0)).col_span(2))
                .child(div().h(px_(20.0)))
                .into_any(),
        );

        let widths: Vec<f32> = harness
            .cx
            .hits
            .iter()
            .filter(|region| region.bounds.size.height.0 == 20.0)
            .map(|region| region.bounds.size.width.0)
            .collect();
        assert_eq!(widths.len(), 2, "not two cells: {widths:?}");
        assert!((widths[0] - 400.0).abs() < 0.5, "the span was ignored: {widths:?}");
        assert!((widths[1] - 200.0).abs() < 0.5, "{widths:?}");
    }

    #[test]
    fn a_grid_wraps_onto_a_new_row_when_the_columns_run_out() {
        let mut harness = Harness::new();
        harness.size = Size::new(px_(400.0), px_(300.0));
        harness.frame(
            div()
                .size_full()
                .grid()
                .grid_columns([TrackSize::Fr(1.0), TrackSize::Fr(1.0)])
                .child(div().h(px_(20.0)))
                .child(div().h(px_(20.0)))
                .child(div().h(px_(20.0)))
                .into_any(),
        );

        let tops: Vec<f32> = harness
            .cx
            .hits
            .iter()
            .filter(|region| region.bounds.size.height.0 == 20.0)
            .map(|region| region.bounds.origin.y.0)
            .collect();
        assert_eq!(tops.len(), 3);
        assert_eq!(tops[0], tops[1], "the first two are not on one row");
        assert!(tops[2] > tops[0], "the third did not wrap onto a new row");
    }

    // -- commands and key contexts ----------------------------------------

    #[test]
    fn a_tables_resize_handle_is_where_it_can_be_grabbed() {
        // The handle is placed against the heading's trailing edge. If the
        // absolute placement produces nothing, the press lands on the heading
        // instead and resizing a column silently sorts it.
        use crate::table::{table, table_column, TableState, EDGE_HIT};

        let state = Model::new(TableState::default());
        let columns = [table_column("name", "Name").width(200.0)];

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(table(&columns, 1, state, |_, _| {
                    div().child(text("cell"))
                }))
                .into_any(),
        );

        // The heading is the widest thing at the top; the handle is the thin
        // one at its right edge.
        let handles: Vec<&HitRegion> = harness
            .cx
            .hits
            .iter()
            .filter(|region| {
                region.bounds.size.width.0 > 0.0
                    && region.bounds.size.width.0 <= EDGE_HIT + 0.5
                    && region.bounds.size.height.0 > 4.0
            })
            .collect();

        assert!(
            !handles.is_empty(),
            "no grab handle was laid out at all; widths seen: {:?}",
            harness
                .cx
                .hits
                .iter()
                .map(|r| (r.bounds.size.width.0, r.bounds.size.height.0))
                .collect::<Vec<_>>()
        );
        let handle = handles[0];
        assert!(
            (handle.bounds.origin.x.0 + handle.bounds.size.width.0 - 200.0).abs() < 1.0,
            "the handle is not on the column's trailing edge: {:?}",
            handle.bounds
        );
    }

    #[test]
    fn dragging_a_column_edge_does_not_also_sort_it() {
        // Stopping the press from propagating does not stop the click that
        // follows, so a heading has to recognise the gesture itself.
        use crate::table::{table, table_column, TableState};

        let state = Model::new(TableState::default());
        let columns = [table_column("name", "Name").width(200.0)];

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(table(&columns, 1, state.clone(), |_, _| {
                    div().child(text("cell"))
                }))
                .into_any(),
        );

        // On the trailing edge: a resize.
        harness.click(198.0, 10.0);
        assert_eq!(
            state.read().sort(),
            None,
            "dragging the edge sorted the column"
        );

        // Well inside it: a sort.
        harness.click(80.0, 10.0);
        assert!(state.read().sort().is_some(), "the heading stopped sorting");
    }

    /// Whether a frame drew a tooltip saying `text`.
    fn shows_tooltip(harness: &Harness, text: &str) -> bool {
        harness
            .cx
            .access
            .nodes
            .iter()
            .any(|node| node.text.as_deref() == Some(text))
    }

    #[test]
    fn a_tooltip_is_also_said_to_a_reader_without_waiting() {
        // Somebody using a screen reader never rests a pointer on anything,
        // so a tooltip that only ever appeared on hover would say nothing to
        // them at all.
        let tree = described(
            div()
                .size_full()
                .child(
                    crate::div::Div::new("button")
                        .label("Delete")
                        .tooltip("Delete this file")
                        .child(text("Delete")),
                )
                .into_any(),
        );
        let node = named(&tree, "Delete").expect("the control lost its name");
        assert_eq!(node.description.as_deref(), Some("Delete this file"));
    }

    #[test]
    fn a_tooltip_waits_before_it_appears() {
        // Moving across a row of controls must not flash a tooltip on each
        // one, so nothing is shown until the pointer has stopped.
        let mut harness = Harness::new();
        harness.cx.access.active = true;

        let build = || {
            div()
                .size_full()
                .child(div().size(px_(80.0)).tooltip("Delete this file"))
                .into_any()
        };

        harness.frame(build());
        harness.drag_to(40.0, 40.0);

        harness.frame_at(0.1, build());
        assert!(
            !shows_tooltip(&harness, "Delete this file"),
            "it appeared straight away"
        );

        // And a frame is owed at the moment it would appear, or it never
        // appears until the pointer happens to move again.
        assert!(
            harness.cx.redraw_at.is_some(),
            "nothing asked to be drawn again, so it would never appear"
        );

        harness.frame_at(1.0, build());
        assert!(shows_tooltip(&harness, "Delete this file"), "it never appeared");
    }

    #[test]
    fn moving_on_takes_a_tooltip_away() {
        let mut harness = Harness::new();
        harness.cx.access.active = true;

        let build = || {
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px_(0.0))
                        .top(px_(0.0))
                        .size(px_(80.0))
                        .tooltip("First"),
                )
                .child(
                    div()
                        .absolute()
                        .left(px_(200.0))
                        .top(px_(0.0))
                        .size(px_(80.0)),
                )
                .into_any()
        };

        harness.frame(build());
        harness.drag_to(40.0, 40.0);
        harness.frame_at(1.0, build());
        assert!(shows_tooltip(&harness, "First"));

        harness.drag_to(240.0, 40.0);
        harness.frame_at(1.1, build());
        assert!(!shows_tooltip(&harness, "First"), "it followed the pointer away");
    }

    #[test]
    fn pressing_anything_puts_a_tooltip_away() {
        let mut harness = Harness::new();
        harness.cx.access.active = true;
        let build = || {
            div()
                .size_full()
                .child(div().size(px_(80.0)).tooltip("Delete this file"))
                .into_any()
        };

        harness.frame(build());
        harness.drag_to(40.0, 40.0);
        harness.frame_at(1.0, build());
        assert!(shows_tooltip(&harness, "Delete this file"));

        harness.press(40.0, 40.0);
        harness.frame_at(1.1, build());
        assert!(
            !shows_tooltip(&harness, "Delete this file"),
            "it stayed over whatever was being done"
        );
    }

    // -- dragging inside the window ---------------------------------------

    /// A source on the left and a target on the right, reporting what landed.
    fn drag_harness(landed: Model<String>) -> Harness {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px_(0.0))
                        .top(px_(0.0))
                        .size(px_(100.0))
                        .draggable("tab", 7usize),
                )
                .child(
                    div()
                        .absolute()
                        .left(px_(200.0))
                        .top(px_(0.0))
                        .size(px_(100.0))
                        .on_drop("tab", move |dropped, _| {
                            let value = dropped.value::<usize>().copied().unwrap_or(0);
                            landed.set(format!("tab {value}"));
                        }),
                )
                .into_any(),
        );
        harness
    }

    #[test]
    fn dragging_something_onto_a_target_delivers_it() {
        let landed = Model::new(String::new());
        let mut harness = drag_harness(landed.clone());

        harness.press(50.0, 50.0);
        harness.drag_to(150.0, 50.0);
        harness.drag_to(250.0, 50.0);
        harness.release(250.0, 50.0);

        assert_eq!(landed.get(), "tab 7");
    }

    #[test]
    fn clicking_something_draggable_is_still_a_click() {
        // A press and release without movement has to stay a click, or
        // anything draggable stops being clickable.
        let clicked = Model::new(false);
        let saw = clicked.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .size(px_(100.0))
                        .draggable("tab", 1usize)
                        .on_click(move |_, _| saw.set(true)),
                )
                .into_any(),
        );

        harness.click(50.0, 50.0);
        assert!(clicked.get(), "a draggable element stopped being clickable");
    }

    #[test]
    fn dropping_somewhere_that_accepts_nothing_delivers_nothing() {
        let landed = Model::new(String::new());
        let mut harness = drag_harness(landed.clone());

        harness.press(50.0, 50.0);
        harness.drag_to(150.0, 50.0);
        // Let go over empty space rather than over the target.
        harness.release(150.0, 150.0);

        assert_eq!(landed.get(), "", "something was delivered to nowhere");
    }

    #[test]
    fn a_target_ignores_a_kind_it_does_not_accept() {
        let landed = Model::new(String::new());
        let saw = landed.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px_(0.0))
                        .top(px_(0.0))
                        .size(px_(100.0))
                        .draggable("file", 1usize),
                )
                .child(
                    div()
                        .absolute()
                        .left(px_(200.0))
                        .top(px_(0.0))
                        .size(px_(100.0))
                        .on_drop("tab", move |_, _| saw.set("wrong".into())),
                )
                .into_any(),
        );

        harness.press(50.0, 50.0);
        harness.drag_to(250.0, 50.0);
        harness.release(250.0, 50.0);
        assert_eq!(landed.get(), "", "a target took something it does not accept");
    }

    #[test]
    fn what_is_carried_and_where_it_would_land_are_both_known() {
        // Both are what the stylesheet matches on, so both have to be right
        // while the drag is happening rather than only at the end.
        let landed = Model::new(String::new());
        let mut harness = drag_harness(landed);

        harness.press(50.0, 50.0);
        harness.drag_to(250.0, 50.0);

        assert!(harness.cx.input.drag.is_active());
        assert!(
            harness.cx.input.drop_target.is_some(),
            "over the target and nothing was marked"
        );

        harness.drag_to(150.0, 150.0);
        assert!(
            harness.cx.input.drop_target.is_none(),
            "the mark stayed behind after the pointer left"
        );
    }

    #[test]
    fn a_drag_leaves_nothing_behind_when_it_ends() {
        let landed = Model::new(String::new());
        let mut harness = drag_harness(landed);
        harness.press(50.0, 50.0);
        harness.drag_to(250.0, 50.0);
        harness.release(250.0, 50.0);

        assert!(!harness.cx.input.drag.is_active());
        assert!(harness.cx.input.drop_target.is_none());
    }

    #[test]
    fn a_command_reaches_the_element_that_answers_it() {
        let ran = Model::new(String::new());
        let saved = ran.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .key_context("workspace")
                .on_command("file:save", move |_| saved.set("saved".into()))
                .child(div().size(px_(10.0)).focusable().autofocus())
                .into_any(),
        );

        assert!(harness.cx.dispatch_command("file:save").handled);
        assert_eq!(ran.get(), "saved");
    }

    #[test]
    fn a_command_travels_outwards_until_something_takes_it() {
        // The nearer handler wins, and the outer one is left alone. This is
        // what lets a pane answer what the editor inside it ignores.
        let ran = Model::new(String::new());
        let outer = ran.clone();
        let inner = ran.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_command("edit:copy", move |_| outer.set("outer".into()))
                .child(
                    div()
                        .size(px_(10.0))
                        .focusable()
                        .autofocus()
                        .on_command("edit:copy", move |_| inner.set("inner".into())),
                )
                .into_any(),
        );

        harness.cx.dispatch_command("edit:copy");
        assert_eq!(ran.get(), "inner");
    }

    #[test]
    fn a_command_asked_for_from_inside_a_handler_is_sent_afterwards() {
        // What a menu item does: it is chosen during a click, and the command
        // cannot be sent from there because the handler table is borrowed.
        let ran = Model::new(String::new());
        let answered = ran.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_command("file:save", move |_| answered.set("saved".into()))
                .child(
                    div()
                        .size(px_(20.0))
                        .focusable()
                        .autofocus()
                        .on_click(|_, cx| cx.dispatch_command("file:save")),
                )
                .into_any(),
        );

        harness.click(10.0, 10.0);
        assert_eq!(ran.get(), "", "it went out during the click");
        let outcome = harness.cx.drain_commands();
        assert!(outcome.handled);
        assert_eq!(ran.get(), "saved");
    }

    #[test]
    fn a_command_that_asks_for_itself_does_not_spin_forever() {
        // A window that stops responding is a worse failure than one that
        // drops the tail of an unreasonable chain.
        let count = Model::new(0u32);
        let seen = count.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .focusable()
                .autofocus()
                .on_command("loop:forever", move |cx| {
                    seen.update(|n| *n += 1);
                    cx.dispatch_command("loop:forever");
                })
                .into_any(),
        );

        harness.cx.input.pending_commands.push("loop:forever".into());
        harness.cx.drain_commands();
        let runs = count.get();
        assert!(runs > 0, "it never ran at all");
        assert!(runs <= 16, "it ran {runs} times and did not stop");
    }

    #[test]
    fn a_command_reaches_a_handler_with_nothing_focused() {
        // What a menu does. Nothing is focused while a menu is open, and a
        // command that only travelled the focus chain would go nowhere.
        let ran = Model::new(String::new());
        let answered = ran.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_command("file:save", move |_| answered.set("saved".into()))
                .into_any(),
        );

        assert_eq!(harness.cx.input.focused, None, "the test assumed no focus");
        assert!(harness.cx.dispatch_command("file:save").handled);
        assert_eq!(ran.get(), "saved");
    }

    #[test]
    fn focus_still_wins_over_the_fallback() {
        // The fallback must not overtake the chain: the nearest handler to
        // focus is still the one that should answer.
        let ran = Model::new(String::new());
        let outer = ran.clone();
        let inner = ran.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_command("edit:copy", move |_| outer.set("outer".into()))
                .child(
                    div()
                        .size(px_(10.0))
                        .focusable()
                        .autofocus()
                        .on_command("edit:copy", move |_| inner.set("inner".into())),
                )
                .into_any(),
        );

        harness.cx.dispatch_command("edit:copy");
        assert_eq!(ran.get(), "inner");
    }

    #[test]
    fn a_command_nothing_answers_is_not_an_error() {
        let mut harness = Harness::new();
        harness.frame(div().size_full().into_any());
        assert!(!harness.cx.dispatch_command("nobody:answers").handled);
    }

    #[test]
    fn the_focused_element_knows_which_contexts_enclose_it() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .key_context("workspace")
                .child(
                    div()
                        .key_context("editor")
                        .size(px_(10.0))
                        .focusable()
                        .autofocus(),
                )
                .into_any(),
        );

        let contexts: Vec<&str> = harness
            .cx
            .focused_contexts
            .iter()
            .map(|c| c.as_ref())
            .collect();
        assert_eq!(contexts, ["workspace", "editor"], "outermost first");
    }

    #[test]
    fn a_context_that_does_not_enclose_focus_is_not_claimed() {
        // Two siblings, one focused. The other's context must not appear, or
        // a binding meant for the file tree would fire while typing in the
        // editor next to it.
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(div().key_context("tree").size(px_(10.0)))
                .child(
                    div()
                        .key_context("editor")
                        .size(px_(10.0))
                        .focusable()
                        .autofocus(),
                )
                .into_any(),
        );

        let contexts: Vec<&str> = harness
            .cx
            .focused_contexts
            .iter()
            .map(|c| c.as_ref())
            .collect();
        assert_eq!(contexts, ["editor"]);
    }

    #[test]
    fn losing_focus_clears_the_context() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .key_context("editor")
                        .size(px_(10.0))
                        .focusable()
                        .autofocus(),
                )
                .into_any(),
        );
        assert!(!harness.cx.focused_contexts.is_empty());

        // Focus taken away, then another frame. A press now must not still
        // resolve against where focus used to be.
        harness.cx.input.focused = None;
        harness.frame(div().size_full().into_any());
        assert!(harness.cx.focused_contexts.is_empty());
        assert!(harness.cx.focused_chain.is_empty());
    }

    #[test]
    fn a_submenu_is_hit_before_the_menu_that_opened_it() {
        // Two overlays where one is inside the other, overlapping. A menu and
        // the submenu it opened: a click in the overlap belongs to the submenu,
        // because that is the thing on top.
        let picked = Model::new(String::new());
        let menu = picked.clone();
        let submenu = picked.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .overlay()
                        .absolute()
                        .top(px_(0.0))
                        .left(px_(0.0))
                        .size(px_(200.0))
                        .on_click(move |_, _| menu.set("menu".into()))
                        .child(
                            div()
                                .overlay()
                                .absolute()
                                .top(px_(0.0))
                                .left(px_(0.0))
                                .size(px_(100.0))
                                .on_click(move |_, _| submenu.set("submenu".into())),
                        ),
                )
                .into_any(),
        );

        harness.click(50.0, 50.0);
        assert_eq!(picked.get(), "submenu");
    }

    #[test]
    fn an_overlay_is_hit_before_what_it_covers() {
        let picked = Model::new(String::new());
        let under = picked.clone();
        let over = picked.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top(px_(0.0))
                        .left(px_(0.0))
                        .size(px_(200.0))
                        .on_click(move |_, _| under.set("under".into())),
                )
                .child(
                    div()
                        .overlay()
                        .absolute()
                        .top(px_(0.0))
                        .left(px_(0.0))
                        .size(px_(200.0))
                        .on_click(move |_, _| over.set("over".into())),
                )
                .into_any(),
        );

        harness.click(100.0, 100.0);
        assert_eq!(picked.get(), "over");
    }

    #[test]
    fn a_press_elsewhere_dismisses_an_open_popup() {
        let open = Model::new(true);
        let closer = open.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top(px_(0.0))
                        .left(px_(0.0))
                        .size(px_(80.0))
                        .on_dismiss(move |_| closer.set(false))
                        // The list sits outside its owner's box, exactly as a
                        // real dropdown does.
                        .child(
                            div()
                                .overlay()
                                .absolute()
                                .top(px_(90.0))
                                .left(px_(0.0))
                                .size(px_(80.0)),
                        ),
                )
                .into_any(),
        );

        harness.press(40.0, 40.0);
        harness.release(40.0, 40.0);
        assert!(open.get(), "a press on the control must not dismiss it");

        harness.press(40.0, 130.0);
        harness.release(40.0, 130.0);
        assert!(open.get(), "a press on the list must not dismiss it");

        harness.press(300.0, 300.0);
        harness.release(300.0, 300.0);
        assert!(!open.get(), "a press elsewhere must dismiss it");
    }

    #[test]
    fn ancestry_follows_the_paint_tree_not_the_geometry() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top(px_(0.0))
                        .left(px_(0.0))
                        .size(px_(60.0))
                        .child(
                            div()
                                .overlay()
                                .absolute()
                                .top(px_(200.0))
                                .left(px_(0.0))
                                .size(px_(60.0)),
                        ),
                )
                .into_any(),
        );

        let owner = harness
            .cx
            .hits
            .iter()
            .find(|r| r.bounds.size.width == px_(60.0) && r.bounds.origin.y == px_(0.0))
            .map(|r| r.id)
            .expect("owner painted");

        // A point inside the popup is nowhere near the owner's box, but the
        // owner is still an ancestor of what was hit.
        let involved = harness.cx.ancestry_at(Point::new(px_(30.0), px_(230.0)));
        assert!(
            involved.contains(&owner),
            "the popup's owner should count as involved"
        );
    }

    #[test]
    fn adding_a_handler_keeps_the_one_already_there() {
        // A widget with its own key handling must not lose it when a caller
        // adds theirs. This is what made backspace stop working in a field the
        // moment an application added an Enter handler to it.
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let first = log.clone();
        let second = log.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size(px_(60.0))
                .on_click(move |_, _| first.borrow_mut().push("widget"))
                .on_click(move |_, _| second.borrow_mut().push("caller"))
                .into_any(),
        );

        harness.click(30.0, 30.0);
        assert_eq!(*log.borrow(), vec!["widget", "caller"]);
    }

    #[test]
    fn a_field_still_edits_after_a_caller_adds_a_key_handler() {
        let state = Model::new(TextInputState::new("ab"));
        let submitted = Model::new(false);
        let flag = submitted.clone();

        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .on_key_down(move |event, _| {
                    if event.key == Key::Enter {
                        flag.set(true);
                    }
                })
                .into_any(),
        );

        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        let key = |k: Key| crate::event::KeyEvent {
            key: k,
            modifiers: Default::default(),
            repeat: false,
        };

        // The field's own editing still runs.
        harness
            .router
            .handle(&mut harness.cx, &InputEvent::KeyDown(key(Key::Backspace)));
        assert_eq!(state.read().value, "a");

        // And so does the handler the caller added.
        harness
            .router
            .handle(&mut harness.cx, &InputEvent::KeyDown(key(Key::Enter)));
        assert!(submitted.get());
    }

    #[test]
    fn a_field_keeps_every_space_it_is_given() {
        let state = Model::new(TextInputState::new(""));
        state.update(|s| {
            s.insert("a");
            s.insert(" ");
            s.insert(" ");
            s.insert("b");
        });
        assert_eq!(state.read().value, "a  b");
        assert_eq!(state.read().cursor, 4);
    }

    /// A focused area with real fonts, or `None` where the machine has none.
    ///
    /// Moving between lines is a question about laid out text, so unlike most
    /// of these tests it cannot be answered without a font.
    fn focused_area(value: &str, width: f32) -> Option<(Harness, Model<TextInputState>)> {
        let mut text = TextSystem::with_system_fonts();
        let probe = guirs_text::TextStyle::default();
        text.fonts
            .resolve(&probe.family, probe.weight, probe.style)?;

        let state = Model::new(TextInputState::new(value));
        let mut harness = Harness {
            cx: Cx::new(text, StyleEngine::default()),
            router: InputRouter::new(),
            size: Size::new(Px(400.0), Px(400.0)),
        };
        let build = |state: &Model<TextInputState>, width: f32| {
            text_area(state.clone(), "type here")
                .w(px_(width))
                .h(px_(200.0))
                .into_any()
        };
        harness.frame(build(&state, width));
        harness.press(4.0, 4.0);
        harness.release(4.0, 4.0);
        harness.frame(build(&state, width));
        Some((harness, state))
    }

    fn arrow(key: Key, shift: bool) -> InputEvent {
        InputEvent::KeyDown(crate::event::KeyEvent {
            key,
            modifiers: crate::event::Modifiers {
                shift,
                ..Default::default()
            },
            repeat: false,
        })
    }

    #[test]
    fn an_area_wraps_at_its_own_width() {
        let prose = "the quick brown fox jumps over the lazy dog and keeps on going";
        let Some((harness, _)) = focused_area(prose, 120.0) else {
            return;
        };
        let editor = harness.cx.editors.values().next().expect("no editor");
        assert!(
            editor.layout.line_count() > 1,
            "an area of 120 pixels laid {} out on one line",
            prose.len()
        );
    }

    #[test]
    fn a_single_line_field_never_wraps_however_long_it_gets() {
        let mut text = TextSystem::with_system_fonts();
        let probe = guirs_text::TextStyle::default();
        if text
            .fonts
            .resolve(&probe.family, probe.weight, probe.style)
            .is_none()
        {
            return;
        }

        let prose = "the quick brown fox jumps over the lazy dog and keeps on going";
        let state = Model::new(TextInputState::new(prose));
        let mut harness = Harness {
            cx: Cx::new(text, StyleEngine::default()),
            router: InputRouter::new(),
            size: Size::new(Px(400.0), Px(400.0)),
        };
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(120.0))
                .h(px_(36.0))
                .into_any(),
        );

        let editor = harness.cx.editors.values().next().expect("no editor");
        assert_eq!(
            editor.layout.line_count(),
            1,
            "a single line field wrapped, which would put the caret somewhere \
             the value has no offset for"
        );
    }

    #[test]
    fn an_area_grows_with_what_is_typed_in_it() {
        let mut text = TextSystem::with_system_fonts();
        let probe = guirs_text::TextStyle::default();
        if text
            .fonts
            .resolve(&probe.family, probe.weight, probe.style)
            .is_none()
        {
            return;
        }

        let measure = |text: TextSystem, value: &str| -> Px {
            let state = Model::new(TextInputState::new(value));
            let mut harness = Harness {
                cx: Cx::new(text, StyleEngine::default()),
                router: InputRouter::new(),
                size: Size::new(Px(400.0), Px(400.0)),
            };
            // No height given, so it takes the height its content asks for.
            harness.frame(
                text_area(state.clone(), "type here")
                    .w(px_(200.0))
                    .into_any(),
            );
            harness
                .cx
                .hits
                .iter()
                .map(|region| region.bounds.size.height)
                .fold(Px::ZERO, Px::max)
        };

        let one = measure(TextSystem::with_system_fonts(), "one line");
        let three = measure(text, "one line\ntwo lines\nthree lines");
        assert!(
            three > one,
            "an area of three lines was no taller than one of a single line: \
             {three:?} against {one:?}"
        );
    }

    #[test]
    fn an_area_takes_a_newline_from_the_return_key() {
        let state = Model::new(TextInputState::new("ab"));
        let mut harness = Harness::new();
        harness.frame(
            text_area(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(80.0))
                .into_any(),
        );
        harness.press(100.0, 40.0);
        harness.release(100.0, 40.0);
        state.update(|s| s.cursor = 1);

        harness
            .router
            .handle(&mut harness.cx, &arrow(Key::Enter, false));
        assert_eq!(state.read().value, "a\nb");
    }

    #[test]
    fn a_single_line_field_leaves_the_return_key_alone() {
        let state = Model::new(TextInputState::new("ab"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );
        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        harness
            .router
            .handle(&mut harness.cx, &arrow(Key::Enter, false));
        // Enter means whatever the application says it means here.
        assert_eq!(state.read().value, "ab");
    }

    #[test]
    fn the_caret_moves_between_lines() {
        let Some((mut harness, state)) = focused_area("first line\nsecond line", 300.0) else {
            return;
        };
        state.update(|s| s.cursor = 0);

        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));
        let after = state.read().cursor;
        assert!(
            after > "first line".len(),
            "down stayed on the first line, at {after}"
        );

        harness.router.handle(&mut harness.cx, &arrow(Key::Up, false));
        assert_eq!(state.read().cursor, 0, "up did not come back");
    }

    #[test]
    fn a_column_survives_a_short_line_in_between() {
        // Down through a short middle line and back up should return to the
        // column it set out from, not to the end of the short line.
        let Some((mut harness, state)) = focused_area("aaaaaaaaaa\nbb\naaaaaaaaaa", 300.0) else {
            return;
        };
        state.update(|s| s.cursor = 8);

        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));
        let middle = state.read().cursor;
        // The short line has nowhere to put column eight, so it clamps.
        assert!((11..=14).contains(&middle), "landed at {middle}");

        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));
        let bottom = state.read().cursor;
        let column = bottom - "aaaaaaaaaa\nbb\n".len();
        assert_eq!(column, 8, "the column was lost passing the short line");
    }

    #[test]
    fn moving_sideways_forgets_the_column() {
        let Some((mut harness, state)) = focused_area("aaaaaaaaaa\nbb\naaaaaaaaaa", 300.0) else {
            return;
        };
        state.update(|s| s.cursor = 8);

        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));
        // A sideways move is a new column, so the old one must not come back.
        harness.router.handle(&mut harness.cx, &arrow(Key::Left, false));
        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));

        let bottom = state.read().cursor;
        let column = bottom - "aaaaaaaaaa\nbb\n".len();
        assert!(column < 8, "the stale column came back, at {column}");
    }

    #[test]
    fn up_from_the_first_line_goes_to_the_start() {
        let Some((mut harness, state)) = focused_area("first\nsecond", 300.0) else {
            return;
        };
        state.update(|s| s.cursor = 3);

        harness.router.handle(&mut harness.cx, &arrow(Key::Up, false));
        assert_eq!(state.read().cursor, 0);

        state.update(|s| s.cursor = 8);
        harness.router.handle(&mut harness.cx, &arrow(Key::Down, false));
        assert_eq!(state.read().cursor, "first\nsecond".len());
    }

    #[test]
    fn shift_and_an_arrow_selects_down_the_lines() {
        let Some((mut harness, state)) = focused_area("first line\nsecond line", 300.0) else {
            return;
        };
        state.update(|s| {
            s.cursor = 0;
            s.anchor = None;
        });

        harness.router.handle(&mut harness.cx, &arrow(Key::Down, true));
        let selection = state.read().selection().expect("nothing was selected");
        assert_eq!(selection.start, 0);
        assert!(selection.end > "first line".len());
    }

    #[test]
    fn home_and_end_work_on_the_line_the_caret_is_on() {
        let Some((mut harness, state)) = focused_area("first\nsecond", 300.0) else {
            return;
        };
        state.update(|s| s.cursor = 9);

        harness.router.handle(&mut harness.cx, &arrow(Key::Home, false));
        assert_eq!(state.read().cursor, 6, "home left the line it was on");

        harness.router.handle(&mut harness.cx, &arrow(Key::End, false));
        assert_eq!(state.read().cursor, 12);
    }

    /// Paint a tree with a screen reader listening, and hand back what it
    /// would have been told.
    fn described(root: AnyElement) -> crate::access::AccessTree {
        let mut harness = Harness::new();
        harness.cx.access.active = true;
        // Deliberately not resolving labels here. Painting a frame is what
        // gives an element its name, and a test that did that step itself
        // would pass while every real window announced nothing.
        harness.frame(root);
        std::mem::take(&mut harness.cx.access)
    }

    fn named(tree: &crate::access::AccessTree, label: &str) -> Option<crate::access::AccessNode> {
        tree.nodes
            .iter()
            .find(|node| node.label.as_deref() == Some(label))
            .cloned()
    }

    #[test]
    fn a_button_is_announced_by_what_is_written_on_it() {
        let tree = described(
            div()
                .size_full()
                .child(button("Save").w(px_(120.0)).h(px_(40.0)))
                .into_any(),
        );

        let save = named(&tree, "Save").expect("the button had no name");
        assert_eq!(save.role, crate::access::AccessRole::Button);
        assert!(save.bounds.size.width > Px::ZERO, "it had nowhere to be");
    }

    #[test]
    fn an_icon_only_control_can_be_given_a_name() {
        // Nothing written on it, so without saying so it would be announced
        // as an unnamed button, which is the commonest accessibility failure
        // there is.
        let tree = described(
            div()
                .size_full()
                .child(
                    div()
                        .w(px_(34.0))
                        .h(px_(34.0))
                        .label("Close")
                        .role(crate::access::AccessRole::Button),
                )
                .into_any(),
        );

        let close = named(&tree, "Close").expect("the control had no name");
        assert_eq!(close.role, crate::access::AccessRole::Button);
    }

    #[test]
    fn a_checkbox_says_whether_it_is_ticked() {
        let on = described(
            div()
                .size_full()
                .child(checkbox("Notifications", true, |_| {}))
                .into_any(),
        );
        let node = named(&on, "Notifications").expect("no checkbox found");
        assert_eq!(node.role, crate::access::AccessRole::CheckBox);
        assert_eq!(node.checked, Some(true));

        let off = described(
            div()
                .size_full()
                .child(checkbox("Notifications", false, |_| {}))
                .into_any(),
        );
        assert_eq!(named(&off, "Notifications").unwrap().checked, Some(false));
    }

    #[test]
    fn a_text_field_is_named_by_its_placeholder_and_valued_by_its_text() {
        let state = Model::new(crate::widgets::TextInputState::new("hello"));
        let tree = described(
            div()
                .size_full()
                .child(crate::widgets::text_input(state, "Ask anything"))
                .into_any(),
        );
        let field = named(&tree, "Ask anything").expect("the field has no name");
        assert_eq!(field.role, crate::access::AccessRole::TextInput);
        // The name stays put while the value changes. A name that moved with
        // every keystroke would announce a different control each time.
        assert_eq!(field.value.as_deref(), Some("hello"));
    }

    #[test]
    fn a_control_is_not_named_after_the_popup_inside_it() {
        // A menu title paints the menu it opened as a child of itself. Named
        // from its contents, it comes out called after every entry on the
        // menu, which is what a reader would then announce.
        let tree = described(
            div()
                .size_full()
                .child(
                    crate::div::Div::new("button")
                        .label("File")
                        .child(text("File"))
                        .child(
                            div()
                                .overlay()
                                .child(text("Open Recent"))
                                .child(text("Save")),
                        ),
                )
                .into_any(),
        );
        assert!(
            named(&tree, "File").is_some(),
            "the control is not called what it says it is called: {:?}",
            tree.nodes
                .iter()
                .filter_map(|n| n.label.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_shortcut_is_offered_apart_from_the_name() {
        // "New Ctrl+N" read as a name is wrong twice over: it is not what the
        // entry is called, and the key is not announced as a key.
        let tree = described(
            div()
                .size_full()
                .child(
                    crate::div::Div::new("button")
                        .label("New")
                        .access_shortcut("Ctrl+N")
                        .child(text("New"))
                        .child(text("Ctrl+N").decorative()),
                )
                .into_any(),
        );
        let node = named(&tree, "New").expect("the entry lost its name");
        assert_eq!(node.shortcut.as_deref(), Some("Ctrl+N"));
    }

    #[test]
    fn a_clickable_box_is_reachable() {
        // A plain group is dropped from the tree the platform builds, so a
        // clickable div left as a group takes its own only means of activation
        // with it. Most of an interface here is built from clickable divs.
        let tree = described(
            div()
                .size_full()
                .child(div().on_click(|_, _| {}).child(text("Explore")))
                .into_any(),
        );
        let node = named(&tree, "Explore").expect("nothing named Explore");
        assert_eq!(node.role, crate::access::AccessRole::Button);
        assert!(node.clickable);

        // A box with nothing behind it stays scenery.
        let tree = described(div().size_full().child(div().child(text("Just text"))).into_any());
        assert!(
            !tree
                .nodes
                .iter()
                .any(|n| n.role == crate::access::AccessRole::Button),
            "a box that does nothing was announced as a button"
        );
    }

    #[test]
    fn a_group_is_not_named_after_everything_inside_it() {
        // Gathering text for plain containers names the outermost one after
        // the whole window, so a reader walking the tree reads the entire
        // interface aloud before arriving at anything in it.
        let tree = described(
            div()
                .size_full()
                .child(text("What can I help with?"))
                .child(button("Send"))
                .into_any(),
        );
        let group = tree
            .nodes
            .iter()
            .find(|node| node.role == crate::access::AccessRole::Group)
            .expect("no group painted");
        assert_eq!(group.label, None, "a group named itself after its contents");

        // The button still takes its name from the word inside it.
        assert!(named(&tree, "Send").is_some(), "the button lost its name");
    }

    #[test]
    fn decorative_text_stays_out_of_the_name() {
        // An icon font's glyph is a picture. Announced, it is a character from
        // a private-use area that means nothing to anyone.
        let glyph = "\u{e8bb}";
        let tree = described(
            div()
                .size_full()
                .child(
                    crate::div::Div::new("button")
                        .label("Close")
                        .child(text(glyph).decorative()),
                )
                .into_any(),
        );
        let node = named(&tree, "Close").expect("the control lost its name");
        assert_eq!(node.role, crate::access::AccessRole::Button);
        assert!(
            !tree.nodes.iter().any(|n| n.text.as_deref() == Some(glyph)),
            "the glyph was described"
        );
    }

    #[test]
    fn a_decorative_part_does_not_erase_its_container() {
        // A checkbox's tick is skipped, and skipping it must not take the
        // checkbox's own state with it. The tick is painted inside the
        // checkbox, so a careless implementation writes the tick's empty state
        // over the checkbox that is still open above it.
        let tree = described(
            div()
                .size_full()
                .child(checkbox("Telemetry", true, |_| {}))
                .into_any(),
        );
        let node = named(&tree, "Telemetry").expect("no checkbox found");
        assert_eq!(node.role, crate::access::AccessRole::CheckBox);
        assert_eq!(node.checked, Some(true), "the tick erased the checkbox");

        // And the tick itself is not described at all, or the name would come
        // out as the tick followed by the label.
        let tick = "\u{2713}";
        assert!(
            !tree.nodes.iter().any(|n| n.text.as_deref() == Some(tick)),
            "the tick was described"
        );
    }

    #[test]
    fn a_slider_is_a_position_in_a_range() {
        let tree = described(
            div()
                .size_full()
                .child(slider(0.25, |_| {}).w(px_(200.0)).h(px_(24.0)))
                .into_any(),
        );
        let node = tree
            .nodes
            .iter()
            .find(|n| n.role == crate::access::AccessRole::Slider)
            .expect("no slider found");
        assert_eq!(node.numeric, Some([0.25, 0.0, 1.0]));
    }

    #[test]
    fn a_text_field_is_a_field_and_not_a_box() {
        let state = Model::new(TextInputState::new("hello"));
        let tree = described(
            div()
                .size_full()
                .child(text_input(state, "type here").w(px_(200.0)).h(px_(36.0)))
                .into_any(),
        );
        let node = tree
            .nodes
            .iter()
            .find(|n| n.role == crate::access::AccessRole::TextInput)
            .expect("no field found");
        assert!(node.focusable, "a field a reader cannot reach is no use");
    }

    #[test]
    fn nothing_is_described_when_nothing_is_listening() {
        let mut harness = Harness::new();
        // Left inactive, which is how an ordinary run works.
        harness.frame(
            div()
                .size_full()
                .child(button("Save").w(px_(120.0)).h(px_(40.0)))
                .into_any(),
        );
        assert!(
            harness.cx.access.is_empty(),
            "an application with no screen reader still paid for one"
        );
    }

    #[test]
    fn the_tree_matches_the_way_the_interface_nests() {
        let tree = described(
            div()
                .size_full()
                .child(
                    div()
                        .w(px_(300.0))
                        .h(px_(100.0))
                        .label("Preferences")
                        .child(checkbox("Telemetry", false, |_| {})),
                )
                .into_any(),
        );

        let card = named(&tree, "Preferences").expect("no card");
        let box_index = tree.index_of(card.id).unwrap();
        // The checkbox is inside it, however deep the boxes go.
        let mut stack = tree.nodes[box_index].children.clone();
        let mut found = false;
        while let Some(next) = stack.pop() {
            if tree.nodes[next].role == crate::access::AccessRole::CheckBox {
                found = true;
                break;
            }
            stack.extend(tree.nodes[next].children.iter().copied());
        }
        assert!(found, "the checkbox was not inside the card");
    }

    #[test]
    fn autofocus_claims_the_keyboard_only_when_nothing_else_has_it() {
        let first = Model::new(TextInputState::new("first"));
        let second = Model::new(TextInputState::new("second"));

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .col()
                .child(
                    text_input(first.clone(), "one")
                        .w(px_(200.0))
                        .h(px_(36.0))
                        .autofocus(),
                )
                .child(
                    text_input(second.clone(), "two")
                        .w(px_(200.0))
                        .h(px_(36.0))
                        .autofocus(),
                )
                .into_any(),
        );

        // Two elements asking is not a fight: the first one painted wins.
        let focused = harness.cx.input.focused.expect("nothing took focus");
        assert!(harness.cx.editors.contains_key(&focused));
        let first_choice = focused;

        // And a later frame does not steal it back from wherever the person
        // has since clicked.
        harness.press(100.0, 60.0);
        harness.release(100.0, 60.0);
        let clicked = harness.cx.input.focused;
        assert_ne!(clicked, Some(first_choice), "the press moved focus");

        harness.frame(
            div()
                .size_full()
                .col()
                .child(
                    text_input(first.clone(), "one")
                        .w(px_(200.0))
                        .h(px_(36.0))
                        .autofocus(),
                )
                .child(
                    text_input(second.clone(), "two")
                        .w(px_(200.0))
                        .h(px_(36.0))
                        .autofocus(),
                )
                .into_any(),
        );
        assert_eq!(harness.cx.input.focused, clicked, "autofocus stole focus back");
    }

    #[test]
    fn composing_reaches_the_field_that_has_focus() {
        use crate::event::ImeEvent;

        let state = Model::new(TextInputState::new(""));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );
        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        harness.router.handle(
            &mut harness.cx,
            &InputEvent::Ime(ImeEvent::Preedit {
                text: "\u{304b}".into(),
                cursor: Some((3, 3)),
            }),
        );
        // Shown, but not yet part of what the application would read.
        assert_eq!(state.read().value, "");
        assert_eq!(state.read().display_text(), "\u{304b}");

        harness.router.handle(
            &mut harness.cx,
            &InputEvent::Ime(ImeEvent::Commit("\u{79c1}".into())),
        );
        assert_eq!(state.read().value, "\u{79c1}");
        assert!(!state.read().is_composing());
    }

    #[test]
    fn losing_the_input_method_abandons_what_was_composed() {
        use crate::event::ImeEvent;

        let state = Model::new(TextInputState::new("kept"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );
        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        harness.router.handle(
            &mut harness.cx,
            &InputEvent::Ime(ImeEvent::Preedit {
                text: "\u{304b}".into(),
                cursor: None,
            }),
        );
        assert!(state.read().is_composing());

        harness
            .router
            .handle(&mut harness.cx, &InputEvent::Ime(ImeEvent::Disabled));
        assert!(!state.read().is_composing());
        assert_eq!(state.read().value, "kept");
    }

    #[test]
    fn composing_with_nothing_focused_is_ignored() {
        use crate::event::ImeEvent;

        let mut harness = Harness::new();
        harness.frame(div().size_full().into_any());
        // No field, no focus, and no panic.
        let out = harness.router.handle(
            &mut harness.cx,
            &InputEvent::Ime(ImeEvent::Commit("x".into())),
        );
        assert!(!out.redraw);
    }

    #[test]
    fn a_focused_field_asks_for_an_input_method_and_says_where() {
        let state = Model::new(TextInputState::new("hello"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );
        // Nothing focused yet, so composition stays off.
        assert!(harness.cx.ime_area.is_none());

        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);
        // Painting again is what publishes the caret area.
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );

        let area = harness
            .cx
            .ime_area
            .expect("a focused field should place the candidate window");
        assert!(area.size.height > Px::ZERO);
    }

    #[test]
    fn a_press_while_composing_abandons_the_composition() {
        use crate::event::ImeEvent;

        let state = Model::new(TextInputState::new("ab"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );
        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        harness.router.handle(
            &mut harness.cx,
            &InputEvent::Ime(ImeEvent::Preedit {
                text: "\u{304b}".into(),
                cursor: None,
            }),
        );
        assert!(state.read().is_composing());

        harness.press(10.0, 18.0);
        assert!(!state.read().is_composing());
        assert_eq!(state.read().value, "ab");
    }

    #[test]
    fn a_press_in_a_field_takes_over_editing_it() {
        let state = Model::new(TextInputState::new("hello"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );

        // The field registered itself, so the window knows how to edit it.
        assert_eq!(harness.cx.editors.len(), 1);

        harness.press(100.0, 18.0);
        assert!(harness.cx.input.focused.is_some(), "the field takes focus");
        assert!(
            harness.cx.focused_editor().is_some(),
            "the focused element is the field the window can edit"
        );
        // A press in a field is not a document text selection.
        assert!(harness.cx.input.text_selection.is_none());
    }

    #[test]
    fn select_all_reaches_the_focused_field() {
        let state = Model::new(TextInputState::new("hello world"));
        let mut harness = Harness::new();
        harness.frame(
            text_input(state.clone(), "type here")
                .w(px_(200.0))
                .h(px_(36.0))
                .into_any(),
        );

        harness.press(100.0, 18.0);
        harness.release(100.0, 18.0);

        let mut modifiers = crate::event::Modifiers::default();
        #[cfg(target_os = "macos")]
        {
            modifiers.meta = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            modifiers.control = true;
        }

        harness.router.handle(
            &mut harness.cx,
            &InputEvent::KeyDown(crate::event::KeyEvent {
                key: Key::Character('a'),
                modifiers,
                repeat: false,
            }),
        );

        assert_eq!(state.read().selection(), Some(0..11));
    }

    #[test]
    fn a_field_without_focus_is_not_edited_by_a_shortcut() {
        let state = Model::new(TextInputState::new("hello"));
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(text_input(state.clone(), "type here").w(px_(200.0)).h(px_(36.0)))
                .into_any(),
        );

        // Nothing focused.
        let mut modifiers = crate::event::Modifiers::default();
        #[cfg(target_os = "macos")]
        {
            modifiers.meta = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            modifiers.control = true;
        }
        harness.router.handle(
            &mut harness.cx,
            &InputEvent::KeyDown(crate::event::KeyEvent {
                key: Key::Character('a'),
                modifiers,
                repeat: false,
            }),
        );

        assert_eq!(state.read().selection(), None);
    }

    #[test]
    fn clicks_still_bubble_and_can_still_be_stopped() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let inner = log.clone();
        let outer = log.clone();

        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .on_click(move |_, _| outer.borrow_mut().push("outer"))
                .child(
                    div()
                        .size(px_(60.0))
                        .on_click(move |_, _| inner.borrow_mut().push("inner")),
                )
                .into_any(),
        );

        harness.click(30.0, 30.0);
        assert_eq!(*log.borrow(), vec!["inner", "outer"]);
    }

    #[test]
    fn tab_moves_focus_through_the_focusable_elements() {
        let mut harness = Harness::new();
        harness.frame(
            div()
                .size_full()
                .child(button("One").w(px_(80.0)).h(px_(30.0)))
                .child(button("Two").w(px_(80.0)).h(px_(30.0)))
                .into_any(),
        );

        let order = harness.cx.focus_order.clone();
        assert_eq!(order.len(), 2);

        let tab = crate::event::KeyEvent {
            key: Key::Tab,
            modifiers: Default::default(),
            repeat: false,
        };
        harness.router.handle(&mut harness.cx, &InputEvent::KeyDown(tab));
        assert_eq!(harness.cx.input.focused, Some(order[0]));
        harness.router.handle(&mut harness.cx, &InputEvent::KeyDown(tab));
        assert_eq!(harness.cx.input.focused, Some(order[1]));
    }

    #[test]
    fn the_wheel_scrolls_the_innermost_list_that_can_still_move() {
        let mut harness = Harness::new();
        harness.frame(
            scroll_view()
                .id("outer")
                .w(px_(200.0))
                .h(px_(100.0))
                .child(div().w(px_(180.0)).h(px_(900.0)).shrink(0.0).child(text("x")))
                .into_any(),
        );

        let owner = harness.cx.hits[0].id;
        harness.router.handle(
            &mut harness.cx,
            &InputEvent::Scroll(crate::event::ScrollEvent {
                position: Point::new(Px(50.0), Px(50.0)),
                delta: Point::new(Px(0.0), Px(120.0)),
                modifiers: Default::default(),
                precise: false,
            }),
        );
        assert_eq!(harness.cx.scroll_state(owner).offset.y, Px(120.0));
    }

    fn px_(v: f32) -> Px {
        Px(v)
    }
}
