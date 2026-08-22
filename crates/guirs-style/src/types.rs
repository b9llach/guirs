//! The value types a style property can hold.
//!
//! These are the vocabulary shared by the stylesheet parser and the typed Rust
//! builder API, so both authoring paths land on exactly the same representation.

use guirs_core::{Px, SharedString};

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// How an element establishes a formatting context for its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Display {
    /// Children are laid out with flexbox.
    #[default]
    Flex,
    /// Children stack vertically and fill the width.
    Block,
    /// Children are placed into rows and columns declared up front.
    Grid,
    /// The element and its subtree are not laid out or painted.
    None,
}

/// How wide or tall one row or column of a grid is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TrackSize {
    /// As big as what is in it.
    #[default]
    Auto,
    /// A fixed size.
    Px(crate::Px),
    /// A share of the container.
    Percent(f32),
    /// A share of whatever is left after the fixed tracks have had theirs.
    /// This is what makes a grid a grid rather than a table of fixed columns.
    Fr(f32),
    /// The smallest it can be without its contents overflowing.
    MinContent,
    /// As big as its contents would like to be.
    MaxContent,
}

/// Where an item sits on one axis of a grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridPlacement {
    /// Wherever the grid puts it.
    #[default]
    Auto,
    /// Against a numbered line. Lines are counted from one, and negative
    /// numbers count back from the end, so `-1` is the last line.
    Line(i16),
    /// Across this many tracks, starting wherever the grid puts it.
    Span(u16),
}

/// Where an item starts and ends on one axis of a grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridLine {
    pub start: GridPlacement,
    pub end: GridPlacement,
}

impl GridLine {
    /// Across this many tracks, starting wherever the grid puts it.
    ///
    /// The span goes on the start, which is what `grid-column: span 3` means
    /// in CSS: begin somewhere and take three.
    pub fn span(tracks: u16) -> Self {
        GridLine {
            start: GridPlacement::Span(tracks.max(1)),
            end: GridPlacement::Auto,
        }
    }

    /// From one line to another, the way `grid-column: 2 / 5` reads.
    pub fn between(start: i16, end: i16) -> Self {
        GridLine {
            start: GridPlacement::Line(start),
            end: GridPlacement::Line(end),
        }
    }
}

/// Whether an element participates in normal flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Position {
    #[default]
    Relative,
    /// Removed from flow and positioned against the nearest positioned ancestor.
    Absolute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

impl FlexDirection {
    #[inline]
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

/// Distribution along the main axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch,
}

/// Alignment along the cross axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum AlignItems {
    #[default]
    Stretch,
    Start,
    Center,
    End,
    Baseline,
}

/// What happens to content that overflows the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Overflow {
    #[default]
    Visible,
    /// Clipped, with no scrollbar and no scrolling.
    Hidden,
    /// Clipped and scrollable, with the scrollbar always reserved.
    Scroll,
    /// Clipped and scrollable, with the scrollbar shown only when needed.
    Auto,
}

impl Overflow {
    #[inline]
    pub fn clips(self) -> bool {
        !matches!(self, Overflow::Visible)
    }

    #[inline]
    pub fn scrolls(self) -> bool {
        matches!(self, Overflow::Scroll | Overflow::Auto)
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// A CSS style numeric font weight, `1..=1000`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: FontWeight = FontWeight(100);
    pub const EXTRA_LIGHT: FontWeight = FontWeight(200);
    pub const LIGHT: FontWeight = FontWeight(300);
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const MEDIUM: FontWeight = FontWeight(500);
    pub const SEMIBOLD: FontWeight = FontWeight(600);
    pub const BOLD: FontWeight = FontWeight(700);
    pub const EXTRA_BOLD: FontWeight = FontWeight(800);
    pub const BLACK: FontWeight = FontWeight(900);
}

impl Default for FontWeight {
    fn default() -> Self {
        FontWeight::NORMAL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// The vertical advance between baselines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum LineHeight {
    /// Derived from the font's own metrics.
    #[default]
    Normal,
    /// An absolute height.
    Px(Px),
    /// A multiple of the font size.
    Multiple(OrderedF32),
}

impl LineHeight {
    /// Resolve against a font size and the font's natural line height.
    pub fn resolve(self, font_size: Px, natural: Px) -> Px {
        match self {
            LineHeight::Normal => natural,
            LineHeight::Px(v) => v,
            LineHeight::Multiple(m) => font_size * m.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// How whitespace and line breaking are handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum WhiteSpace {
    /// Collapse runs of whitespace and wrap at the box width.
    #[default]
    Normal,
    /// Collapse runs of whitespace but never wrap.
    NoWrap,
    /// Preserve whitespace and break only at explicit newlines.
    Pre,
    /// Preserve whitespace and also wrap at the box width.
    PreWrap,
}

impl WhiteSpace {
    #[inline]
    pub fn wraps(self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::PreWrap)
    }

    #[inline]
    pub fn preserves(self) -> bool {
        matches!(self, WhiteSpace::Pre | WhiteSpace::PreWrap)
    }
}

/// Lines drawn through or under a text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TextDecoration {
    pub underline: bool,
    pub strikethrough: bool,
}

impl TextDecoration {
    pub const NONE: TextDecoration = TextDecoration {
        underline: false,
        strikethrough: false,
    };

