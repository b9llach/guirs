//! Specified and computed styles.
//!
//! [`Style`] is a sparse set of overrides: every field is optional, and unset
//! fields fall through to whatever the cascade produced beneath them. It is what
//! a stylesheet rule holds, and what the typed builder API produces.
//!
//! [`ComputedStyle`] is the dense result. Every field has a concrete value,
//! inherited properties have been pulled down from the parent, and relative
//! lengths that depend only on font metrics have been resolved. Layout and
//! painting only ever see a `ComputedStyle`.

use guirs_core::{
    BorderStyle, BoxShadow, Corners, Edges, Length, Paint, Px, Rgba, SharedString, Size,
    Transform2D,
};
use smallvec::{smallvec, SmallVec};

use crate::types::*;

/// Shadows are usually one or two deep, so keep them inline.
pub type ShadowList = SmallVec<[BoxShadow; 2]>;
/// Likewise for transitions.
pub type TransitionList = SmallVec<[Transition; 4]>;

/// A sparse set of style overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    // Layout
    pub display: Option<Display>,
    pub position: Option<Position>,
    pub top: Option<Length>,
    pub right: Option<Length>,
    pub bottom: Option<Length>,
    pub left: Option<Length>,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub min_width: Option<Length>,
    pub min_height: Option<Length>,
    pub max_width: Option<Length>,
    pub max_height: Option<Length>,
    pub aspect_ratio: Option<f32>,
    pub margin_top: Option<Length>,
    pub margin_right: Option<Length>,
    pub margin_bottom: Option<Length>,
    pub margin_left: Option<Length>,
    pub padding_top: Option<Length>,
    pub padding_right: Option<Length>,
    pub padding_bottom: Option<Length>,
    pub padding_left: Option<Length>,
    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<Length>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub align_self: Option<AlignItems>,
    pub align_content: Option<JustifyContent>,
    pub row_gap: Option<Length>,
    pub column_gap: Option<Length>,
    pub overflow_x: Option<Overflow>,
    pub overflow_y: Option<Overflow>,

    // Paint
    pub background: Option<Paint>,
    pub border_top_width: Option<Px>,
    pub border_right_width: Option<Px>,
    pub border_bottom_width: Option<Px>,
    pub border_left_width: Option<Px>,
    pub border_color: Option<Rgba>,
    pub border_style: Option<BorderStyle>,
    pub radius_top_left: Option<Px>,
    pub radius_top_right: Option<Px>,
    pub radius_bottom_right: Option<Px>,
    pub radius_bottom_left: Option<Px>,
    pub box_shadows: Option<ShadowList>,
    pub opacity: Option<f32>,
    /// Kept decomposed rather than as a matrix, because this is the form that
    /// animates the way a reader expects.
    pub transform: Option<Transform2D>,

    // Inherited
    pub color: Option<Rgba>,
    pub font_family: Option<FontFamily>,
    pub font_size: Option<Length>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub line_height: Option<LineHeight>,
    pub letter_spacing: Option<Length>,
    pub text_align: Option<TextAlign>,
    pub white_space: Option<WhiteSpace>,
    pub text_decoration: Option<TextDecoration>,
    pub text_overflow: Option<TextOverflow>,
    pub cursor: Option<CursorStyle>,
    /// Whether this element takes part in hit testing. Inherited, so turning it
    /// off on a container excuses the whole subtree.
    pub pointer_events: Option<bool>,

    // Behaviour
    pub transitions: Option<TransitionList>,
}

macro_rules! merge_fields {
    ($self:ident, $other:ident, $($field:ident),+ $(,)?) => {
        $(
            if $other.$field.is_some() {
                $self.$field = $other.$field.clone();
            }
        )+
    };
}

impl Style {
    pub fn new() -> Self {
        Style::default()
    }

    /// Overlay `other` on top of `self`. Fields set in `other` win.
    pub fn merge_from(&mut self, other: &Style) {
        merge_fields!(
            self,
            other,
            display,
            position,
            top,
            right,
            bottom,
            left,
            width,
            height,
            min_width,
            min_height,
            max_width,
            max_height,
            aspect_ratio,
            margin_top,
            margin_right,
            margin_bottom,
            margin_left,
            padding_top,
            padding_right,
            padding_bottom,
            padding_left,
            flex_direction,
            flex_wrap,
            flex_grow,
            flex_shrink,
            flex_basis,
            justify_content,
            align_items,
            align_self,
            align_content,
            row_gap,
            column_gap,
            overflow_x,
            overflow_y,
            background,
            border_top_width,
            border_right_width,
            border_bottom_width,
            border_left_width,
            border_color,
            border_style,
            radius_top_left,
            radius_top_right,
            radius_bottom_right,
            radius_bottom_left,
            box_shadows,
            opacity,
            transform,
            color,
            font_family,
            font_size,
            font_weight,
            font_style,
            line_height,
            letter_spacing,
            text_align,
            white_space,
            text_decoration,
            text_overflow,
            cursor,
            pointer_events,
            transitions,
        );
    }

