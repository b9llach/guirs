//! The built in widgets.
//!
//! Almost every one is a [`Div`] with a distinct element type name and a few
//! preset styles, which means the whole set is styleable from a stylesheet
//! with no special cases: `button {}`, `select:open {}`, `input:focus {}`.
//! Returning a plain `Div` also means every widget composes with the whole
//! styling API rather than a curated subset of it.
//!
//! Widgets are controlled: they render the value they are handed and report
//! changes through a callback. Nothing here owns application state.

use std::rc::Rc;

use guirs_core::{Bounds, Corners, Length, Paint, Point, Px, Rgba, SharedString};
use guirs_render::{Glyph, Quad};
use guirs_style::{ComputedStyle, CursorStyle, StateFlags, Style, Styled, WhiteSpace};
use guirs_text::{subpixel_phase, GlyphKey, LayoutOptions, TextStyle};

use crate::context::{content_box, Cx, TextEditor};
use crate::div::{div, Div};
use crate::element::Element;
use crate::event::{Key, MouseButton};
use crate::layout::{LayoutId, MeasureContext};
use crate::model::Model;
use crate::text::text;

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// A horizontal stack.
pub fn row() -> Div {
    div().row().items_center()
}

/// A vertical stack.
pub fn column() -> Div {
    div().col()
}

/// A container whose children overlap, for badges and overlays.
pub fn stack() -> Div {
    div().relative()
}

/// A surface with the `panel` type name, for sidebars and toolbars.
pub fn panel() -> Div {
    Div::new("panel").col()
}

/// A raised surface with the `card` type name.
pub fn card() -> Div {
    Div::new("card").col()
}

/// A vertically scrolling viewport.
///
/// Vertical only, because that is what almost every scroll view wants and
/// because scrolling both axes by default produces a horizontal scrollbar the
/// moment content is a pixel too wide.
pub fn scroll_view() -> Div {
    div()
        .col()
        .overflow_y(guirs_style::Overflow::Scroll)
        .overflow_x(guirs_style::Overflow::Hidden)
}

/// A horizontally scrolling viewport.
pub fn scroll_view_x() -> Div {
    div()
        .row()
        .overflow_x(guirs_style::Overflow::Scroll)
        .overflow_y(guirs_style::Overflow::Hidden)
}

