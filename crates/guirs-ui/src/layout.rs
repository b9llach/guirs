//! Layout.
//!
//! Boxes are laid out by taffy, which implements flexbox and block layout the
//! way browsers do. This module's job is translating a [`ComputedStyle`] into
//! taffy's own style type and handing back results in guirs coordinates.
//!
//! Text is the one thing taffy cannot size on its own, so text nodes carry a
//! measure context and the text system is invoked during layout to answer
//! "how tall is this string if it may be this wide".

use guirs_core::{Bounds, Length, Point, Px, Size};
use guirs_style::{GridLine, GridPlacement, TrackSize, 
    AlignItems, ComputedStyle, Display, FlexDirection, FlexWrap, JustifyContent, Overflow, Position,
};
use guirs_text::{LayoutOptions, RichText, TextStyle, TextSystem};
use taffy::prelude::*;
use taffy::TaffyTree;

/// Identifies a node in the layout tree.
pub type LayoutId = taffy::NodeId;

/// What a text node needs in order to measure itself.
#[derive(Clone, Debug)]
pub struct MeasureContext {
    /// The content, which carries its styled spans if it has any. Plain text
    /// is the same type with an empty span table and takes the same path.
    pub content: RichText,
    pub style: TextStyle,
    pub options: LayoutOptions,
}

/// The layout tree for one frame.
pub struct LayoutEngine {
    tree: TaffyTree<MeasureContext>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        LayoutEngine::new()
    }
}

impl std::fmt::Debug for LayoutEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutEngine")
            .field("nodes", &self.tree.total_node_count())
            .finish()
    }
}

impl LayoutEngine {
    pub fn new() -> Self {
        let mut tree = TaffyTree::with_capacity(256);
        // Rounding snaps computed edges to whole pixels, which stops adjacent
        // boxes leaving a hairline gap between them.
        tree.enable_rounding();
        LayoutEngine { tree }
    }

    /// Discard the previous frame's tree.
    pub fn clear(&mut self) {
        self.tree.clear();
    }

    pub fn node_count(&self) -> usize {
        self.tree.total_node_count()
    }

    /// Add a box node with the given children.
    pub fn add_node(&mut self, style: &ComputedStyle, children: &[LayoutId]) -> LayoutId {
        let taffy_style = to_taffy_style(style);
        if children.is_empty() {
            self.tree
                .new_leaf(taffy_style)
                .expect("layout tree ran out of capacity")
        } else {
            self.tree
                .new_with_children(taffy_style, children)
                .expect("layout tree ran out of capacity")
        }
    }

    /// Add a node whose size comes from measuring text.
    pub fn add_text_node(&mut self, style: &ComputedStyle, context: MeasureContext) -> LayoutId {
        self.tree
            .new_leaf_with_context(to_taffy_style(style), context)
            .expect("layout tree ran out of capacity")
    }

    /// Add a leaf of a fixed intrinsic size, for images and custom painting.
    pub fn add_fixed_node(&mut self, style: &ComputedStyle, intrinsic: Size<Px>) -> LayoutId {
        let mut taffy_style = to_taffy_style(style);
        if matches!(style.size.width, Length::Auto) {
            taffy_style.size.width = Dimension::length(intrinsic.width.0);
        }
        if matches!(style.size.height, Length::Auto) {
            taffy_style.size.height = Dimension::length(intrinsic.height.0);
        }
        self.tree
            .new_leaf(taffy_style)
            .expect("layout tree ran out of capacity")
    }