    /// Overlay `other` and return the result.
    pub fn merged(mut self, other: &Style) -> Style {
        self.merge_from(other);
        self
    }

    /// Resolve into a dense style, inheriting from `parent`.
    pub fn resolve(&self, parent: &ComputedStyle, root_font_size: Px) -> ComputedStyle {
        // Font size resolves first, because everything expressed in `em` or as
        // a percentage of the font size depends on it.
        let font_size = match self.font_size {
            None => parent.font_size,
            Some(Length::Px(v)) => v,
            Some(Length::Rems(v)) => v.to_px(root_font_size),
            Some(Length::Percent(f)) => parent.font_size * f,
            Some(Length::Auto) | Some(Length::Fraction(_)) => parent.font_size,
        };

        let resolve_len = |value: Option<Length>, fallback: Length| -> Length {
            match value {
                Some(Length::Rems(v)) => Length::Px(v.to_px(root_font_size)),
                Some(other) => other,
                None => fallback,
            }
        };

        let letter_spacing = match self.letter_spacing {
            None => parent.letter_spacing,
            Some(Length::Px(v)) => v,
            Some(Length::Rems(v)) => v.to_px(root_font_size),
            Some(Length::Percent(f)) => font_size * f,
            Some(_) => Px::ZERO,
        };

        ComputedStyle {
            display: self.display.unwrap_or_default(),
            position: self.position.unwrap_or_default(),
            inset: Edges {
                top: resolve_len(self.top, Length::Auto),
                right: resolve_len(self.right, Length::Auto),
                bottom: resolve_len(self.bottom, Length::Auto),
                left: resolve_len(self.left, Length::Auto),
            },
            size: Size {
                width: resolve_len(self.width, Length::Auto),
                height: resolve_len(self.height, Length::Auto),
            },
            min_size: Size {
                width: resolve_len(self.min_width, Length::Auto),
                height: resolve_len(self.min_height, Length::Auto),
            },
            max_size: Size {
                width: resolve_len(self.max_width, Length::Auto),
                height: resolve_len(self.max_height, Length::Auto),
            },
            aspect_ratio: self.aspect_ratio,
            margin: Edges {
                top: resolve_len(self.margin_top, Length::Px(Px::ZERO)),
                right: resolve_len(self.margin_right, Length::Px(Px::ZERO)),
                bottom: resolve_len(self.margin_bottom, Length::Px(Px::ZERO)),
                left: resolve_len(self.margin_left, Length::Px(Px::ZERO)),
            },
            padding: Edges {
                top: resolve_len(self.padding_top, Length::Px(Px::ZERO)),
                right: resolve_len(self.padding_right, Length::Px(Px::ZERO)),
                bottom: resolve_len(self.padding_bottom, Length::Px(Px::ZERO)),
                left: resolve_len(self.padding_left, Length::Px(Px::ZERO)),
            },
            flex_direction: self.flex_direction.unwrap_or_default(),
            flex_wrap: self.flex_wrap.unwrap_or_default(),
            flex_grow: self.flex_grow.unwrap_or(0.0),
            flex_shrink: self.flex_shrink.unwrap_or(1.0),
            flex_basis: resolve_len(self.flex_basis, Length::Auto),
            justify_content: self.justify_content.unwrap_or_default(),
            align_items: self.align_items.unwrap_or_default(),
            align_self: self.align_self,
            align_content: self.align_content.unwrap_or_default(),
            gap: Size {
                width: resolve_len(self.column_gap, Length::Px(Px::ZERO)),
                height: resolve_len(self.row_gap, Length::Px(Px::ZERO)),
            },
            overflow_x: self.overflow_x.unwrap_or_default(),
            overflow_y: self.overflow_y.unwrap_or_default(),

            background: self.background.clone().unwrap_or(Paint::None),
            border_widths: Edges {
                top: self.border_top_width.unwrap_or(Px::ZERO),
                right: self.border_right_width.unwrap_or(Px::ZERO),
                bottom: self.border_bottom_width.unwrap_or(Px::ZERO),
                left: self.border_left_width.unwrap_or(Px::ZERO),
            },
            border_color: self.border_color.unwrap_or(Rgba::TRANSPARENT),
            border_style: self.border_style.unwrap_or(BorderStyle::Solid),
            border_radius: Corners {
                top_left: self.radius_top_left.unwrap_or(Px::ZERO),
                top_right: self.radius_top_right.unwrap_or(Px::ZERO),
                bottom_right: self.radius_bottom_right.unwrap_or(Px::ZERO),
                bottom_left: self.radius_bottom_left.unwrap_or(Px::ZERO),
            },
            box_shadows: self.box_shadows.clone().unwrap_or_default(),
            opacity: self.opacity.unwrap_or(1.0),
            // Not inherited. A transform applies to a subtree by being applied
            // to the element that carries it, so a child inheriting it too
            // would apply it twice.
            transform: self.transform.unwrap_or(Transform2D::IDENTITY),

            color: self.color.unwrap_or(parent.color),
            font_family: self
                .font_family
                .clone()
                .unwrap_or_else(|| parent.font_family.clone()),
            font_size,
            font_weight: self.font_weight.unwrap_or(parent.font_weight),
            font_style: self.font_style.unwrap_or(parent.font_style),
            line_height: self.line_height.unwrap_or(parent.line_height),
            letter_spacing,
            text_align: self.text_align.unwrap_or(parent.text_align),
            white_space: self.white_space.unwrap_or(parent.white_space),
            text_decoration: self.text_decoration.unwrap_or(parent.text_decoration),
            text_overflow: self.text_overflow.unwrap_or(parent.text_overflow),
            cursor: self.cursor.unwrap_or(parent.cursor),
            pointer_events: self.pointer_events.unwrap_or(parent.pointer_events),
            transitions: self.transitions.clone().unwrap_or_default(),
        }
    }
}