/// A viewport that scrolls on both axes.
pub fn scroll_view_xy() -> Div {
    div().col().overflow_scroll()
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// A labelled button.
///
/// Add behaviour with `.on_click(...)` and appearance with classes:
/// `button("Save").class("primary").on_click(...)`.
pub fn button(label: impl Into<SharedString>) -> Div {
    Div::new("button")
        .focusable()
        .cursor_pointer()
        .row()
        .items_center()
        .justify_center()
        .child(text(label).whitespace_nowrap())
}

/// A button with no label text, for toolbars.
pub fn icon_button(glyph: impl Into<SharedString>) -> Div {
    Div::new("button")
        .class("icon")
        .focusable()
        .cursor_pointer()
        .row()
        .center()
        .child(text(glyph).whitespace_nowrap())
}

/// A button that carries no chrome of its own.
pub fn text_button(label: impl Into<SharedString>) -> Div {
    button(label).class("ghost")
}

// ---------------------------------------------------------------------------
// Toggles
// ---------------------------------------------------------------------------

/// A checkbox with its label.
///
/// Matches `checkbox`, and `checkbox:checked` when it is on.
pub fn checkbox(
    label: impl Into<SharedString>,
    checked: bool,
    on_toggle: impl Fn(bool) + 'static,
) -> Div {
    Div::new("checkbox")
        .access_checked(checked)
        .focusable()
        .cursor_pointer()
        .row()
        .items_center()
        .state_if(StateFlags::CHECKED, checked)
        .child(
            Div::new("checkbox-box").decorative()
                .row()
                .center()
                .state_if(StateFlags::CHECKED, checked)
                .child(crate::element::when(checked, || {
                    text("\u{2713}").class("checkbox-mark")
                })),
        )
        .child(text(label).class("checkbox-label").whitespace_nowrap())
        .on_click(move |_, cx| {
            on_toggle(!checked);
            cx.request_redraw();
        })
}

/// A sliding on and off switch.
///
/// Matches `toggle`, and `toggle:checked` when it is on.
pub fn toggle(on: bool, on_change: impl Fn(bool) + 'static) -> Div {
    Div::new("toggle")
        .access_checked(on)
        .focusable()
        .cursor_pointer()
        .row()
        .items_center()
        .state_if(StateFlags::CHECKED, on)
        .justify(if on {
            guirs_style::JustifyContent::End
        } else {
            guirs_style::JustifyContent::Start
        })
        .child(Div::new("toggle-knob").decorative().state_if(StateFlags::CHECKED, on))
        .on_click(move |_, cx| {
            on_change(!on);
            cx.request_redraw();
        })
}

/// A radio button in a group.
pub fn radio(
    label: impl Into<SharedString>,
    selected: bool,
    on_select: impl Fn() + 'static,
) -> Div {
    Div::new("radio")
        .access_checked(selected)
        .focusable()
        .cursor_pointer()
        .row()
        .items_center()
        .state_if(StateFlags::SELECTED, selected)
        .child(
            Div::new("radio-mark").decorative()
                .row()
                .center()
                .state_if(StateFlags::SELECTED, selected)
                .child(crate::element::when(selected, || {
                    Div::new("radio-dot").rounded_full()
                })),
        )
        .child(text(label).class("radio-label").whitespace_nowrap())
        .on_click(move |_, cx| {
            on_select();
            cx.request_redraw();
        })
}

// ---------------------------------------------------------------------------
// Ranges
// ---------------------------------------------------------------------------

/// A draggable slider over the range zero to one.
///
/// Dragging works past the ends of the track, because the pointer is captured
/// by whatever it was pressed on until it is released.
pub fn slider(value: f32, on_change: impl Fn(f32) + 'static) -> Div {
    let value = value.clamp(0.0, 1.0);
    let callback = Rc::new(on_change);
    let on_press = callback.clone();
    let on_drag = callback;

    Div::new("slider")
        // Announced as a position in a range rather than as a number of
        // pixels, which is the only form that means anything out loud.
        .access_range(value as f64, 0.0, 1.0)
        .focusable()
        .relative()
        .row()
        .items_center()
        .cursor_pointer()
        .child(
            Div::new("slider-track")
                .w_full()
                .row()
                .items_center()
                .child(Div::new("slider-fill").decorative().w(Length::Percent(value)).h_full()),
        )
        .child(
            Div::new("slider-thumb").decorative()
                .absolute()
                .left(Length::Percent(value)),
        )
        .on_mouse_down(move |event, cx| {
            on_press(event.fraction().x);
            cx.request_redraw();
        })
        .on_mouse_move(move |event, cx| {
            if cx.input.is_down(MouseButton::Left) {
                on_drag(event.fraction().x);
                cx.request_redraw();
            }
        })
}

/// A non interactive progress bar.
pub fn progress(fraction: f32) -> Div {
    let fraction = fraction.clamp(0.0, 1.0);
    Div::new("progress")
        .access_range(fraction as f64, 0.0, 1.0)
        .row()
        .items_center()
        .child(Div::new("progress-fill").decorative().w(Length::Percent(fraction)).h_full())
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// A dropdown.
///
/// The popup is painted on an overlay layer, so it draws over everything else
/// in the window and is not clipped by an enclosing scroll view.
pub fn select(
    options: &[impl AsRef<str>],
    selected: usize,
    open: Model<bool>,
    on_select: impl Fn(usize) + 'static,
) -> Div {
    let is_open = open.get();
    let label: SharedString = options
        .get(selected)
        .map(|o| SharedString::from(o.as_ref()))
        .unwrap_or_default();

    let callback = Rc::new(on_select);
    let toggle_open = open.clone();
    let dismiss = open.clone();

    let mut popup = Div::new("select-popup").col();
    if is_open {
        for (index, option) in options.iter().enumerate() {
            let choose = callback.clone();
            let close = open.clone();
            popup = popup.child(
                Div::new("select-option")
                    .row()
                    .items_center()
                    .cursor_pointer()
                    .state_if(StateFlags::SELECTED, index == selected)
                    .child(text(option.as_ref()).whitespace_nowrap())
                    .on_click(move |_, cx| {
                        choose(index);
                        close.set(false);
                        cx.stop_propagation();
                        cx.request_redraw();
                    }),
            );
        }
    }

    Div::new("select")
        .focusable()
        .cursor_pointer()
        .relative()
        .row()
        .items_center()
        .justify_between()
        .state_if(StateFlags::OPEN, is_open)
        .child(text(label).class("select-value").whitespace_nowrap())
        .child(text("\u{25be}").class("select-arrow"))
        .child(crate::element::when(is_open, move || {
            popup
                .overlay()
                .absolute()
                .top(Length::Percent(1.0))
                .left(Px(0.0))
        }))
        .on_click(move |_, cx| {
            toggle_open.update(|open| *open = !*open);
            cx.request_redraw();
        })
        .on_dismiss(move |cx| {
            // Clicking anywhere else closes the list and keeps the current
            // selection, which is what every other dropdown does.
            if dismiss.get() {
                dismiss.set(false);
                cx.request_redraw();
            }
        })
}

/// A row of tabs.
pub fn tab_bar(
    labels: &[impl AsRef<str>],
    selected: usize,
    on_select: impl Fn(usize) + 'static,
) -> Div {
    let callback = Rc::new(on_select);
    let mut bar = Div::new("tab-bar").row().items_center();
    for (index, label) in labels.iter().enumerate() {
        let choose = callback.clone();
        bar = bar.child(
            Div::new("tab")
                .focusable()
                .cursor_pointer()
                .row()
                .center()
                .state_if(StateFlags::SELECTED, index == selected)
                .child(text(label.as_ref()).whitespace_nowrap())
                .on_click(move |_, cx| {
                    choose(index);
                    cx.request_redraw();
                }),
        );
    }
    bar
}

/// A selectable row, for lists and file trees.
pub fn list_item(selected: bool) -> Div {
    Div::new("list-item")
        .row()
        .items_center()
        .cursor_pointer()
        .state_if(StateFlags::SELECTED, selected)
}

// ---------------------------------------------------------------------------
// Text entry
// ---------------------------------------------------------------------------

/// The editing state of a text field.
///
/// Offsets are byte indices into `value` and always land on character
/// boundaries, so a field holding non ASCII text edits correctly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInputState {
    pub value: String,
    /// Byte offset of the caret.
    pub cursor: usize,
    /// Byte offset of the other end of the selection, when there is one.
    pub anchor: Option<usize>,
    /// What the input method is currently composing, if anything.
    ///
    /// Deliberately not part of `value`. Composing text is a proposal rather
    /// than an edit: someone typing Japanese sees several intermediate strings
    /// before choosing one, and an application reading `value` in between
    /// should see what has actually been entered, not the guesses.
    pub preedit: Option<Preedit>,
}

/// Text an input method is composing, shown but not yet committed.
///
/// Held apart from the value and spliced in only for display, so the caret,
/// the selection and everything the application reads stay in the value's own
/// coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// Where in `value` the composing text appears.
    pub at: usize,
    /// The range the input method wants marked inside `text`, which is where
    /// it will put its candidate window. `None` means it wants no caret shown.
    pub cursor: Option<(usize, usize)>,
}