    /// Run layout for a tree rooted at `root`.
    pub fn compute(&mut self, root: LayoutId, available: Size<Px>, text: &mut TextSystem) {
        let space = taffy::Size {
            width: AvailableSpace::Definite(available.width.0),
            height: AvailableSpace::Definite(available.height.0),
        };

        let _ = self.tree.compute_layout_with_measure(
            root,
            space,
            |known, available_space, _node, context, _style| {
                let Some(context) = context else {
                    return taffy::Size::ZERO;
                };
                // Both dimensions already decided: nothing to measure.
                if let (Some(width), Some(height)) = (known.width, known.height) {
                    return taffy::Size { width, height };
                }

                let max_width = match known.width {
                    Some(width) => Some(Px(width)),
                    None => match available_space.width {
                        AvailableSpace::Definite(width) => Some(Px(width)),
                        // Min content means break at every opportunity, which a
                        // zero width constraint expresses exactly.
                        AvailableSpace::MinContent => Some(Px(0.0)),
                        AvailableSpace::MaxContent => None,
                    },
                };

                let options = LayoutOptions {
                    max_width,
                    ..context.options.clone()
                };
                let laid_out = text.layout_rich(&context.content, &context.style, &options);
                // Round up. Taffy rounds computed edges to whole pixels, so a
                // box sized to a fractional intrinsic width can come back a
                // fraction narrower than the text that asked for it, and paint
                // would then wrap a line that layout thought fitted.
                taffy::Size {
                    width: known.width.unwrap_or(laid_out.size.width.0.ceil()),
                    height: known.height.unwrap_or(laid_out.size.height.0.ceil()),
                }
            },
        );
    }

    /// Position and size of a node, relative to its parent's content box.
    pub fn bounds(&self, node: LayoutId) -> Bounds<Px> {
        match self.tree.layout(node) {
            Ok(layout) => Bounds::from_xywh(
                Px(layout.location.x),
                Px(layout.location.y),
                Px(layout.size.width),
                Px(layout.size.height),
            ),
            Err(_) => Bounds::zero(),
        }
    }

    /// The size of a node's contents, which can exceed the node itself and is
    /// what a scroll view needs in order to size its range.
    pub fn content_size(&self, node: LayoutId) -> Size<Px> {
        match self.tree.layout(node) {
            Ok(layout) => Size::new(Px(layout.content_size.width), Px(layout.content_size.height)),
            Err(_) => Size::zero(),
        }
    }

    /// Resolved padding, in case a caller needs the content box.
    pub fn padding(&self, node: LayoutId) -> guirs_core::Edges<Px> {
        match self.tree.layout(node) {
            Ok(layout) => guirs_core::Edges {
                top: Px(layout.padding.top),
                right: Px(layout.padding.right),
                bottom: Px(layout.padding.bottom),
                left: Px(layout.padding.left),
            },
            Err(_) => guirs_core::Edges::zero(),
        }
    }

    /// Resolved border widths.
    pub fn border(&self, node: LayoutId) -> guirs_core::Edges<Px> {
        match self.tree.layout(node) {
            Ok(layout) => guirs_core::Edges {
                top: Px(layout.border.top),
                right: Px(layout.border.right),
                bottom: Px(layout.border.bottom),
                left: Px(layout.border.left),
            },
            Err(_) => guirs_core::Edges::zero(),
        }
    }

    /// Absolute bounds, given the parent's absolute origin.
    #[inline]
    pub fn absolute_bounds(&self, node: LayoutId, parent_origin: Point<Px>) -> Bounds<Px> {
        self.bounds(node).translate(parent_origin)
    }
}

// ---------------------------------------------------------------------------
// Style translation
// ---------------------------------------------------------------------------

fn to_dimension(length: Length) -> Dimension {
    match length {
        Length::Auto => Dimension::auto(),
        Length::Px(v) => Dimension::length(v.0),
        Length::Percent(f) => Dimension::percent(f),
        // Rems are resolved during style computation, and a flex fraction is
        // not a box dimension.
        Length::Rems(_) | Length::Fraction(_) => Dimension::auto(),
    }
}

fn to_length_percentage(length: Length) -> LengthPercentage {
    match length {
        Length::Px(v) => LengthPercentage::length(v.0),
        Length::Percent(f) => LengthPercentage::percent(f),
        _ => LengthPercentage::length(0.0),
    }
}

fn to_length_percentage_auto(length: Length) -> LengthPercentageAuto {
    match length {
        Length::Auto => LengthPercentageAuto::auto(),
        Length::Px(v) => LengthPercentageAuto::length(v.0),
        Length::Percent(f) => LengthPercentageAuto::percent(f),
        _ => LengthPercentageAuto::auto(),
    }
}

