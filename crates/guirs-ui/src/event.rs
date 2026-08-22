//! Input events.
//!
//! These are deliberately independent of winit: the framework's own event types
//! are what widgets and application code see, so the windowing backend stays
//! replaceable and the events carry logical pixels rather than physical ones.

use guirs_core::{Bounds, Point, Px};

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// Modifier keys held during an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// The Windows key, or Command on macOS.
    pub meta: bool,
}

impl Modifiers {
    #[inline]
    pub fn none(&self) -> bool {
        !self.shift && !self.control && !self.alt && !self.meta
    }

    /// Whether the accelerator on this platform is the Control key.
    ///
    /// Everywhere except macOS. Worth asking rather than assuming: a binding
    /// that means Control has to know whether that is already the accelerator,
    /// or it ends up demanding the same key twice.
    #[inline]
    pub fn accelerator_is_control() -> bool {
        !cfg!(target_os = "macos")
    }

    /// The platform's usual accelerator modifier: Command on macOS, Control
    /// elsewhere. Checking this instead of `control` is what makes a shortcut
    /// feel native on both.
    #[inline]
    pub fn accelerator(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.meta
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.control
        }
    }
}

/// A pointer event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseEvent {
    /// Position in the window, in logical pixels.
    pub position: Point<Px>,
    /// Position relative to the top left of the element handling the event.
    ///
    /// Filled in per recipient during dispatch, so a slider can turn a click
    /// into a value without ever being told where it is on screen.
    pub local: Point<Px>,
    /// Bounds of the element handling the event.
    pub bounds: Bounds<Px>,
    pub button: Option<MouseButton>,
    pub modifiers: Modifiers,
    /// How many clicks in quick succession this is, starting at one.
    pub click_count: u32,
}

impl Default for MouseEvent {
    fn default() -> Self {
        MouseEvent {
            position: Point::zero(),
            local: Point::zero(),
            bounds: Bounds::zero(),
            button: None,
            modifiers: Modifiers::default(),
            click_count: 1,
        }
    }
}

impl MouseEvent {
    /// Where the pointer sits across the element, as a fraction from 0 to 1.
    ///
    /// Clamped, so a drag that leaves the element still yields a usable value.
    pub fn fraction(&self) -> Point<f32> {
        let along = |value: Px, extent: Px| -> f32 {
            if extent <= Px::ZERO {
                0.0
            } else {
                (value / extent).clamp(0.0, 1.0)
            }
        };
        Point::new(
            along(self.local.x, self.bounds.size.width),
            along(self.local.y, self.bounds.size.height),
        )
    }

    /// Retarget this event at an element with the given bounds.
    pub fn relative_to(mut self, bounds: Bounds<Px>) -> MouseEvent {
        self.bounds = bounds;
        self.local = Point::new(
            self.position.x - bounds.origin.x,
            self.position.y - bounds.origin.y,
        );
        self
    }
}

/// A scroll wheel or trackpad gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollEvent {
    pub position: Point<Px>,
    /// Movement in logical pixels. Positive y scrolls the content down.
    pub delta: Point<Px>,
    pub modifiers: Modifiers,
    /// True for a trackpad, which sends continuous small deltas rather than
    /// discrete notches. Momentum smoothing should only apply to wheels.
    pub precise: bool,
}

/// A keyboard key, named rather than scan coded so bindings are portable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    Space,
    Function(u8),
    /// A printable character, already case folded by the platform.
    Character(char),
    /// Anything the framework does not name.
    Unknown,
}

/// A key press or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    /// Whether this press came from the key repeating.
    pub repeat: bool,
}

impl KeyEvent {
    /// Whether this is `key` with no modifiers held.
    pub fn is_plain(&self, key: Key) -> bool {
        self.key == key && self.modifiers.none()
    }

    /// Whether this is the platform accelerator plus `character`.
    pub fn is_shortcut(&self, character: char) -> bool {
        self.modifiers.accelerator()
            && !self.modifiers.alt
            && self.key == Key::Character(character)
    }
}

/// Text produced by the platform's input method.
///
/// This is separate from [`KeyEvent`] because a single keystroke can produce no
/// text, one character, or a whole composed sequence, and only the platform
/// knows which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputEvent {
    pub text: String,
}

/// Anything that can arrive from the platform.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    MouseMove(MouseEvent),
    MouseDown(MouseEvent),
    MouseUp(MouseEvent),
    /// The pointer left the window entirely.
    MouseLeave,
    Scroll(ScrollEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    /// The window gained or lost keyboard focus.
    WindowFocus(bool),
    /// An input method is composing text.
    Ime(ImeEvent),
    /// Files were dropped onto the window from the desktop.
    FileDrop(FileDropEvent),
    /// Files are being dragged over the window but have not been let go.
    ///
    /// Sent so a drop target can light up. `paths` is what the platform is
    /// willing to say about them ahead of the drop, which on some platforms is
    /// nothing at all.
    FileHover(FileDropEvent),
    /// The drag left the window, or was cancelled.
    FileHoverCancelled,
}