impl Styled for Style {
    #[inline]
    fn style_mut(&mut self) -> &mut Style {
        self
    }
}

// ---------------------------------------------------------------------------
// ComputedStyle
// ---------------------------------------------------------------------------

/// A fully resolved style. Every field is concrete.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub inset: Edges<Length>,
    pub size: Size<Length>,
    pub min_size: Size<Length>,
    pub max_size: Size<Length>,
    pub aspect_ratio: Option<f32>,
    pub margin: Edges<Length>,
    pub padding: Edges<Length>,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub align_self: Option<AlignItems>,
    pub align_content: JustifyContent,
    /// `width` is the column gap, `height` the row gap.
    pub gap: Size<Length>,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,

    pub background: Paint,
    pub border_widths: Edges<Px>,
    pub border_color: Rgba,
    pub border_style: BorderStyle,
    pub border_radius: Corners<Px>,
    pub box_shadows: ShadowList,
    pub opacity: f32,
    pub transform: Transform2D,

    pub color: Rgba,
    pub font_family: FontFamily,
    pub font_size: Px,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub line_height: LineHeight,
    pub letter_spacing: Px,
    pub text_align: TextAlign,
    pub white_space: WhiteSpace,
    pub text_decoration: TextDecoration,
    pub text_overflow: TextOverflow,
    pub cursor: CursorStyle,
    pub pointer_events: bool,

    pub transitions: TransitionList,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Flex,
            position: Position::Relative,
            inset: Edges {
                top: Length::Auto,
                right: Length::Auto,
                bottom: Length::Auto,
                left: Length::Auto,
            },
            size: Size {
                width: Length::Auto,
                height: Length::Auto,
            },
            min_size: Size {
                width: Length::Auto,
                height: Length::Auto,
            },
            max_size: Size {
                width: Length::Auto,
                height: Length::Auto,
            },
            aspect_ratio: None,
            margin: Edges {
                top: Length::Px(Px::ZERO),
                right: Length::Px(Px::ZERO),
                bottom: Length::Px(Px::ZERO),
                left: Length::Px(Px::ZERO),
            },
            padding: Edges {
                top: Length::Px(Px::ZERO),
                right: Length::Px(Px::ZERO),
                bottom: Length::Px(Px::ZERO),
                left: Length::Px(Px::ZERO),
            },
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            align_self: None,
            align_content: JustifyContent::Start,
            gap: Size {
                width: Length::Px(Px::ZERO),
                height: Length::Px(Px::ZERO),
            },
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,

            background: Paint::None,
            border_widths: Edges {
                top: Px::ZERO,
                right: Px::ZERO,
                bottom: Px::ZERO,
                left: Px::ZERO,
            },
            border_color: Rgba::TRANSPARENT,
            border_style: BorderStyle::Solid,
            border_radius: Corners {
                top_left: Px::ZERO,
                top_right: Px::ZERO,
                bottom_right: Px::ZERO,
                bottom_left: Px::ZERO,
            },
            box_shadows: smallvec![],
            opacity: 1.0,
            transform: Transform2D::IDENTITY,

            color: Rgba::WHITE,
            font_family: FontFamily::default(),
            font_size: Px(14.0),
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::Normal,
            line_height: LineHeight::Normal,
            letter_spacing: Px::ZERO,
            text_align: TextAlign::Start,
            white_space: WhiteSpace::Normal,
            text_decoration: TextDecoration::NONE,
            text_overflow: TextOverflow::Clip,
            cursor: CursorStyle::Default,
            pointer_events: true,

            transitions: smallvec![],
        }
    }
}