fn to_align_items(value: AlignItems) -> taffy::AlignItems {
    match value {
        AlignItems::Start => taffy::AlignItems::FLEX_START,
        AlignItems::Center => taffy::AlignItems::CENTER,
        AlignItems::End => taffy::AlignItems::FLEX_END,
        AlignItems::Stretch => taffy::AlignItems::STRETCH,
        AlignItems::Baseline => taffy::AlignItems::BASELINE,
    }
}

fn to_justify_content(value: JustifyContent) -> taffy::JustifyContent {
    match value {
        JustifyContent::Start => taffy::JustifyContent::FLEX_START,
        JustifyContent::Center => taffy::JustifyContent::CENTER,
        JustifyContent::End => taffy::JustifyContent::FLEX_END,
        JustifyContent::SpaceBetween => taffy::JustifyContent::SPACE_BETWEEN,
        JustifyContent::SpaceAround => taffy::JustifyContent::SPACE_AROUND,
        JustifyContent::SpaceEvenly => taffy::JustifyContent::SPACE_EVENLY,
        JustifyContent::Stretch => taffy::JustifyContent::STRETCH,
    }
}

fn to_overflow(value: Overflow) -> taffy::Overflow {
    match value {
        Overflow::Visible => taffy::Overflow::Visible,
        Overflow::Hidden => taffy::Overflow::Hidden,
        // Both reserve space for a scrollbar only when one is shown, which the
        // scroll view decides for itself.
        Overflow::Scroll | Overflow::Auto => taffy::Overflow::Scroll,
    }
}

/// One row or column, in the form taffy takes.
fn to_taffy_track(track: TrackSize) -> taffy::GridTemplateComponent<String> {
    use taffy::style_helpers::*;
    taffy::GridTemplateComponent::Single(match track {
        TrackSize::Auto => auto(),
        TrackSize::Px(value) => length(value.0),
        TrackSize::Percent(fraction) => percent(fraction),
        TrackSize::Fr(share) => fr(share),
        TrackSize::MinContent => min_content(),
        TrackSize::MaxContent => max_content(),
    })
}

fn to_taffy_placement(placement: GridPlacement) -> taffy::GridPlacement {
    match placement {
        GridPlacement::Auto => taffy::GridPlacement::Auto,
        // Zero is not a line in CSS, and taffy would read it as one, so it is
        // treated as "wherever" rather than silently meaning line one.
        GridPlacement::Line(0) => taffy::GridPlacement::Auto,
        GridPlacement::Line(index) => taffy::GridPlacement::from_line_index(index),
        GridPlacement::Span(tracks) => taffy::GridPlacement::from_span(tracks),
    }
}

fn to_taffy_line(line: GridLine) -> taffy::Line<taffy::GridPlacement> {
    taffy::Line {
        start: to_taffy_placement(line.start),
        end: to_taffy_placement(line.end),
    }
}