/// What an input method is doing.
///
/// Typing Japanese, Chinese or Korean, and accented Latin on several
/// platforms, does not produce a character per keystroke. The input method
/// composes a proposal, shows it in place, and only later commits it. A field
/// that only understands finished characters cannot be used in those
/// languages at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    /// Composition became possible. Nothing to draw yet.
    Enabled,
    /// The composing text changed.
    ///
    /// `cursor` is the range the input method wants marked, in bytes into
    /// `text`. `None` means it wants no caret drawn. An empty `text` ends the
    /// composition without committing anything.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// Text was accepted and should be inserted.
    Commit(String),
    /// Composition is no longer possible. Anything half composed is abandoned.
    Disabled,
}

/// Files handed to the window by the desktop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDropEvent {
    pub paths: Vec<std::path::PathBuf>,
    /// Where the pointer was. The platform does not report a position with a
    /// drop on every system, so this is the last place the pointer was seen,
    /// which is where the drop happened.
    pub position: Point<Px>,
    pub modifiers: Modifiers,
}

impl InputEvent {
    /// Where the pointer was, for events that involve one.
    pub fn position(&self) -> Option<Point<Px>> {
        match self {
            InputEvent::MouseMove(e) | InputEvent::MouseDown(e) | InputEvent::MouseUp(e) => {
                Some(e.position)
            }
            InputEvent::Scroll(e) => Some(e.position),
            InputEvent::FileDrop(e) | InputEvent::FileHover(e) => Some(e.position),
            _ => None,
        }
    }

    /// Whether handling this event should move keyboard focus.
    pub fn is_focus_change(&self) -> bool {
        matches!(self, InputEvent::MouseDown(_))
    }
}

/// Which edge or corner a resize drag pulls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl ResizeEdge {
    /// The pointer shape that signals this edge.
    pub fn cursor(self) -> guirs_style::CursorStyle {
        use guirs_style::CursorStyle;
        match self {
            ResizeEdge::North | ResizeEdge::South => CursorStyle::ResizeVertical,
            ResizeEdge::East | ResizeEdge::West => CursorStyle::ResizeHorizontal,
            ResizeEdge::NorthWest | ResizeEdge::SouthEast => CursorStyle::ResizeNwSe,
            ResizeEdge::NorthEast | ResizeEdge::SouthWest => CursorStyle::ResizeNeSw,
        }
    }
}

/// Something an element asked the platform window to do.
///
/// Requests are queued rather than performed inline, because an event handler
/// runs deep inside the element tree with no access to the window, and because
/// a drag or a resize has to be handed to the platform while the press that
/// started it is still live.
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    /// Close this window. The application exits when the last one goes.
    Close,
    /// Let the platform move the window with the pointer.
    StartDrag,
    /// Let the platform resize the window from an edge.
    StartResize(ResizeEdge),
    /// Open another window, with its own root and its own options.
    ///
    /// Boxed because a window carries the closure that draws it, which is why
    /// this enum is neither `Copy` nor comparable.
    Open(Box<NewWindow>),
    /// Set this window's title.
    SetTitle(String),
}

// Two requests to open a window are never the same request: each carries the
// closure that draws it, and closures have no identity worth comparing. Every
// other action is plain data and compares as such.
impl PartialEq for WindowAction {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (WindowAction::Minimize, WindowAction::Minimize)
            | (WindowAction::ToggleMaximize, WindowAction::ToggleMaximize)
            | (WindowAction::Close, WindowAction::Close)
            | (WindowAction::StartDrag, WindowAction::StartDrag) => true,
            (WindowAction::StartResize(a), WindowAction::StartResize(b)) => a == b,
            (WindowAction::SetTitle(a), WindowAction::SetTitle(b)) => a == b,
            _ => false,
        }
    }
}

impl std::fmt::Debug for WindowAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowAction::Minimize => write!(f, "Minimize"),
            WindowAction::ToggleMaximize => write!(f, "ToggleMaximize"),
            WindowAction::Close => write!(f, "Close"),
            WindowAction::StartDrag => write!(f, "StartDrag"),
            WindowAction::StartResize(edge) => write!(f, "StartResize({edge:?})"),
            WindowAction::Open(spec) => write!(f, "Open({:?})", spec.options.title),
            WindowAction::SetTitle(title) => write!(f, "SetTitle({title:?})"),
        }
    }
}

/// A window an application asked for while running.
pub struct NewWindow {
    pub options: crate::app::WindowOptions,
    pub root: crate::window::RootFn,
}

impl NewWindow {
    pub fn new(
        options: crate::app::WindowOptions,
        root: impl FnMut(&mut crate::context::Cx) -> crate::element::AnyElement + 'static,
    ) -> Self {
        NewWindow {
            options,
            root: Box::new(root),
        }
    }
}