impl ComputedStyle {
    /// Whether this element paints anything of its own, ignoring children.
    pub fn paints(&self) -> bool {
        self.background.is_visible()
            || (self.border_style.draws()
                && !self.border_color.is_transparent()
                && !self.border_widths.is_zero())
            || self.box_shadows.iter().any(|s| s.is_visible())
    }

    /// Whether children must be clipped to this element's padding box.
    #[inline]
    pub fn clips_content(&self) -> bool {
        self.overflow_x.clips() || self.overflow_y.clips()
    }

    /// The transition entry governing `property`, if any.
    ///
    /// Later entries win, matching how a `transition` shorthand list is read.
    pub fn transition_for(&self, property: AnimatableProperty) -> Option<Transition> {
        self.transitions
            .iter()
            .rev()
            .find(|t| t.target.matches(property))
            .copied()
    }

    /// Whether any transition is declared.
    #[inline]
    pub fn has_transitions(&self) -> bool {
        !self.transitions.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Builder API
// ---------------------------------------------------------------------------

/// The typed styling API.
///
/// Anything that owns a [`Style`] gets the full property set by implementing
/// the single required method. Elements forward to their own style, so
/// `div().bg(color).rounded(8.0)` and `Style::new().bg(color).rounded(8.0)`
/// are the same calls against the same fields.
pub trait Styled: Sized {
    fn style_mut(&mut self) -> &mut Style;

    // -- display and flow --

    fn display(mut self, value: Display) -> Self {
        self.style_mut().display = Some(value);
        self
    }

    /// Lay children out with flexbox.
    fn flex(mut self) -> Self {
        self.style_mut().display = Some(Display::Flex);
        self
    }

    /// Stack children vertically, each filling the width.
    fn block(mut self) -> Self {
        self.style_mut().display = Some(Display::Block);
        self
    }

    /// Remove from layout and painting entirely.
    fn hidden(mut self) -> Self {
        self.style_mut().display = Some(Display::None);
        self
    }

    /// Arrange children left to right.
    fn row(mut self) -> Self {
        let s = self.style_mut();
        s.display.get_or_insert(Display::Flex);
        s.flex_direction = Some(FlexDirection::Row);
        self
    }

    /// Arrange children top to bottom.
    fn col(mut self) -> Self {
        let s = self.style_mut();
        s.display.get_or_insert(Display::Flex);
        s.flex_direction = Some(FlexDirection::Column);
        self
    }

    fn flex_direction(mut self, value: FlexDirection) -> Self {
        self.style_mut().flex_direction = Some(value);
        self
    }

    fn wrap(mut self) -> Self {
        self.style_mut().flex_wrap = Some(FlexWrap::Wrap);
        self
    }

    fn no_wrap(mut self) -> Self {
        self.style_mut().flex_wrap = Some(FlexWrap::NoWrap);
        self
    }

    fn grow(mut self, value: f32) -> Self {
        self.style_mut().flex_grow = Some(value);
        self
    }

    fn shrink(mut self, value: f32) -> Self {
        self.style_mut().flex_shrink = Some(value);
        self
    }

    fn basis(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().flex_basis = Some(value.into());
        self
    }

    /// Take all remaining space along the main axis.
    fn flex_1(mut self) -> Self {
        let s = self.style_mut();
        s.flex_grow = Some(1.0);
        s.flex_shrink = Some(1.0);
        s.flex_basis = Some(Length::Px(Px::ZERO));
        self
    }

    fn justify(mut self, value: JustifyContent) -> Self {
        self.style_mut().justify_content = Some(value);
        self
    }

    fn justify_start(self) -> Self {
        self.justify(JustifyContent::Start)
    }

    fn justify_center(self) -> Self {
        self.justify(JustifyContent::Center)
    }

    fn justify_end(self) -> Self {
        self.justify(JustifyContent::End)
    }

    fn justify_between(self) -> Self {
        self.justify(JustifyContent::SpaceBetween)
    }

    fn justify_around(self) -> Self {
        self.justify(JustifyContent::SpaceAround)
    }

    fn justify_evenly(self) -> Self {
        self.justify(JustifyContent::SpaceEvenly)
    }

    fn items(mut self, value: AlignItems) -> Self {
        self.style_mut().align_items = Some(value);
        self
    }

    fn items_start(self) -> Self {
        self.items(AlignItems::Start)
    }

    fn items_center(self) -> Self {
        self.items(AlignItems::Center)
    }

    fn items_end(self) -> Self {
        self.items(AlignItems::End)
    }

    fn items_stretch(self) -> Self {
        self.items(AlignItems::Stretch)
    }

    fn items_baseline(self) -> Self {
        self.items(AlignItems::Baseline)
    }

    fn align_self(mut self, value: AlignItems) -> Self {
        self.style_mut().align_self = Some(value);
        self
    }

    fn align_content(mut self, value: JustifyContent) -> Self {
        self.style_mut().align_content = Some(value);
        self
    }

    /// Center children on both axes.
    fn center(self) -> Self {
        self.justify_center().items_center()
    }

    // -- gaps --

    fn gap(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.row_gap = Some(value.into());
        s.column_gap = Some(value.into());
        self
    }

    fn gap_x(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().column_gap = Some(value.into());
        self
    }

    fn gap_y(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().row_gap = Some(value.into());
        self
    }

    // -- size --

    fn w(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().width = Some(value.into());
        self
    }

    fn h(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().height = Some(value.into());
        self
    }

    fn size(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.width = Some(value.into());
        s.height = Some(value.into());
        self
    }

    fn w_full(self) -> Self {
        self.w(Length::Percent(1.0))
    }

    fn h_full(self) -> Self {
        self.h(Length::Percent(1.0))
    }

    fn size_full(self) -> Self {
        self.w_full().h_full()
    }

    fn w_auto(self) -> Self {
        self.w(Length::Auto)
    }

    fn h_auto(self) -> Self {
        self.h(Length::Auto)
    }

    fn min_w(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().min_width = Some(value.into());
        self
    }

    fn min_h(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().min_height = Some(value.into());
        self
    }

    fn max_w(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().max_width = Some(value.into());
        self
    }

    fn max_h(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().max_height = Some(value.into());
        self
    }

    fn aspect_ratio(mut self, value: f32) -> Self {
        self.style_mut().aspect_ratio = Some(value);
        self
    }

    // -- padding --

    fn p(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.padding_top = Some(value.into());
        s.padding_right = Some(value.into());
        s.padding_bottom = Some(value.into());
        s.padding_left = Some(value.into());
        self
    }

    fn px(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.padding_left = Some(value.into());
        s.padding_right = Some(value.into());
        self
    }

    fn py(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.padding_top = Some(value.into());
        s.padding_bottom = Some(value.into());
        self
    }

    fn pt(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().padding_top = Some(value.into());
        self
    }

    fn pr(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().padding_right = Some(value.into());
        self
    }

    fn pb(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().padding_bottom = Some(value.into());
        self
    }

    fn pl(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().padding_left = Some(value.into());
        self
    }

    // -- margin --

    fn m(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.margin_top = Some(value.into());
        s.margin_right = Some(value.into());
        s.margin_bottom = Some(value.into());
        s.margin_left = Some(value.into());
        self
    }

    fn mx(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.margin_left = Some(value.into());
        s.margin_right = Some(value.into());
        self
    }

    fn my(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.margin_top = Some(value.into());
        s.margin_bottom = Some(value.into());
        self
    }

    fn mt(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().margin_top = Some(value.into());
        self
    }

    fn mr(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().margin_right = Some(value.into());
        self
    }

    fn mb(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().margin_bottom = Some(value.into());
        self
    }

    fn ml(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().margin_left = Some(value.into());
        self
    }

    /// Center horizontally within the parent by taking equal automatic margins.
    fn mx_auto(mut self) -> Self {
        let s = self.style_mut();
        s.margin_left = Some(Length::Auto);
        s.margin_right = Some(Length::Auto);
        self
    }

    // -- position --

    fn absolute(mut self) -> Self {
        self.style_mut().position = Some(Position::Absolute);
        self
    }

    fn relative(mut self) -> Self {
        self.style_mut().position = Some(Position::Relative);
        self
    }

    fn top(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().top = Some(value.into());
        self
    }

    fn right(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().right = Some(value.into());
        self
    }

    fn bottom(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().bottom = Some(value.into());
        self
    }

    fn left(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().left = Some(value.into());
        self
    }

    /// Pin to all four edges of the positioned ancestor.
    fn inset(mut self, value: impl Into<Length> + Copy) -> Self {
        let s = self.style_mut();
        s.top = Some(value.into());
        s.right = Some(value.into());
        s.bottom = Some(value.into());
        s.left = Some(value.into());
        self
    }

    // -- overflow --

    fn overflow(mut self, value: Overflow) -> Self {
        let s = self.style_mut();
        s.overflow_x = Some(value);
        s.overflow_y = Some(value);
        self
    }

    fn overflow_x(mut self, value: Overflow) -> Self {
        self.style_mut().overflow_x = Some(value);
        self
    }

    fn overflow_y(mut self, value: Overflow) -> Self {
        self.style_mut().overflow_y = Some(value);
        self
    }

    fn overflow_hidden(self) -> Self {
        self.overflow(Overflow::Hidden)
    }

    fn overflow_scroll(self) -> Self {
        self.overflow(Overflow::Scroll)
    }

    // -- background --

    fn bg(mut self, value: impl Into<Paint>) -> Self {
        self.style_mut().background = Some(value.into());
        self
    }

    fn bg_none(mut self) -> Self {
        self.style_mut().background = Some(Paint::None);
        self
    }

    // -- border --

    fn border(mut self, width: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.border_top_width = Some(width.into());
        s.border_right_width = Some(width.into());
        s.border_bottom_width = Some(width.into());
        s.border_left_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_x(mut self, width: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.border_left_width = Some(width.into());
        s.border_right_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_y(mut self, width: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.border_top_width = Some(width.into());
        s.border_bottom_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_t(mut self, width: impl Into<Px>) -> Self {
        let s = self.style_mut();
        s.border_top_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_r(mut self, width: impl Into<Px>) -> Self {
        let s = self.style_mut();
        s.border_right_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_b(mut self, width: impl Into<Px>) -> Self {
        let s = self.style_mut();
        s.border_bottom_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_l(mut self, width: impl Into<Px>) -> Self {
        let s = self.style_mut();
        s.border_left_width = Some(width.into());
        s.border_style.get_or_insert(BorderStyle::Solid);
        self
    }

    fn border_color(mut self, color: Rgba) -> Self {
        self.style_mut().border_color = Some(color);
        self
    }

    fn border_style(mut self, value: BorderStyle) -> Self {
        self.style_mut().border_style = Some(value);
        self
    }

    // -- corner radius --

    fn rounded(mut self, radius: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.radius_top_left = Some(radius.into());
        s.radius_top_right = Some(radius.into());
        s.radius_bottom_right = Some(radius.into());
        s.radius_bottom_left = Some(radius.into());
        self
    }

    fn rounded_t(mut self, radius: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.radius_top_left = Some(radius.into());
        s.radius_top_right = Some(radius.into());
        self
    }

    fn rounded_b(mut self, radius: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.radius_bottom_left = Some(radius.into());
        s.radius_bottom_right = Some(radius.into());
        self
    }

    fn rounded_l(mut self, radius: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.radius_top_left = Some(radius.into());
        s.radius_bottom_left = Some(radius.into());
        self
    }

    fn rounded_r(mut self, radius: impl Into<Px> + Copy) -> Self {
        let s = self.style_mut();
        s.radius_top_right = Some(radius.into());
        s.radius_bottom_right = Some(radius.into());
        self
    }

    fn rounded_tl(mut self, radius: impl Into<Px>) -> Self {
        self.style_mut().radius_top_left = Some(radius.into());
        self
    }

    fn rounded_tr(mut self, radius: impl Into<Px>) -> Self {
        self.style_mut().radius_top_right = Some(radius.into());
        self
    }

    fn rounded_br(mut self, radius: impl Into<Px>) -> Self {
        self.style_mut().radius_bottom_right = Some(radius.into());
        self
    }

    fn rounded_bl(mut self, radius: impl Into<Px>) -> Self {
        self.style_mut().radius_bottom_left = Some(radius.into());
        self
    }

    /// A pill shape. The radius clamps to half the shorter dimension at paint
    /// time, so this works at any size.
    fn rounded_full(self) -> Self {
        self.rounded(Px(9999.0))
    }

    // -- shadow --

    fn shadow(mut self, shadow: BoxShadow) -> Self {
        self.style_mut()
            .box_shadows
            .get_or_insert_with(SmallVec::new)
            .push(shadow);
        self
    }

    fn shadows(mut self, shadows: impl IntoIterator<Item = BoxShadow>) -> Self {
        self.style_mut().box_shadows = Some(shadows.into_iter().collect());
        self
    }

    fn shadow_none(mut self) -> Self {
        self.style_mut().box_shadows = Some(SmallVec::new());
        self
    }

    // -- misc paint --

    /// Move this element and everything in it.
    ///
    /// Painting only. Layout has already happened by the time a transform
    /// applies, which is what makes it cheap to animate: nothing is measured
    /// again, and nothing around it moves.
    fn translate(mut self, x: impl Into<Px>, y: impl Into<Px>) -> Self {
        let current = self.style_mut().transform.get_or_insert(Transform2D::IDENTITY);
        current.translate_x = x.into();
        current.translate_y = y.into();
        self
    }

    /// Scale about the transform origin, the center by default.
    fn scale(self, factor: f32) -> Self {
        self.scale_xy(factor, factor)
    }

    fn scale_xy(mut self, x: f32, y: f32) -> Self {
        let current = self.style_mut().transform.get_or_insert(Transform2D::IDENTITY);
        current.scale_x = x;
        current.scale_y = y;
        self
    }

    /// Rotate clockwise about the transform origin, in degrees.
    fn rotate(mut self, degrees: f32) -> Self {
        let current = self.style_mut().transform.get_or_insert(Transform2D::IDENTITY);
        current.rotate = degrees;
        self
    }

    /// Move the point a rotation or a scale turns about, as a fraction of the
    /// element's own box. `(0.5, 0.5)`, the center, by default.
    fn transform_origin(mut self, x: f32, y: f32) -> Self {
        let current = self.style_mut().transform.get_or_insert(Transform2D::IDENTITY);
        current.origin_x = x;
        current.origin_y = y;
        self
    }

    /// Set the whole transform at once.
    fn transform(mut self, transform: Transform2D) -> Self {
        self.style_mut().transform = Some(transform);
        self
    }

    fn opacity(mut self, value: f32) -> Self {
        self.style_mut().opacity = Some(value);
        self
    }

    fn cursor(mut self, value: CursorStyle) -> Self {
        self.style_mut().cursor = Some(value);
        self
    }

    fn cursor_pointer(self) -> Self {
        self.cursor(CursorStyle::Pointer)
    }

    fn cursor_text(self) -> Self {
        self.cursor(CursorStyle::Text)
    }

    /// Let presses pass straight through this element and its children.
    ///
    /// An overlay that only wants a few small regions to be interactive would
    /// otherwise swallow every click across its whole area.
    fn pointer_events_none(mut self) -> Self {
        self.style_mut().pointer_events = Some(false);
        self
    }

    /// Take part in hit testing again, inside a subtree that opted out.
    fn pointer_events_auto(mut self) -> Self {
        self.style_mut().pointer_events = Some(true);
        self
    }

    // -- text --

    fn text_color(mut self, color: Rgba) -> Self {
        self.style_mut().color = Some(color);
        self
    }

    fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.style_mut().font_family = Some(FontFamily::single(family));
        self
    }

    fn font_families(mut self, families: impl IntoIterator<Item = SharedString>) -> Self {
        self.style_mut().font_family = Some(FontFamily::new(families));
        self
    }

    fn text_size(mut self, size: impl Into<Length>) -> Self {
        self.style_mut().font_size = Some(size.into());
        self
    }

    fn font_weight(mut self, weight: FontWeight) -> Self {
        self.style_mut().font_weight = Some(weight);
        self
    }

    fn bold(self) -> Self {
        self.font_weight(FontWeight::BOLD)
    }

    fn semibold(self) -> Self {
        self.font_weight(FontWeight::SEMIBOLD)
    }

    fn medium(self) -> Self {
        self.font_weight(FontWeight::MEDIUM)
    }

    fn italic(mut self) -> Self {
        self.style_mut().font_style = Some(FontStyle::Italic);
        self
    }

    fn line_height(mut self, value: LineHeight) -> Self {
        self.style_mut().line_height = Some(value);
        self
    }

    fn letter_spacing(mut self, value: impl Into<Length>) -> Self {
        self.style_mut().letter_spacing = Some(value.into());
        self
    }

    fn text_align(mut self, value: TextAlign) -> Self {
        self.style_mut().text_align = Some(value);
        self
    }

    fn text_left(self) -> Self {
        self.text_align(TextAlign::Start)
    }

    fn text_center(self) -> Self {
        self.text_align(TextAlign::Center)
    }

    fn text_right(self) -> Self {
        self.text_align(TextAlign::End)
    }

    fn white_space(mut self, value: WhiteSpace) -> Self {
        self.style_mut().white_space = Some(value);
        self
    }

    fn whitespace_nowrap(self) -> Self {
        self.white_space(WhiteSpace::NoWrap)
    }

    fn underline(mut self) -> Self {
        let d = self
            .style_mut()
            .text_decoration
            .get_or_insert(TextDecoration::NONE);
        d.underline = true;
        self
    }

    fn strikethrough(mut self) -> Self {
        let d = self
            .style_mut()
            .text_decoration
            .get_or_insert(TextDecoration::NONE);
        d.strikethrough = true;
        self
    }

    /// Truncate overflowing text with an ellipsis. Implies no wrapping.
    fn text_ellipsis(mut self) -> Self {
        let s = self.style_mut();
        s.text_overflow = Some(TextOverflow::Ellipsis);
        s.white_space.get_or_insert(WhiteSpace::NoWrap);
        self
    }

    // -- transitions --

    fn transition(mut self, transition: Transition) -> Self {
        self.style_mut()
            .transitions
            .get_or_insert_with(SmallVec::new)
            .push(transition);
        self
    }

    /// Transition every animatable property over `duration_ms`.
    fn transition_all(self, duration_ms: f32) -> Self {
        self.transition(Transition::new(TransitionTarget::All, duration_ms))
    }

    fn transition_property(self, property: AnimatableProperty, duration_ms: f32) -> Self {
        self.transition(Transition::new(
            TransitionTarget::One(property),
            duration_ms,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_only_overwrites_fields_that_are_set() {
        let mut base = Style::new().bg(Rgba::BLACK).w(100.0).text_color(Rgba::WHITE);
        let overlay = Style::new().bg(Rgba::WHITE);
        base.merge_from(&overlay);

        assert_eq!(base.background, Some(Paint::Solid(Rgba::WHITE)));
        assert_eq!(base.width, Some(Length::Px(Px(100.0))));
        assert_eq!(base.color, Some(Rgba::WHITE));
    }

    #[test]
    fn inherited_properties_come_from_the_parent() {
        let parent = Style::new()
            .text_color(Rgba::hex(0xff0000))
            .text_size(20.0)
            .bold()
            .resolve(&ComputedStyle::default(), Px(16.0));

        let child = Style::new().bg(Rgba::BLACK).resolve(&parent, Px(16.0));

        assert_eq!(child.color, Rgba::hex(0xff0000));
        assert_eq!(child.font_size, Px(20.0));
        assert_eq!(child.font_weight, FontWeight::BOLD);
        // Non inherited properties reset.
        assert_eq!(child.background, Paint::Solid(Rgba::BLACK));
        assert_eq!(parent.background, Paint::None);
    }

    #[test]
    fn rem_lengths_resolve_against_the_root_font_size() {
        use guirs_core::rems;
        let computed = Style::new()
            .p(rems(1.0))
            .text_size(rems(1.5))
            .resolve(&ComputedStyle::default(), Px(16.0));

        assert_eq!(computed.padding.top, Length::Px(Px(16.0)));
        assert_eq!(computed.font_size, Px(24.0));
    }

    #[test]
    fn percentage_font_size_is_relative_to_the_parent() {
        let parent = Style::new()
            .text_size(20.0)
            .resolve(&ComputedStyle::default(), Px(16.0));
        let child = Style::new()
            .text_size(Length::Percent(0.5))
            .resolve(&parent, Px(16.0));
        assert_eq!(child.font_size, Px(10.0));
    }

    #[test]
    fn shorthand_helpers_set_every_side() {
        let s = Style::new().p(8.0).rounded(4.0).border(1.0);
        assert_eq!(s.padding_left, Some(Length::Px(Px(8.0))));
        assert_eq!(s.padding_right, Some(Length::Px(Px(8.0))));
        assert_eq!(s.radius_bottom_right, Some(Px(4.0)));
        assert_eq!(s.border_top_width, Some(Px(1.0)));
        assert_eq!(s.border_style, Some(BorderStyle::Solid));
    }

    #[test]
    fn later_transitions_win_for_the_same_property() {
        let computed = Style::new()
            .transition_all(100.0)
            .transition_property(AnimatableProperty::Background, 400.0)
            .resolve(&ComputedStyle::default(), Px(16.0));

        let bg = computed
            .transition_for(AnimatableProperty::Background)
            .unwrap();
        assert_eq!(bg.duration_ms, OrderedF32(400.0));

        let color = computed.transition_for(AnimatableProperty::Color).unwrap();
        assert_eq!(color.duration_ms, OrderedF32(100.0));
    }

    #[test]
    fn direction_helpers_imply_flex_display() {
        let s = Style::new().col();
        assert_eq!(s.display, Some(Display::Flex));
        assert_eq!(s.flex_direction, Some(FlexDirection::Column));
    }

    #[test]
    fn an_empty_style_paints_nothing() {
        assert!(!ComputedStyle::default().paints());
        let filled = Style::new()
            .bg(Rgba::BLACK)
            .resolve(&ComputedStyle::default(), Px(16.0));
        assert!(filled.paints());
    }
}