    pub const UNDERLINE: TextDecoration = TextDecoration {
        underline: true,
        strikethrough: false,
    };

    #[inline]
    pub fn any(self) -> bool {
        self.underline || self.strikethrough
    }
}

/// How text that does not fit is truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// The pointer shape shown over an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum CursorStyle {
    #[default]
    Default,
    Pointer,
    Text,
    Crosshair,
    Move,
    Grab,
    Grabbing,
    NotAllowed,
    Wait,
    ResizeHorizontal,
    ResizeVertical,
    ResizeNwSe,
    ResizeNeSw,
    ColResize,
    RowResize,
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

/// An `f32` that is hashable and totally ordered.
///
/// Style values live in hash maps and get compared for equality to decide
/// whether a repaint is needed, so every float inside a style needs these.
#[derive(Clone, Copy, Debug, Default, PartialOrd)]
pub struct OrderedF32(pub f32);

impl PartialEq for OrderedF32 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for OrderedF32 {}

impl std::hash::Hash for OrderedF32 {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl From<f32> for OrderedF32 {
    fn from(v: f32) -> Self {
        OrderedF32(v)
    }
}

/// A timing curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Easing {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    EaseInOut,
    /// The four control point coordinates of a unit cubic Bezier.
    CubicBezier(OrderedF32, OrderedF32, OrderedF32, OrderedF32),
    /// Jump between `n` discrete values.
    Steps(u16),
}

impl Easing {
    /// The named CSS `ease` curve.
    pub const EASE: Easing = Easing::CubicBezier(
        OrderedF32(0.25),
        OrderedF32(0.1),
        OrderedF32(0.25),
        OrderedF32(1.0),
    );

    /// Map linear progress `t` to eased progress.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => {
                let u = 1.0 - t;
                1.0 - u * u * u
            }
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let u = -2.0 * t + 2.0;
                    1.0 - u * u * u / 2.0
                }
            }
            Easing::CubicBezier(x1, y1, x2, y2) => {
                cubic_bezier(x1.0, y1.0, x2.0, y2.0, t)
            }
            Easing::Steps(n) => {
                if n == 0 {
                    t
                } else {
                    (t * n as f32).floor() / n as f32
                }
            }
        }
    }
}

/// Evaluate a unit cubic Bezier easing curve at `x`.
///
/// The curve is parametric, so finding `y` for a given `x` means inverting the
/// `x` component first. Newton iteration converges in a handful of steps for the
/// well behaved curves easing functions use; the bisection fallback guarantees
/// termination for the pathological ones.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    #[inline]
    fn bezier(a: f32, b: f32, t: f32) -> f32 {
        let inv = 1.0 - t;
        3.0 * inv * inv * t * a + 3.0 * inv * t * t * b + t * t * t
    }

    #[inline]
    fn bezier_derivative(a: f32, b: f32, t: f32) -> f32 {
        let inv = 1.0 - t;
        3.0 * inv * inv * a + 6.0 * inv * t * (b - a) + 3.0 * t * t * (1.0 - b)
    }

    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }

    let mut t = x;
    for _ in 0..8 {
        let error = bezier(x1, x2, t) - x;
        if error.abs() < 1e-5 {
            return bezier(y1, y2, t);
        }
        let slope = bezier_derivative(x1, x2, t);
        if slope.abs() < 1e-6 {
            break;
        }
        t -= error / slope;
    }

    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..24 {
        let value = bezier(x1, x2, t);
        if (value - x).abs() < 1e-5 {
            break;
        }
        if value < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    bezier(y1, y2, t)
}

/// A style property that can be smoothly interpolated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimatableProperty {
    Background,
    BorderColor,
    BorderWidth,
    BorderRadius,
    BoxShadow,
    Color,
    Opacity,
    Width,
    Height,
    Padding,
    Margin,
    FontSize,
    LetterSpacing,
    Inset,
    Transform,
}

impl AnimatableProperty {
    pub const ALL: [AnimatableProperty; 15] = [
        AnimatableProperty::Background,
        AnimatableProperty::BorderColor,
        AnimatableProperty::BorderWidth,
        AnimatableProperty::BorderRadius,
        AnimatableProperty::BoxShadow,
        AnimatableProperty::Color,
        AnimatableProperty::Opacity,
        AnimatableProperty::Width,
        AnimatableProperty::Height,
        AnimatableProperty::Padding,
        AnimatableProperty::Margin,
        AnimatableProperty::FontSize,
        AnimatableProperty::LetterSpacing,
        AnimatableProperty::Inset,
        AnimatableProperty::Transform,
    ];