/// Tracks multi click sequences.
///
/// A double click is two presses close together in both time and space. The
/// distance check matters: without it, clicking two different buttons in quick
/// succession registers as a double click on the second.
#[derive(Debug)]
pub struct ClickTracker {
    last_time: f64,
    last_position: Point<Px>,
    count: u32,
    /// Seconds within which a second press counts as a double click.
    pub interval: f64,
    /// How far the pointer may move between presses, in logical pixels.
    pub slop: f32,
}

impl Default for ClickTracker {
    fn default() -> Self {
        ClickTracker {
            last_time: f64::NEG_INFINITY,
            last_position: Point::zero(),
            count: 0,
            interval: 0.5,
            slop: 4.0,
        }
    }
}

impl ClickTracker {
    /// Record a press and return how many clicks it makes in a row.
    pub fn press(&mut self, position: Point<Px>, now: f64) -> u32 {
        let in_time = now - self.last_time <= self.interval;
        let in_place = self.last_position.distance_to(position) <= self.slop;
        self.count = if in_time && in_place { self.count + 1 } else { 1 };
        self.last_time = now;
        self.last_position = position;
        self.count
    }

    /// Forget the sequence, so the next press starts a new one.
    pub fn reset(&mut self) {
        self.count = 0;
        self.last_time = f64::NEG_INFINITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> Point<Px> {
        Point::new(Px(x), Px(y))
    }

    #[test]
    fn repeated_presses_in_place_escalate() {
        let mut tracker = ClickTracker::default();
        assert_eq!(tracker.press(at(10.0, 10.0), 0.0), 1);
        assert_eq!(tracker.press(at(10.0, 10.0), 0.1), 2);
        assert_eq!(tracker.press(at(11.0, 10.0), 0.2), 3);
    }

    #[test]
    fn a_slow_second_press_starts_over() {
        let mut tracker = ClickTracker::default();
        assert_eq!(tracker.press(at(10.0, 10.0), 0.0), 1);
        assert_eq!(tracker.press(at(10.0, 10.0), 2.0), 1);
    }

    #[test]
    fn a_distant_second_press_starts_over() {
        let mut tracker = ClickTracker::default();
        assert_eq!(tracker.press(at(10.0, 10.0), 0.0), 1);
        assert_eq!(tracker.press(at(200.0, 10.0), 0.05), 1);
    }

    #[test]
    fn resetting_clears_the_sequence() {
        let mut tracker = ClickTracker::default();
        tracker.press(at(0.0, 0.0), 0.0);
        tracker.press(at(0.0, 0.0), 0.05);
        tracker.reset();
        assert_eq!(tracker.press(at(0.0, 0.0), 0.06), 1);
    }

    #[test]
    fn modifiers_report_emptiness() {
        assert!(Modifiers::default().none());
        assert!(!Modifiers {
            shift: true,
            ..Default::default()
        }
        .none());
    }

    #[test]
    fn shortcuts_use_the_platform_accelerator() {
        let mut modifiers = Modifiers::default();
        #[cfg(target_os = "macos")]
        {
            modifiers.meta = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            modifiers.control = true;
        }
        let event = KeyEvent {
            key: Key::Character('c'),
            modifiers,
            repeat: false,
        };
        assert!(event.is_shortcut('c'));
        assert!(!event.is_shortcut('v'));
        assert!(!event.is_plain(Key::Character('c')));
    }

    #[test]
    fn local_coordinates_and_fractions_track_the_element() {
        let bounds = Bounds::from_xywh(Px(100.0), Px(50.0), Px(200.0), Px(20.0));
        let event = MouseEvent {
            position: at(150.0, 60.0),
            ..Default::default()
        }
        .relative_to(bounds);

        assert_eq!(event.local, at(50.0, 10.0));
        assert_eq!(event.fraction().x, 0.25);
        assert_eq!(event.fraction().y, 0.5);
    }

    #[test]
    fn fractions_clamp_when_the_pointer_leaves_the_element() {
        let bounds = Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(10.0));
        let past_end = MouseEvent {
            position: at(500.0, 5.0),
            ..Default::default()
        }
        .relative_to(bounds);
        assert_eq!(past_end.fraction().x, 1.0);

        let before_start = MouseEvent {
            position: at(-40.0, 5.0),
            ..Default::default()
        }
        .relative_to(bounds);
        assert_eq!(before_start.fraction().x, 0.0);
    }

    #[test]
    fn a_zero_width_element_yields_a_zero_fraction() {
        let event = MouseEvent::default().relative_to(Bounds::zero());
        assert_eq!(event.fraction().x, 0.0);
    }

    #[test]
    fn only_pointer_events_report_a_position() {
        let event = InputEvent::MouseMove(MouseEvent {
            position: at(5.0, 6.0),
            ..Default::default()
        });
        assert_eq!(event.position(), Some(at(5.0, 6.0)));
        assert_eq!(InputEvent::MouseLeave.position(), None);
        assert_eq!(
            InputEvent::WindowFocus(true).position(),
            None
        );
    }
}