/// Translate a computed style into taffy's own.
pub fn to_taffy_style(style: &ComputedStyle) -> taffy::Style {
    taffy::Style {
        display: match style.display {
            Display::Flex => taffy::Display::Flex,
            Display::Block => taffy::Display::Block,
            Display::Grid => taffy::Display::Grid,
            Display::None => taffy::Display::None,
        },
        position: match style.position {
            Position::Relative => taffy::Position::Relative,
            Position::Absolute => taffy::Position::Absolute,
        },
        overflow: taffy::Point {
            x: to_overflow(style.overflow_x),
            y: to_overflow(style.overflow_y),
        },
        // guirs draws its own scrollbars as an overlay, so layout must not
        // reserve a gutter for them.
        scrollbar_width: 0.0,
        inset: taffy::Rect {
            left: to_length_percentage_auto(style.inset.left),
            right: to_length_percentage_auto(style.inset.right),
            top: to_length_percentage_auto(style.inset.top),
            bottom: to_length_percentage_auto(style.inset.bottom),
        },
        size: taffy::Size {
            width: to_dimension(style.size.width),
            height: to_dimension(style.size.height),
        },
        min_size: taffy::Size {
            width: to_dimension(style.min_size.width),
            height: to_dimension(style.min_size.height),
        },
        max_size: taffy::Size {
            width: to_dimension(style.max_size.width),
            height: to_dimension(style.max_size.height),
        },
        aspect_ratio: style.aspect_ratio,
        margin: taffy::Rect {
            left: to_length_percentage_auto(style.margin.left),
            right: to_length_percentage_auto(style.margin.right),
            top: to_length_percentage_auto(style.margin.top),
            bottom: to_length_percentage_auto(style.margin.bottom),
        },
        padding: taffy::Rect {
            left: to_length_percentage(style.padding.left),
            right: to_length_percentage(style.padding.right),
            top: to_length_percentage(style.padding.top),
            bottom: to_length_percentage(style.padding.bottom),
        },
        border: taffy::Rect {
            left: LengthPercentage::length(style.border_widths.left.0),
            right: LengthPercentage::length(style.border_widths.right.0),
            top: LengthPercentage::length(style.border_widths.top.0),
            bottom: LengthPercentage::length(style.border_widths.bottom.0),
        },
        align_items: Some(to_align_items(style.align_items)),
        align_self: style.align_self.map(to_align_items),
        align_content: Some(to_justify_content(style.align_content)),
        justify_content: Some(to_justify_content(style.justify_content)),
        gap: taffy::Size {
            width: to_length_percentage(style.gap.width),
            height: to_length_percentage(style.gap.height),
        },
        flex_direction: match style.flex_direction {
            FlexDirection::Row => taffy::FlexDirection::Row,
            FlexDirection::Column => taffy::FlexDirection::Column,
            FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
            FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
        },
        flex_wrap: match style.flex_wrap {
            FlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
            FlexWrap::Wrap => taffy::FlexWrap::Wrap,
            FlexWrap::WrapReverse => taffy::FlexWrap::WrapReverse,
        },
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        grid_template_columns: style
            .grid_template_columns
            .as_ref()
            .map(|tracks| tracks.iter().copied().map(to_taffy_track).collect())
            .unwrap_or_default(),
        grid_template_rows: style
            .grid_template_rows
            .as_ref()
            .map(|tracks| tracks.iter().copied().map(to_taffy_track).collect())
            .unwrap_or_default(),
        grid_column: to_taffy_line(style.grid_column),
        grid_row: to_taffy_line(style.grid_row),
        flex_basis: to_dimension(style.flex_basis),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs_style::{Style, Styled};

    fn computed(style: Style) -> ComputedStyle {
        style.resolve(&ComputedStyle::default(), Px(16.0))
    }

    #[test]
    fn a_row_of_two_children_splits_the_width() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child_style = computed(Style::new().flex_1());
        let a = engine.add_node(&child_style, &[]);
        let b = engine.add_node(&child_style, &[]);
        let root = engine.add_node(&computed(Style::new().row().size_full()), &[a, b]);

        engine.compute(root, Size::new(Px(200.0), Px(100.0)), &mut text);

        assert_eq!(engine.bounds(a).size.width, Px(100.0));
        assert_eq!(engine.bounds(b).size.width, Px(100.0));
        assert_eq!(engine.bounds(b).origin.x, Px(100.0));
    }

    #[test]
    fn padding_shrinks_the_content_box() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(&computed(Style::new().flex_1()), &[]);
        let root = engine.add_node(
            &computed(Style::new().row().size_full().p(10.0)),
            &[child],
        );
        engine.compute(root, Size::new(Px(100.0), Px(100.0)), &mut text);

        let bounds = engine.bounds(child);
        assert_eq!(bounds.origin, Point::new(Px(10.0), Px(10.0)));
        assert_eq!(bounds.size.width, Px(80.0));
    }

    #[test]
    fn borders_are_accounted_for_like_padding() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(&computed(Style::new().flex_1()), &[]);
        let root = engine.add_node(
            &computed(Style::new().row().size_full().border(4.0)),
            &[child],
        );
        engine.compute(root, Size::new(Px(100.0), Px(100.0)), &mut text);

        assert_eq!(engine.bounds(child).origin.x, Px(4.0));
        assert_eq!(engine.bounds(child).size.width, Px(92.0));
        assert_eq!(engine.border(root).left, Px(4.0));
    }

    #[test]
    fn gaps_separate_children() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let a = engine.add_node(&computed(Style::new().w(20.0)), &[]);
        let b = engine.add_node(&computed(Style::new().w(20.0)), &[]);
        let root = engine.add_node(&computed(Style::new().row().gap(8.0)), &[a, b]);
        engine.compute(root, Size::new(Px(200.0), Px(50.0)), &mut text);

        assert_eq!(engine.bounds(b).origin.x, Px(28.0));
    }

    #[test]
    fn a_column_stacks_children_vertically() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let a = engine.add_node(&computed(Style::new().h(30.0)), &[]);
        let b = engine.add_node(&computed(Style::new().h(30.0)), &[]);
        let root = engine.add_node(&computed(Style::new().col().size_full()), &[a, b]);
        engine.compute(root, Size::new(Px(100.0), Px(200.0)), &mut text);

        assert_eq!(engine.bounds(a).origin.y, Px(0.0));
        assert_eq!(engine.bounds(b).origin.y, Px(30.0));
    }

    #[test]
    fn centering_positions_a_fixed_child_in_the_middle() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(&computed(Style::new().size(20.0)), &[]);
        let root = engine.add_node(
            &computed(Style::new().row().size_full().center()),
            &[child],
        );
        engine.compute(root, Size::new(Px(100.0), Px(100.0)), &mut text);

        assert_eq!(engine.bounds(child).origin, Point::new(Px(40.0), Px(40.0)));
    }

    #[test]
    fn absolute_children_are_placed_against_their_ancestor() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(
            &computed(Style::new().absolute().top(5.0).left(7.0).size(10.0)),
            &[],
        );
        let root = engine.add_node(&computed(Style::new().size_full()), &[child]);
        engine.compute(root, Size::new(Px(100.0), Px(100.0)), &mut text);

        assert_eq!(engine.bounds(child).origin, Point::new(Px(7.0), Px(5.0)));
    }

    #[test]
    fn display_none_collapses_a_child() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let hidden = engine.add_node(&computed(Style::new().hidden().size(50.0)), &[]);
        let shown = engine.add_node(&computed(Style::new().size(20.0)), &[]);
        let root = engine.add_node(&computed(Style::new().row()), &[hidden, shown]);
        engine.compute(root, Size::new(Px(200.0), Px(50.0)), &mut text);

        assert_eq!(engine.bounds(hidden).size, Size::new(Px(0.0), Px(0.0)));
        assert_eq!(engine.bounds(shown).origin.x, Px(0.0));
    }

    #[test]
    fn min_and_max_constrain_a_flexible_child() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(&computed(Style::new().flex_1().max_w(40.0)), &[]);
        let root = engine.add_node(&computed(Style::new().row().size_full()), &[child]);
        engine.compute(root, Size::new(Px(200.0), Px(50.0)), &mut text);

        assert_eq!(engine.bounds(child).size.width, Px(40.0));
    }

    #[test]
    fn absolute_bounds_accumulate_the_parent_origin() {
        let mut engine = LayoutEngine::new();
        let mut text = TextSystem::new();

        let child = engine.add_node(&computed(Style::new().size(10.0)), &[]);
        let root = engine.add_node(&computed(Style::new().p(5.0)), &[child]);
        engine.compute(root, Size::new(Px(100.0), Px(100.0)), &mut text);

        let absolute = engine.absolute_bounds(child, Point::new(Px(100.0), Px(200.0)));
        assert_eq!(absolute.origin, Point::new(Px(105.0), Px(205.0)));
    }

    #[test]
    fn clearing_resets_the_tree() {
        let mut engine = LayoutEngine::new();
        engine.add_node(&ComputedStyle::default(), &[]);
        assert!(engine.node_count() > 0);
        engine.clear();
        assert_eq!(engine.node_count(), 0);
    }
}