impl Preedit {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Where the caret goes inside the composing text, in bytes from its start.
    #[inline]
    pub fn caret(&self) -> usize {
        match self.cursor {
            Some((_, end)) => end.min(self.text.len()),
            None => self.text.len(),
        }
    }
}

impl TextInputState {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        TextInputState {
            cursor: value.len(),
            value,
            anchor: None,
            preedit: None,
        }
    }

    // -- input methods --

    /// Set or replace the text the input method is composing.
    ///
    /// An empty string clears it, which is how winit signals that composition
    /// ended. The first preedit of a session replaces the selection, the same
    /// as typing would.
    pub fn set_preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.preedit = None;
            return;
        }
        if self.preedit.is_none() {
            self.delete_selection();
        }
        let at = self.clamp(self.cursor);
        // The offsets come from the platform and index the composing string,
        // so a stale or malformed pair is clamped rather than trusted.
        let cursor = cursor.map(|(start, end)| {
            (
                floor_boundary(text, start.min(text.len())),
                floor_boundary(text, end.min(text.len())),
            )
        });
        self.preedit = Some(Preedit {
            text: text.to_string(),
            at,
            cursor,
        });
    }

    pub fn clear_preedit(&mut self) {
        self.preedit = None;
    }

    /// Accept text from the input method, ending any composition.
    pub fn commit(&mut self, text: &str) {
        self.preedit = None;
        if !text.is_empty() {
            self.insert(text);
        }
    }

    #[inline]
    pub fn is_composing(&self) -> bool {
        self.preedit.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// The value with any composing text spliced in, which is what is drawn.
    ///
    /// Borrowed when nothing is being composed, which is almost always, so the
    /// common path allocates nothing.
    pub fn display_text(&self) -> std::borrow::Cow<'_, str> {
        match &self.preedit {
            Some(preedit) if !preedit.is_empty() => {
                let at = preedit.at.min(self.value.len());
                let mut out = String::with_capacity(self.value.len() + preedit.text.len());
                out.push_str(&self.value[..at]);
                out.push_str(&preedit.text);
                out.push_str(&self.value[at..]);
                std::borrow::Cow::Owned(out)
            }
            _ => std::borrow::Cow::Borrowed(&self.value),
        }
    }

    /// Where the caret sits in [`display_text`](Self::display_text).
    ///
    /// While composing it follows the input method's own caret inside the
    /// composing text rather than the value's, which is what lets someone move
    /// around inside a half finished word.
    pub fn display_cursor(&self) -> usize {
        match &self.preedit {
            Some(preedit) if !preedit.is_empty() => preedit.at + preedit.caret(),
            _ => self.cursor,
        }
    }

    /// The composing text's range in [`display_text`](Self::display_text).
    pub fn display_preedit_range(&self) -> Option<std::ops::Range<usize>> {
        let preedit = self.preedit.as_ref()?;
        if preedit.is_empty() {
            return None;
        }
        Some(preedit.at..preedit.at + preedit.text.len())
    }

    /// Map a byte offset in the displayed text back to one in the value.
    ///
    /// Everything the pointer does is answered against what is on screen, and
    /// the value is what the application reads, so the two have to be kept in
    /// step while a composition is open.
    pub fn value_offset(&self, display_offset: usize) -> usize {
        let Some(range) = self.display_preedit_range() else {
            return display_offset.min(self.value.len());
        };
        if display_offset <= range.start {
            display_offset
        } else if display_offset >= range.end {
            display_offset - (range.end - range.start)
        } else {
            // Inside the composing text, which has no home in the value yet.
            range.start
        }
    }

    /// The selected range, ordered.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.value.len();
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Remove the selection, returning whether anything was removed.
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.value.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = None;
        true
    }

    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let at = self.clamp(self.cursor);
        self.value.insert_str(at, text);
        self.cursor = at + text.len();
        self.anchor = None;
    }

    /// Delete backward, one grapheme at a time.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.clamp(self.cursor);
        if at == 0 {
            return;
        }
        let previous = self.previous_boundary(at);
        self.value.replace_range(previous..at, "");
        self.cursor = previous;
    }

    /// Delete forward.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.clamp(self.cursor);
        if at >= self.value.len() {
            return;
        }
        let next = self.next_boundary(at);
        self.value.replace_range(at..next, "");
    }

    pub fn move_left(&mut self, extend: bool) {
        let at = self.clamp(self.cursor);
        self.set_cursor(self.previous_boundary(at), extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        let at = self.clamp(self.cursor);
        self.set_cursor(self.next_boundary(at), extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        self.set_cursor(0, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        self.set_cursor(self.value.len(), extend);
    }

    /// Move the caret, extending the selection or dropping it.
    ///
    /// Public because moving between lines needs the laid out text, so the
    /// window works out where to go and then says so.
    pub fn set_caret(&mut self, to: usize, extend: bool) {
        self.set_cursor(to, extend);
    }

    fn set_cursor(&mut self, to: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = self.clamp(to);
    }

    fn clamp(&self, at: usize) -> usize {
        let at = at.min(self.value.len());
        if self.value.is_char_boundary(at) {
            at
        } else {
            self.previous_boundary(at)
        }
    }

    fn previous_boundary(&self, from: usize) -> usize {
        if from == 0 {
            return 0;
        }
        let mut index = from - 1;
        while index > 0 && !self.value.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn next_boundary(&self, from: usize) -> usize {
        let len = self.value.len();
        if from >= len {
            return len;
        }
        let mut index = from + 1;
        while index < len && !self.value.is_char_boundary(index) {
            index += 1;
        }
        index
    }
}

/// A single line text field.
///
/// Matches `input`, and `input:focus` while it holds keyboard focus.
pub fn text_input(state: Model<TextInputState>, placeholder: impl Into<SharedString>) -> Div {
    field(state, placeholder, false)
}

/// A field that holds more than one line.
///
/// Wraps at its own width, grows downwards as lines are added, and takes Enter
/// as a newline rather than leaving it for the application. Cap the growth
/// with `max_h` where a composer should stop somewhere.
///
/// ```
/// # use guirs_core::px;
/// # use guirs_style::Styled;
/// # use guirs_ui::{model::Model, widgets::{text_area, TextInputState}};
/// # let notes = Model::new(TextInputState::new(""));
/// text_area(notes.clone(), "Notes\u{2026}").w_full().max_h(px(160.0));
/// ```
pub fn text_area(state: Model<TextInputState>, placeholder: impl Into<SharedString>) -> Div {
    let newlines = state.clone();
    field(state, placeholder, true).on_key_down(move |event, cx| {
        // Enter belongs to the text here. A single line field leaves it alone,
        // because there it means whatever the application says it means.
        if event.key == Key::Enter && !event.modifiers.accelerator() {
            newlines.update(|state| {
                if !state.is_composing() {
                    state.insert("\n");
                }
            });
            cx.request_redraw();
        }
    })
}

fn field(
    state: Model<TextInputState>,
    placeholder: impl Into<SharedString>,
    multiline: bool,
) -> Div {
    let typing = state.clone();
    let keys = state.clone();
    let placeholder = placeholder.into();

    Div::new(if multiline { "textarea" } else { "input" })
        .focusable()
        .cursor_text()
        .row()
        .items_center()
        .overflow_hidden()
        // The placeholder is what tells a person what the box is for, so it is
        // the name. The typed text is the value: a name that changed with
        // every keystroke would announce a different control each time.
        .label(placeholder.clone())
        .access_value(state.read().value.clone())
        .child(TextFieldContent {
            state: state.clone(),
            placeholder,
            computed: ComputedStyle::default(),
            node: None,
            global_id: guirs_core::GlobalElementId::ROOT,
            field_id: guirs_core::GlobalElementId::ROOT,
            multiline,
        })
        .on_text(move |event, cx| {
            typing.update(|state| {
                // Composed text arrives as a commit, not as typing. Platforms
                // differ about whether they also send the keystrokes that fed
                // the input method, and acting on both would insert twice.
                if state.is_composing() {
                    return;
                }
                state.insert(&event.text);
            });
            cx.request_redraw();
        })
        .on_key_down(move |event, cx| {
            let shift = event.modifiers.shift;
            keys.update(|state| {
                // While an input method is composing, the arrows and backspace
                // are its business: they move around inside the candidate, and
                // the platform reports the result as a new preedit. Acting on
                // them here as well would edit the text behind the composition.
                if state.is_composing() {
                    return;
                }
                match event.key {
                    Key::Backspace => state.backspace(),
                    Key::Delete => state.delete(),
                    Key::Left => state.move_left(shift),
                    Key::Right => state.move_right(shift),
                    Key::Home => state.move_home(shift),
                    Key::End => state.move_end(shift),
                    Key::Character('a') if event.modifiers.accelerator() => state.select_all(),
                    _ => {}
                }
            });
            cx.request_redraw();
        })
}

/// Draws a text field's value, selection and caret.
///
/// This is a real element rather than a styled `Div` because the caret has to
/// sit at a measured glyph boundary, which only the text layout knows.
struct TextFieldContent {
    state: Model<TextInputState>,
    placeholder: SharedString,
    computed: ComputedStyle,
    node: Option<LayoutId>,
    global_id: guirs_core::GlobalElementId,
    /// The field itself. Focus lives there, not on this content element, so
    /// the caret has to ask about its parent rather than about itself.
    field_id: guirs_core::GlobalElementId,
    /// Whether the value may contain newlines and wrap.
    multiline: bool,
}

impl TextFieldContent {
    fn display(&self) -> (SharedString, bool) {
        // The composing text is spliced in, so a field showing nothing but a
        // half typed Japanese word shows that rather than its placeholder.
        let shown = self.state.read().display_text().into_owned();
        if shown.is_empty() {
            (self.placeholder.clone(), true)
        } else {
            (SharedString::from(shown), false)
        }
    }

    fn text_style(&self) -> TextStyle {
        TextStyle {
            family: self.computed.font_family.clone(),
            weight: self.computed.font_weight,
            style: self.computed.font_style,
            size: self.computed.font_size,
            letter_spacing: self.computed.letter_spacing,
        }
    }

    /// How wide the caret is drawn.
    fn caret_width(&self, cx: &Cx) -> Px {
        cx.styles
            .length_token("caret-width")
            .and_then(|length| length.resolve(Px::ZERO, cx.root_font_size))
            .filter(|width| *width > Px::ZERO)
            .unwrap_or(Px(1.5))
    }

    /// How rounded its ends are. Zero is the usual bar.
    fn caret_radius(&self, cx: &Cx) -> Px {
        cx.styles
            .length_token("caret-radius")
            .and_then(|length| length.resolve(Px::ZERO, cx.root_font_size))
            .unwrap_or(Px::ZERO)
    }

    /// How the value is broken into lines.
    ///
    /// A single line field preserves whitespace and never wraps, which is what
    /// keeps two consecutive spaces from collapsing and shifting every byte
    /// offset after them. An area wraps at the width it was given as well.
    fn layout_options(&self, max_width: Option<Px>) -> LayoutOptions {
        LayoutOptions {
            white_space: if self.multiline {
                WhiteSpace::PreWrap
            } else {
                WhiteSpace::Pre
            },
            max_width: if self.multiline { max_width } else { None },
            line_height: self.computed.line_height,
            ..Default::default()
        }
    }
}

/// How long a caret spends visible, and then hidden, by default.
///
/// Windows and macOS both settle around here, and matching what the rest of
/// the desktop does matters more than any particular number.
const DEFAULT_CARET_BLINK_MS: f32 = 530.0;

/// What a field's caret is doing, kept between frames.
#[derive(Clone, Copy, Debug)]
struct CaretBlink {
    /// When the caret last moved or the text last changed.
    ///
    /// The phase runs from here rather than from an absolute clock, so the
    /// caret is solid at the moment of a keystroke. A caret that blinked out
    /// exactly as a letter appeared would read as a dropped keystroke.
    since: f64,
    cursor: usize,
    length: usize,
}

/// Whether the caret is showing this frame, and when to ask for the next one.
///
/// A period of zero means a caret that never blinks, which is what someone who
/// finds it distracting, or who is recording the screen, will want.
fn caret_phase(since: f64, now: f64, period_ms: f32) -> (bool, Option<f64>) {
    if period_ms <= 0.0 {
        return (true, None);
    }
    let period = (period_ms as f64) / 1000.0;
    let elapsed = (now - since).max(0.0);
    // Even half periods are lit, odd ones dark.
    let visible = ((elapsed / period) as u64).is_multiple_of(2);
    // The next frame it changes, rather than every frame until then.
    let next_edge = ((elapsed / period).floor() + 1.0) * period;
    (visible, Some((next_edge - elapsed).max(0.001)))
}

/// Round a byte offset down to a character boundary of `text`.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

impl Element for TextFieldContent {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        // Before descending, the current element is this one's parent: the
        // field that actually holds focus.
        self.field_id = cx.current_id();
        self.global_id = cx.begin_element(
            None,
            &SharedString::from("input-content"),
            &[],
            false,
            StateFlags::EMPTY,
        );
        self.computed = cx.compute_style(&Style::new().w_full().h_full());
        cx.end_element();

        let (display, _) = self.display();
        let node = cx.layout.add_text_node(
            &self.computed,
            MeasureContext {
                content: guirs_text::RichText::plain(display),
                style: self.text_style(),
                // `Pre` rather than `NoWrap` for a single line field: both
                // refuse to wrap, but `NoWrap` still collapses runs of
                // whitespace, which would swallow a second space and shift
                // every byte offset after it out of step with the value the
                // caret is indexing into.
                //
                // An area passes no width here on purpose. Measuring is where
                // layout asks how tall this is for a width it has not decided
                // yet, and it supplies that width itself.
                options: self.layout_options(None),
            },
        );
        self.node = Some(node);
        node
    }

    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        let (display, is_placeholder) = self.display();
        let content = content_box(bounds, &self.computed, cx.root_font_size);
        let style = self.text_style();
        // A pixel of slack absorbs the difference between the width layout
        // measured and the width it rounded the box to, the same as it does
        // for ordinary text.
        let options = self.layout_options(Some(content.size.width + Px(1.0)));
        let laid_out = cx.text.layout(&display, &style, &options);

        // Register before anything else, so a press this frame already has
        // somewhere to put the caret.
        cx.editors.insert(
            self.field_id,
            TextEditor {
                origin: content.origin,
                layout: laid_out.clone(),
                state: self.state.clone(),
                multiline: self.multiline,
            },
        );

        let opacity = cx.opacity();
        let color = if is_placeholder {
            self.computed.color.fade(opacity * 0.45)
        } else {
            self.computed.color.fade(opacity)
        };

        // A single line is centred in whatever height the field was given. An
        // area starts at the top and grows downwards, because centring a
        // growing block would move every line each time one was added.
        let top = if self.multiline {
            content.origin.y
        } else {
            content.origin.y
                + ((content.size.height - laid_out.size.height) * 0.5).max(Px::ZERO)
        };
        let scale = cx.scale.0;

        let focused = cx.input.focused == Some(self.field_id);
        let state = self.state.read().clone();

        // Selection sits behind the glyphs. Drawn per line rather than as one
        // rectangle, because a selection that wraps covers a ragged region no
        // single rectangle describes.
        if !is_placeholder && focused {
            if let Some(range) = state.selection() {
                let color = cx
                    .styles
                    .color_token("selection")
                    .unwrap_or_else(|| Rgba::rgba8(108, 92, 231, 110));
                let origin = Point::new(content.origin.x, top);
                for rect in laid_out.selection_rects(range) {
                    cx.scene.push_quad(Quad {
                        bounds: rect.translate(origin),
                        corner_radii: Corners::all(Px(2.0)),
                        background: Paint::Solid(color.fade(opacity)),
                        border_widths: guirs_core::Edges::zero(),
                        border_color: Rgba::TRANSPARENT,
                        opacity: 1.0,
                    });
                }
            }
        }

        // Every line has its own baseline, because a wrapped area has more
        // than one and because a line carrying a larger face sits lower than
        // its neighbours.
        for line in &laid_out.lines {
            let baseline = top + line.origin.y + line.baseline;
            let mut pen_x = content.origin.x + line.origin.x;
            for run in &line.shaped.runs {
                for glyph in &run.glyphs {
                    let x = pen_x + glyph.x_offset;
                    cx.scene.push_glyph(Glyph {
                        key: GlyphKey {
                            font: run.font,
                            glyph: glyph.id,
                            size: Px(run.size.0 * scale),
                            phase: subpixel_phase(x.0 * scale),
                        },
                        position: Point::new(x, baseline + glyph.y_offset),
                        color,
                    });
                    pen_x += glyph.advance;
                }
            }
        }

        // A composing run is underlined, which is the convention every
        // platform uses to say "this is a proposal, not yet text". Drawn from
        // the same per line rectangles a selection uses, so a composition that
        // wraps is underlined on both lines rather than across the gap.
        if let Some(range) = state.display_preedit_range() {
            let thickness = (self.computed.font_size * 0.08).max(Px(1.0));
            let origin = Point::new(content.origin.x, top);
            for rect in laid_out.selection_rects(range) {
                let rect = rect.translate(origin);
                cx.scene.push_quad(Quad {
                    bounds: Bounds::from_xywh(
                        rect.origin.x,
                        rect.origin.y + rect.size.height - thickness,
                        rect.size.width,
                        thickness,
                    ),
                    corner_radii: Corners::zero(),
                    background: Paint::Solid(color),
                    border_widths: guirs_core::Edges::zero(),
                    border_color: Rgba::TRANSPARENT,
                    opacity: 1.0,
                });
            }
        }

        if focused {
            // While composing, the caret follows the input method's own
            // position inside the composing text rather than the value's.
            let (caret, caret_height) = if is_placeholder {
                (Point::zero(), laid_out.line_height)
            } else {
                laid_out.caret(state.display_cursor())
            };
            let caret_bounds = Bounds::from_xywh(
                content.origin.x + caret.x,
                top + caret.y,
                self.caret_width(cx),
                caret_height,
            );

            // The phase restarts whenever the caret moves or the text changes,
            // so it is solid while someone is typing and only blinks once they
            // stop.
            let now = cx.now;
            let key = self.field_id.scoped("caret-blink");
            let mut blink = cx.state.peek::<CaretBlink>(key).copied().unwrap_or(
                CaretBlink {
                    since: now,
                    cursor: state.cursor,
                    length: state.value.len(),
                },
            );
            if blink.cursor != state.cursor || blink.length != state.value.len() {
                blink = CaretBlink {
                    since: now,
                    cursor: state.cursor,
                    length: state.value.len(),
                };
            }
            cx.state.put(key, blink);

            let period = cx
                .styles
                .duration_token("caret-blink")
                .unwrap_or(DEFAULT_CARET_BLINK_MS);
            let (visible, next) = caret_phase(blink.since, now, period);
            if let Some(next) = next {
                // One frame when it next changes, rather than sixty a second
                // spent drawing the same caret.
                cx.request_redraw_in(next);
            }

            if visible {
                let caret_color = cx
                    .styles
                    .color_token("caret")
                    .unwrap_or(self.computed.color);
                cx.scene.push_quad(Quad {
                    bounds: caret_bounds,
                    corner_radii: Corners::all(self.caret_radius(cx)),
                    background: Paint::Solid(caret_color.fade(opacity)),
                    border_widths: guirs_core::Edges::zero(),
                    border_color: Rgba::TRANSPARENT,
                    opacity: 1.0,
                });
            }

            // Reported whether or not the caret is showing this instant: the
            // candidate window follows where the caret is, not whether it
            // happens to be lit.
            cx.ime_area = Some(caret_bounds);
        }

        cx.register_hit(self.global_id, bounds, CursorStyle::Text, false, false);
    }
}

crate::impl_into_element!(TextFieldContent);

#[cfg(test)]
mod tests {
    use super::*;

    // Composing text is what typing Japanese, Chinese or Korean produces, and
    // what accented Latin produces on several platforms. The rule these tests
    // pin down is that it is a proposal rather than an edit: it is drawn, but
    // an application reading the value must not see it until it is committed.

    #[test]
    fn a_caret_blinks_on_and_off_in_equal_turns() {
        // On for the first period, off for the second, and back on again.
        let (on, _) = caret_phase(0.0, 0.1, 530.0);
        assert!(on, "the caret was dark the instant it appeared");
        assert!(!caret_phase(0.0, 0.6, 530.0).0);
        assert!(caret_phase(0.0, 1.1, 530.0).0);
        assert!(!caret_phase(0.0, 1.6, 530.0).0);
    }

    #[test]
    fn a_caret_is_solid_at_the_moment_of_a_keystroke() {
        // The phase runs from the last edit, so however long someone has been
        // sitting there, the caret is lit the instant they type. A caret that
        // blinked out exactly as a letter appeared would read as a dropped
        // keystroke.
        for typed_at in [0.0, 3.7, 91.2, 500.0] {
            assert!(
                caret_phase(typed_at, typed_at, 530.0).0,
                "dark immediately after typing at {typed_at}"
            );
        }
    }

    #[test]
    fn the_next_frame_is_asked_for_when_the_caret_changes_not_before() {
        let (_, next) = caret_phase(0.0, 0.0, 530.0);
        let next = next.expect("a blinking caret has to ask for another frame");
        assert!(
            (next - 0.53).abs() < 0.01,
            "asked for a frame in {next}s rather than at the edge"
        );

        // Partway through, it asks for what is left rather than a whole period.
        let (_, next) = caret_phase(0.0, 0.4, 530.0);
        let next = next.unwrap();
        assert!((next - 0.13).abs() < 0.01, "asked for {next}s");
    }

    #[test]
    fn a_period_of_zero_is_a_caret_that_never_blinks() {
        let (visible, next) = caret_phase(0.0, 99.0, 0.0);
        assert!(visible);
        // And nothing has to be drawn on its account, ever.
        assert!(next.is_none(), "a solid caret still asked for frames");
    }

    #[test]
    fn composing_text_is_shown_but_is_not_the_value() {
        let mut state = TextInputState::new("ab");
        state.cursor = 1;
        state.set_preedit("\u{304b}", Some((3, 3)));

        assert_eq!(state.value, "ab", "the value took uncommitted text");
        assert_eq!(state.display_text(), "a\u{304b}b");
        assert!(state.is_composing());
    }

    #[test]
    fn committing_puts_the_text_in_and_ends_the_composition() {
        let mut state = TextInputState::new("ab");
        state.cursor = 1;
        state.set_preedit("\u{304b}", None);
        state.commit("\u{79c1}");

        assert_eq!(state.value, "a\u{79c1}b");
        assert!(!state.is_composing());
        assert_eq!(state.display_text(), "a\u{79c1}b");
        // The caret sits after what was committed.
        assert_eq!(state.cursor, 1 + "\u{79c1}".len());
    }

    #[test]
    fn an_empty_preedit_ends_the_composition_without_committing() {
        let mut state = TextInputState::new("ab");
        state.cursor = 2;
        state.set_preedit("\u{304b}", None);
        state.set_preedit("", None);

        assert_eq!(state.value, "ab");
        assert!(!state.is_composing());
        assert!(state.preedit.is_none());
    }

    #[test]
    fn the_first_preedit_replaces_the_selection() {
        let mut state = TextInputState::new("hello");
        state.anchor = Some(0);
        state.cursor = 5;
        state.set_preedit("\u{304b}", None);

        // Typing over a selection removes it, and composing is typing.
        assert_eq!(state.value, "");
        assert_eq!(state.display_text(), "\u{304b}");

        // A second preedit refines the first rather than deleting again.
        state.set_preedit("\u{304b}\u{306a}", None);
        assert_eq!(state.value, "");
        assert_eq!(state.display_text(), "\u{304b}\u{306a}");
    }

    #[test]
    fn the_caret_follows_the_input_method_inside_the_composition() {
        let mut state = TextInputState::new("xy");
        state.cursor = 1;
        // "a b", with the input method's caret after the first character.
        state.set_preedit("a b", Some((1, 1)));

        assert_eq!(state.display_text(), "xa by");
        assert_eq!(state.display_cursor(), 2);

        // With no caret reported, it goes to the end of the composition.
        state.set_preedit("a b", None);
        assert_eq!(state.display_cursor(), 1 + 3);
    }

    #[test]
    fn a_caret_offset_from_the_platform_is_clamped_to_a_character() {
        let mut state = TextInputState::new("");
        // Past the end, and in the middle of a three byte character.
        state.set_preedit("\u{304b}", Some((99, 2)));
        let preedit = state.preedit.as_ref().unwrap();
        assert_eq!(preedit.cursor, Some((3, 0)));
        // Whatever it says, slicing by it must not panic.
        let _ = state.display_text();
        let _ = state.display_cursor();
    }

    #[test]
    fn display_offsets_map_back_onto_the_value() {
        let mut state = TextInputState::new("abcd");
        state.cursor = 2;
        state.set_preedit("XY", None);
        assert_eq!(state.display_text(), "abXYcd");

        // Before the composition, offsets are unchanged.
        assert_eq!(state.value_offset(0), 0);
        assert_eq!(state.value_offset(2), 2);
        // Inside it there is nowhere in the value to point at, so it collapses
        // to where the composition begins.
        assert_eq!(state.value_offset(3), 2);
        // After it, the composition's length comes back off.
        assert_eq!(state.value_offset(4), 2);
        assert_eq!(state.value_offset(6), 4);
    }

    #[test]
    fn without_a_composition_nothing_is_copied_or_mapped() {
        let state = TextInputState::new("plain");
        assert!(matches!(
            state.display_text(),
            std::borrow::Cow::Borrowed("plain")
        ));
        assert_eq!(state.value_offset(3), 3);
        assert!(state.display_preedit_range().is_none());
        assert!(!state.is_composing());
    }

    #[test]
    fn an_abandoned_composition_leaves_the_value_alone() {
        let mut state = TextInputState::new("ab");
        state.cursor = 2;
        state.set_preedit("\u{304b}\u{306a}", None);
        state.clear_preedit();

        assert_eq!(state.value, "ab");
        assert_eq!(state.display_text(), "ab");
    }

    #[test]
    fn committing_nothing_still_ends_the_composition() {
        let mut state = TextInputState::new("ab");
        state.set_preedit("\u{304b}", None);
        state.commit("");
        assert!(!state.is_composing());
        assert_eq!(state.value, "ab");
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut state = TextInputState::new("helo");
        state.cursor = 3;
        state.insert("l");
        assert_eq!(state.value, "hello");
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn backspace_removes_one_character_before_the_caret() {
        let mut state = TextInputState::new("abc");
        state.backspace();
        assert_eq!(state.value, "ab");
        assert_eq!(state.cursor, 2);

        state.move_home(false);
        state.backspace();
        assert_eq!(state.value, "ab", "backspace at the start does nothing");
    }

    #[test]
    fn delete_removes_the_character_after_the_caret() {
        let mut state = TextInputState::new("abc");
        state.move_home(false);
        state.delete();
        assert_eq!(state.value, "bc");
        assert_eq!(state.cursor, 0);

        state.move_end(false);
        state.delete();
        assert_eq!(state.value, "bc", "delete at the end does nothing");
    }

    #[test]
    fn editing_respects_multi_byte_characters() {
        // Each of these is two or three bytes, so byte arithmetic that assumed
        // one byte per character would split them.
        let mut state = TextInputState::new("a\u{00e9}\u{65e5}");
        assert_eq!(state.value.len(), 6);

        state.backspace();
        assert_eq!(state.value, "a\u{00e9}");
        state.backspace();
        assert_eq!(state.value, "a");
    }

    #[test]
    fn arrow_keys_step_by_character_not_by_byte() {
        let mut state = TextInputState::new("a\u{65e5}b");
        state.move_home(false);
        state.move_right(false);
        assert_eq!(state.cursor, 1);
        state.move_right(false);
        assert_eq!(state.cursor, 4, "the middle character is three bytes");
        state.move_left(false);
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn shift_extends_a_selection_and_a_plain_arrow_drops_it() {
        let mut state = TextInputState::new("hello");
        state.move_home(false);
        state.move_right(true);
        state.move_right(true);
        assert_eq!(state.selection(), Some(0..2));

        state.move_right(false);
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut state = TextInputState::new("hello");
        state.select_all();
        state.insert("bye");
        assert_eq!(state.value, "bye");
        assert_eq!(state.cursor, 3);
        assert!(!state.has_selection());
    }

    #[test]
    fn backspace_removes_the_selection_rather_than_one_character() {
        let mut state = TextInputState::new("hello");
        state.select_all();
        state.backspace();
        assert_eq!(state.value, "");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn a_zero_width_selection_is_no_selection() {
        let mut state = TextInputState::new("hello");
        state.anchor = Some(2);
        state.cursor = 2;
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn select_all_covers_the_whole_value() {
        let mut state = TextInputState::new("hello");
        state.select_all();
        assert_eq!(state.selection(), Some(0..5));
    }

    #[test]
    fn cursor_positions_are_clamped_into_the_value() {
        let mut state = TextInputState::new("hi");
        state.cursor = 999;
        state.insert("!");
        assert_eq!(state.value, "hi!");
    }
}