    pub fn from_name(name: &str) -> Option<AnimatableProperty> {
        Some(match name {
            "background" | "background-color" => AnimatableProperty::Background,
            "border-color" => AnimatableProperty::BorderColor,
            "border-width" => AnimatableProperty::BorderWidth,
            "border-radius" => AnimatableProperty::BorderRadius,
            "box-shadow" => AnimatableProperty::BoxShadow,
            "color" => AnimatableProperty::Color,
            "opacity" => AnimatableProperty::Opacity,
            "width" => AnimatableProperty::Width,
            "height" => AnimatableProperty::Height,
            "padding" => AnimatableProperty::Padding,
            "margin" => AnimatableProperty::Margin,
            "font-size" => AnimatableProperty::FontSize,
            "letter-spacing" => AnimatableProperty::LetterSpacing,
            "inset" | "top" | "right" | "bottom" | "left" => AnimatableProperty::Inset,
            "transform" | "translate" | "rotate" | "scale" => AnimatableProperty::Transform,
            _ => return None,
        })
    }
}

/// Which properties a transition applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TransitionTarget {
    #[default]
    All,
    One(AnimatableProperty),
}

impl TransitionTarget {
    #[inline]
    pub fn matches(self, property: AnimatableProperty) -> bool {
        match self {
            TransitionTarget::All => true,
            TransitionTarget::One(p) => p == property,
        }
    }
}

/// A single `transition` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Transition {
    pub target: TransitionTarget,
    pub duration_ms: OrderedF32,
    pub delay_ms: OrderedF32,
    pub easing: Easing,
}

impl Default for Transition {
    fn default() -> Self {
        Transition {
            target: TransitionTarget::All,
            duration_ms: OrderedF32(150.0),
            delay_ms: OrderedF32(0.0),
            easing: Easing::EaseOut,
        }
    }
}

impl Transition {
    pub fn new(target: TransitionTarget, duration_ms: f32) -> Self {
        Transition {
            target,
            duration_ms: OrderedF32(duration_ms),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// A prioritized list of font family names.
///
/// Resolution walks the list and takes the first family the font database
/// knows about, then falls back to the system UI font.
///
/// The list is behind an `Arc` because font family is an inherited property:
/// every element's computed style carries one, and a computed style is cloned
/// several times per element per frame. A `Vec` here meant an allocation on
/// every one of those clones, which dominated frame time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFamily(pub std::sync::Arc<[SharedString]>);

impl Default for FontFamily {
    fn default() -> Self {
        FontFamily(std::sync::Arc::from(Vec::new()))
    }
}

impl FontFamily {
    pub fn single(name: impl Into<SharedString>) -> Self {
        FontFamily(std::sync::Arc::from(vec![name.into()]))
    }

    /// Build from any sequence of names.
    pub fn new(names: impl IntoIterator<Item = SharedString>) -> Self {
        FontFamily(std::sync::Arc::from(names.into_iter().collect::<Vec<_>>()))
    }

    /// The names, in priority order.
    #[inline]
    pub fn names(&self) -> &[SharedString] {
        &self.0
    }

    #[inline]
    pub fn primary(&self) -> Option<&SharedString> {
        self.0.first()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for FontFamily {
    fn from(name: &str) -> Self {
        FontFamily::single(name)
    }
}

impl From<String> for FontFamily {
    fn from(name: String) -> Self {
        FontFamily::single(name)
    }
}

impl From<SharedString> for FontFamily {
    fn from(name: SharedString) -> Self {
        FontFamily::single(name)
    }
}

impl<T: Into<SharedString>, const N: usize> From<[T; N]> for FontFamily {
    fn from(names: [T; N]) -> Self {
        FontFamily::new(names.into_iter().map(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves_are_pinned_at_both_ends() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::EASE,
        ] {
            assert!((e.apply(0.0) - 0.0).abs() < 1e-4, "start of {e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-4, "end of {e:?}");
        }
    }

    #[test]
    fn easing_is_monotonic_for_standard_curves() {
        for e in [Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut, Easing::EASE] {
            let mut previous = -1.0;
            for i in 0..=50 {
                let v = e.apply(i as f32 / 50.0);
                assert!(v >= previous - 1e-4, "{e:?} went backwards at {i}");
                previous = v;
            }
        }
    }

    #[test]
    fn ease_out_starts_faster_than_linear() {
        assert!(Easing::EaseOut.apply(0.25) > 0.25);
        assert!(Easing::EaseIn.apply(0.25) < 0.25);
    }

    #[test]
    fn cubic_bezier_matches_the_named_ease_curve() {
        // The midpoint of the CSS `ease` curve is a well known value.
        let mid = Easing::EASE.apply(0.5);
        assert!((mid - 0.8024).abs() < 0.01, "got {mid}");
    }

    #[test]
    fn steps_quantizes_progress() {
        let e = Easing::Steps(4);
        assert_eq!(e.apply(0.0), 0.0);
        assert_eq!(e.apply(0.3), 0.25);
        assert_eq!(e.apply(0.6), 0.5);
    }

    #[test]
    fn line_height_resolution() {
        assert_eq!(
            LineHeight::Multiple(OrderedF32(1.5)).resolve(Px(16.0), Px(19.0)),
            Px(24.0)
        );
        assert_eq!(LineHeight::Normal.resolve(Px(16.0), Px(19.0)), Px(19.0));
    }
}
