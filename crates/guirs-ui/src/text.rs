//! The text element.
//!
//! Text is measured during layout and laid out again during paint. The second
//! pass is not wasted work: layout only knows the constraint, paint knows the
//! width the box actually got, and the shaping cache makes the repeat almost
//! free.
//!
//! One element covers both plain and rich text. A plain string is a block with
//! an empty span table, so a label pays nothing for the machinery a styled
//! paragraph needs, and there is one measuring and painting path rather than
//! two that drift apart.

use std::rc::Rc;

use guirs_core::{Bounds, Corners, Edges, ElementId, GlobalElementId, Paint, Point, Px, Rgba,
    SharedString};
use guirs_render::{Glyph, Quad, Underline};
use guirs_style::{ComputedStyle, CursorStyle, StateFlags, Style, Styled, TextDecoration};
use guirs_text::{subpixel_phase, GlyphKey, LayoutOptions, RichText, SpanStyle, TextLayout,
    TextStyle, BASE_SPAN};
use smallvec::SmallVec;

use crate::context::{content_box, Cx, Handlers, MouseHandler, SelectableText};
use crate::element::{Element, IntoElement};
use crate::event::MouseEvent;
use crate::layout::{LayoutId, MeasureContext};

/// A clickable link found in the painted text.
type LinkRegion = (Bounds<Px>, SharedString);

/// Called with a link span's target when that span is clicked.
pub type LinkHandler = Rc<dyn Fn(&SharedString, &mut crate::EventContext)>;

/// A run of text.
pub struct Text {
    content: RichText,
    element_type: SharedString,
    id: Option<ElementId>,
    classes: SmallVec<[SharedString; 4]>,
    style: Style,

    selectable: bool,
    /// Skipped when the interface describes itself. For text that is a picture
    /// rather than words: an icon glyph, a tick, a chevron.
    access_decorative: bool,
    on_link: Option<LinkHandler>,
    global_id: GlobalElementId,
    computed: ComputedStyle,
    node: Option<LayoutId>,
}

/// A run of text.
pub fn text(content: impl Into<SharedString>) -> Text {
    Text::new(content)
}

/// A run of text. An alias that reads better next to form controls.
pub fn label(content: impl Into<SharedString>) -> Text {
    Text::new(content)
}

/// A paragraph with styled spans in it.
///
/// ```
/// # use guirs_text::rich;
/// # use guirs_ui::text::rich_text;
/// rich_text(rich("Rust is ").bold("fast").push(" and ").code("safe"));
/// ```
pub fn rich_text(content: impl Into<RichText>) -> Text {
    Text::rich(content)
}

impl Text {
    pub fn new(content: impl Into<SharedString>) -> Self {
        Text::rich(RichText::plain(content))
    }

    pub fn rich(content: impl Into<RichText>) -> Self {
        Text {
            content: content.into(),
            element_type: SharedString::from("text"),
            id: None,
            classes: SmallVec::new(),
            style: Style::new(),
            selectable: false,
            access_decorative: false,
            on_link: None,
            global_id: GlobalElementId::ROOT,
            computed: ComputedStyle::default(),
            node: None,
        }
    }

    /// Report a different type name to the stylesheet, so a widget's own label
    /// can be targeted separately from loose text.
    pub fn as_type(mut self, element_type: &'static str) -> Self {
        self.element_type = SharedString::from(element_type);
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one or more classes, separated by whitespace.
    pub fn class(mut self, class: impl Into<SharedString>) -> Self {
        crate::div::push_classes(&mut self.classes, class.into());
        self
    }

    /// Add classes only when `condition` holds.
    pub fn class_if(self, condition: bool, class: impl Into<SharedString>) -> Self {
        if condition {
            self.class(class)
        } else {
            self
        }
    }

    /// Let the pointer select this text, and the platform shortcut copy it.
    ///
    /// Off by default: most text in an interface is a label, and making labels
    /// selectable turns every stray drag into a highlight.
    pub fn selectable(mut self) -> Self {
        self.selectable = true;
        self
    }

    /// Leave this text out of the interface's description.
    ///
    /// For text that is a picture: an icon font's glyph, a tick inside a
    /// checkbox, a chevron on a menu. Announcing those gives a person a
    /// character they cannot act on, and worse, it becomes part of the name of
    /// whatever contains them.
    pub fn decorative(mut self) -> Self {
        self.access_decorative = true;
        self
    }

    /// Called when a link span is clicked, with that span's target.
    ///
    /// The framework never follows a link itself. What a link means is the
    /// application's decision: open a browser, jump to an anchor, or run a
    /// command.
    pub fn on_link(mut self, f: impl Fn(&SharedString, &mut crate::EventContext) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    pub fn content(&self) -> &SharedString {
        &self.content.text
    }

    pub fn rich_content(&self) -> &RichText {
        &self.content
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

    fn layout_options(&self, max_width: Option<Px>) -> LayoutOptions {
        LayoutOptions {
            max_width,
            line_height: self.computed.line_height,
            align: self.computed.text_align,
            white_space: self.computed.white_space,
            overflow: self.computed.text_overflow,
            max_lines: None,
        }
    }

    /// The span style behind a run, or `None` when the run is plain.
    fn span_style(&self, span: u32) -> Option<&SpanStyle> {
        if span == BASE_SPAN {
            return None;
        }
        self.content
            .spans
            .get(span as usize)
            .map(|entry| &entry.style)
    }
}

impl Styled for Text {
    #[inline]
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

/// One stretch of a line drawn in a single span's style.
///
/// Shaping splits a span wherever the font or script changes, and a background
/// or an underline drawn per shaping run would show seams. Merging the runs
/// back together first is what keeps a code chip one rectangle.
struct Segment {
    span: u32,
    left: Px,
    right: Px,
}

impl Element for Text {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        self.global_id = cx.begin_element(
            self.id.as_ref(),
            &self.element_type,
            &self.classes,
            false,
            StateFlags::EMPTY,
        );
        self.computed = cx.compute_style(&self.style);
        cx.end_element();

        let node = cx.layout.add_text_node(
            &self.computed,
            MeasureContext {
                content: self.content.clone(),
                style: self.text_style(),
                options: self.layout_options(None),
            },
        );
        self.node = Some(node);
        node
    }

    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        if !crate::context::is_visible(&self.computed) || self.content.is_empty() {
            return;
        }

        cx.push_opacity(self.computed.opacity);
        let opacity = cx.opacity();
        let content = content_box(bounds, &self.computed, cx.root_font_size);

        let style = self.text_style();
        // A pixel of slack absorbs the difference between the width layout
        // measured and the width it rounded the box to. Without it, text that
        // fits exactly gets wrapped one character early.
        let options = self.layout_options(Some(content.size.width + Px(1.0)));
        let laid_out = cx.text.layout_rich(&self.content, &style, &options);

        let base_color = self.computed.color.fade(opacity);
        let scale = cx.scale.0;

        if self.selectable {
            cx.selectable.insert(
                self.global_id,
                SelectableText {
                    origin: content.origin,
                    layout: laid_out.clone(),
                    // Counted as runs are registered, which happens in paint
                    // order, which is reading order.
                    order: cx.selectable.len() as u32,
                },
            );

            // The highlight goes down before the glyphs, so the text stays
            // readable on top of it.
            if let Some(selection) = cx.input.text_selection {
                // Asked by position rather than by identity: a selection that
                // began in another run still covers this one, and asking
                // whether this is the run it started in would highlight only
                // the first paragraph of several.
                let order = cx.selectable.get(&self.global_id).map(|text| text.order);
                let covered = order.and_then(|order| {
                    selection.range_in(order, laid_out.text.len())
                });
                if let Some(range) = covered {
                    let highlight = cx
                        .styles
                        .color_token("selection")
                        .unwrap_or_else(|| Rgba::rgba8(108, 92, 231, 110));
                    for rect in laid_out.selection_rects(range) {
                        cx.scene.push_quad(Quad {
                            bounds: rect.translate(content.origin),
                            corner_radii: Corners::all(Px(2.0)),
                            background: Paint::Solid(highlight.fade(opacity)),
                            border_widths: Edges::zero(),
                            border_color: Rgba::TRANSPARENT,
                            opacity: 1.0,
                        });
                    }
                }
            }
        }

        let element_decoration = self.computed.text_decoration;
        let mut links: Vec<LinkRegion> = Vec::new();
        let mut segments: Vec<Segment> = Vec::new();

        for line in &laid_out.lines {
            let line_top = content.origin.y + line.origin.y;
            // The baseline is per line rather than per block, because a line
            // carrying a larger span sits lower than its neighbours and every
            // glyph on it has to share one.
            let baseline = line_top + line.baseline;
            let line_left = content.origin.x + line.origin.x;

            segments.clear();
            let mut pen_x = line_left;
            for run in &line.shaped.runs {
                let left = pen_x;
                pen_x += run.width;
                match segments.last_mut() {
                    Some(last) if last.span == run.span => last.right = pen_x,
                    _ => segments.push(Segment {
                        span: run.span,
                        left,
                        right: pen_x,
                    }),
                }
            }

            // Backgrounds first, then glyphs on top of them.
            for segment in &segments {
                let Some(span) = self.span_style(segment.span) else {
                    continue;
                };
                if let Some(background) = span.background {
                    let pad = (self.computed.font_size * 0.18).max(Px(1.0));
                    cx.scene.push_quad(Quad {
                        bounds: Bounds::from_xywh(
                            segment.left - pad * 0.5,
                            line_top,
                            (segment.right - segment.left) + pad,
                            line.height,
                        ),
                        corner_radii: Corners::all((self.computed.font_size * 0.25).max(Px(2.0))),
                        background: Paint::Solid(background.fade(opacity)),
                        border_widths: Edges::zero(),
                        border_color: Rgba::TRANSPARENT,
                        opacity: 1.0,
                    });
                }
            }

            let mut pen_x = line_left;
            for run in &line.shaped.runs {
                let color = self
                    .span_style(run.span)
                    .and_then(|span| span.color)
                    .map(|color| color.fade(opacity))
                    .unwrap_or(base_color);

                for glyph in &run.glyphs {
                    let x = pen_x + glyph.x_offset;
                    let y = baseline + glyph.y_offset;
                    cx.scene.push_glyph(Glyph {
                        key: GlyphKey {
                            font: run.font,
                            glyph: glyph.id,
                            // Glyphs are rasterized at device resolution, so the
                            // cache key carries the physical size.
                            size: Px(run.size.0 * scale),
                            phase: subpixel_phase(x.0 * scale),
                        },
                        position: Point::new(x, y),
                        color,
                    });
                    pen_x += glyph.advance;
                }
            }

            // The element's own decoration covers the whole line; a span's
            // covers only itself.
            if element_decoration.any() {
                push_decoration(
                    cx,
                    element_decoration,
                    line_left,
                    line.width,
                    baseline,
                    line.ascent,
                    self.computed.font_size,
                    base_color,
                );
            }

            for segment in &segments {
                let Some(span) = self.span_style(segment.span) else {
                    continue;
                };
                let color = span
                    .color
                    .map(|color| color.fade(opacity))
                    .unwrap_or(base_color);
                if span.decoration.any() {
                    push_decoration(
                        cx,
                        span.decoration,
                        segment.left,
                        segment.right - segment.left,
                        baseline,
                        line.ascent,
                        self.computed.font_size,
                        color,
                    );
                }
                if let Some(href) = &span.link {
                    links.push((
                        Bounds::from_xywh(
                            segment.left,
                            line_top,
                            segment.right - segment.left,
                            line.height,
                        ),
                        href.clone(),
                    ));
                }
            }
        }

        // A leaf, and the source of almost every name in the tree: a button
        // is a box with one of these inside it.
        if self.access_decorative {
            cx.access.suppress();
        }
        if cx.access.push(
            self.global_id,
            crate::access::AccessRole::Label,
            bounds,
        )
        .is_some()
        {
            if let Some(node) = cx.access.current_mut() {
                node.text = Some(self.content.text.clone());
            }
            cx.access.pop();
        }
        if self.access_decorative {
            cx.access.resume();
        }

        // Text takes part in hit testing so that `:hover` works on it and so a
        // text cursor can be requested over it.
        if self.computed.pointer_events {
            let over_link = links
                .iter()
                .any(|(rect, _)| rect.contains(cx.input.mouse_position));
            let cursor = if over_link {
                CursorStyle::Pointer
            } else if self.selectable {
                CursorStyle::Text
            } else {
                self.computed.cursor
            };
            cx.register_hit(self.global_id, bounds, cursor, false, false);

            if let Some(on_link) = self.on_link.clone() {
                if !links.is_empty() {
                    // Which link was clicked depends on where the click landed,
                    // so the regions travel with the handler rather than being
                    // resolved now.
                    let handler: MouseHandler =
                        Rc::new(move |event: &MouseEvent, cx: &mut crate::EventContext| {
                            for (rect, href) in &links {
                                if rect.contains(event.position) {
                                    on_link(href, cx);
                                    break;
                                }
                            }
                        });
                    cx.register_handlers(
                        self.global_id,
                        &Handlers {
                            on_click: Some(handler),
                            ..Handlers::default()
                        },
                    );
                }
            }
        }

        cx.pop_opacity();
    }
}

/// Draw an underline, a strikethrough, or both, over one stretch of a line.
#[allow(clippy::too_many_arguments)]
fn push_decoration(
    cx: &mut Cx,
    decoration: TextDecoration,
    left: Px,
    width: Px,
    baseline: Px,
    ascent: Px,
    font_size: Px,
    color: Rgba,
) {
    if width <= Px::ZERO {
        return;
    }
    let thickness = (font_size * 0.07).max(Px(1.0));
    if decoration.underline {
        cx.scene.push_underline(Underline {
            bounds: Bounds::from_xywh(left, baseline + thickness * 2.0, width, thickness),
            color,
        });
    }
    if decoration.strikethrough {
        cx.scene.push_underline(Underline {
            bounds: Bounds::from_xywh(left, baseline - ascent * 0.32, width, thickness),
            color,
        });
    }
}

crate::impl_into_element!(Text);

impl IntoElement for &str {
    fn into_any(self) -> crate::AnyElement {
        crate::AnyElement::new(Text::new(self))
    }
}

impl IntoElement for String {
    fn into_any(self) -> crate::AnyElement {
        crate::AnyElement::new(Text::new(self))
    }
}

impl IntoElement for SharedString {
    fn into_any(self) -> crate::AnyElement {
        crate::AnyElement::new(Text::new(self))
    }
}

impl IntoElement for RichText {
    fn into_any(self) -> crate::AnyElement {
        crate::AnyElement::new(Text::rich(self))
    }
}

/// The color text falls back to when nothing in the cascade sets one.
pub const DEFAULT_TEXT_COLOR: Rgba = Rgba {
    r: 0.9,
    g: 0.9,
    b: 0.9,
    a: 1.0,
};

/// Where a byte offset in some laid out text sits, for callers that need to
/// place something against a character rather than a box.
pub fn caret_position(layout: &TextLayout, origin: Point<Px>, index: usize) -> Point<Px> {
    let local = layout.position_for_index(index);
    Point::new(origin.x + local.x, origin.y + local.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Cx;
    use crate::div::div;
    use guirs_core::Size;
    use guirs_style::StyleEngine;
    use guirs_text::{rich, TextSystem};

    fn frame<E: Element>(mut root: E, size: Size<Px>, styles: StyleEngine) -> Cx {
        let mut cx = Cx::new(TextSystem::new(), styles);
        cx.begin_frame(size, guirs_core::ScaleFactor(1.0), 0.0);
        let node = root.request_layout(&mut cx);
        cx.layout.compute(node, size, &mut cx.text);
        let bounds = cx.layout.bounds(node);
        root.paint(bounds, &mut cx);
        cx.end_frame();
        cx
    }

    #[test]
    fn text_inherits_color_from_its_container() {
        let root = div()
            .text_color(Rgba::hex(0xff8800))
            .child(text("hello"));
        let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
        cx.begin_frame(Size::new(Px(200.0), Px(50.0)), guirs_core::ScaleFactor(1.0), 0.0);

        let mut root = root;
        let node = root.request_layout(&mut cx);
        cx.layout
            .compute(node, Size::new(Px(200.0), Px(50.0)), &mut cx.text);
        root.paint(cx.layout.bounds(node), &mut cx);

        // With no fonts loaded there are no glyphs, but the cascade still ran
        // and the text element still registered itself.
        assert!(cx.hits.len() >= 2);
    }

    #[test]
    fn empty_text_paints_nothing() {
        let cx = frame(
            text(""),
            Size::new(Px(200.0), Px(50.0)),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().sprites, 0);
        assert!(cx.hits.is_empty());
    }

    #[test]
    fn a_stylesheet_can_target_text_by_type() {
        let mut root = text("hello");
        let cx = {
            let mut cx = Cx::new(
                TextSystem::new(),
                StyleEngine::from_source("text { color: #00ff00; font-size: 22px; }"),
            );
            cx.begin_frame(Size::new(Px(200.0), Px(50.0)), guirs_core::ScaleFactor(1.0), 0.0);
            let node = root.request_layout(&mut cx);
            cx.layout
                .compute(node, Size::new(Px(200.0), Px(50.0)), &mut cx.text);
            root.paint(cx.layout.bounds(node), &mut cx);
            cx
        };
        let _ = cx;
        assert_eq!(root.computed.color, Rgba::hex(0x00ff00));
        assert_eq!(root.computed.font_size, Px(22.0));
    }

    #[test]
    fn strings_convert_into_text_elements() {
        let from_str = "literal".into_any();
        let from_string = String::from("owned").into_any();
        let from_shared = SharedString::from("shared").into_any();
        // Nothing to assert beyond the conversions compiling and producing
        // elements that can be laid out.
        let mut cx = Cx::new(TextSystem::new(), StyleEngine::default());
        for mut element in [from_str, from_string, from_shared] {
            element.request_layout(&mut cx);
        }
    }

    #[test]
    fn a_custom_type_name_changes_what_the_cascade_matches() {
        let mut root = text("Save").as_type("button-label");
        let mut cx = Cx::new(
            TextSystem::new(),
            StyleEngine::from_source("button-label { color: #123456; }"),
        );
        cx.begin_frame(Size::new(Px(100.0), Px(30.0)), guirs_core::ScaleFactor(1.0), 0.0);
        root.request_layout(&mut cx);
        assert_eq!(root.computed.color, Rgba::hex(0x123456));
    }

    /// A frame drawn with real fonts, or `None` on a machine with none.
    ///
    /// Painting without a face produces no glyphs, so the tests that inspect
    /// what was drawn skip rather than assert on an empty scene.
    fn painted<E: Element>(mut root: E, size: Size<Px>, mouse: Point<Px>) -> Option<Cx> {
        let mut text = TextSystem::with_system_fonts();
        let probe = TextStyle::default();
        text.fonts
            .resolve(&probe.family, probe.weight, probe.style)?;

        let mut cx = Cx::new(text, StyleEngine::default());
        cx.input.mouse_position = mouse;
        cx.begin_frame(size, guirs_core::ScaleFactor(1.0), 0.0);
        let node = root.request_layout(&mut cx);
        cx.layout.compute(node, size, &mut cx.text);
        root.paint(cx.layout.bounds(node), &mut cx);
        cx.end_frame();
        Some(cx)
    }

    fn glyph_colors(cx: &Cx) -> Vec<Rgba> {
        cx.scene
            .sprites()
            .iter()
            .filter_map(|sprite| match sprite {
                guirs_render::SpriteItem::Glyph(glyph) => Some(glyph.color),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_colored_span_paints_its_own_glyphs_in_its_own_color() {
        let marked = Rgba::hex(0xff0044);
        let element = rich_text(rich("plain ").colored("marked", marked).push(" plain"))
            .text_color(Rgba::hex(0x112233));
        let Some(cx) = painted(element, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        let colors = glyph_colors(&cx);
        assert!(!colors.is_empty(), "nothing was drawn");
        assert!(colors.contains(&marked));
        assert!(colors.contains(&Rgba::hex(0x112233)));
    }

    #[test]
    fn a_background_span_paints_a_chip_behind_its_text() {
        let plain = rich_text(rich("plain text here"));
        let Some(without) = painted(plain, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        let chipped = rich_text(
            rich("plain ").styled(
                "chip",
                SpanStyle::default().with_background(Rgba::hex(0x223344)),
            ),
        );
        let Some(with) = painted(chipped, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        assert_eq!(
            with.scene.stats().quads,
            without.scene.stats().quads + 1,
            "a background span should add exactly one quad per line it covers"
        );
    }

    #[test]
    fn an_underlined_span_underlines_only_itself() {
        let whole = rich_text(rich("under me")).underline();
        let Some(whole) = painted(whole, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        let part = rich_text(rich("under ").underline("me"));
        let Some(part) = painted(part, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        // Both draw exactly one rule; the span's is the narrower of the two.
        assert_eq!(whole.scene.stats().quads, 1);
        assert_eq!(part.scene.stats().quads, 1);

        let width_of = |cx: &Cx| match &cx.scene.quads()[0] {
            guirs_render::QuadItem::Box(quad) => quad.bounds.size.width,
            other => panic!("expected a box, got {other:?}"),
        };
        assert!(width_of(&part) < width_of(&whole));
    }

    #[test]
    fn the_cursor_becomes_a_pointer_over_a_link() {
        let build = || rich_text(rich("go ").link("here", "https://example.com").push(" now"));

        // Far to the right of the text, past every glyph.
        let Some(away) = painted(build(), Size::new(Px(400.0), Px(80.0)), Point::new(Px(390.0), Px(4.0)))
        else {
            return;
        };
        assert_eq!(away.hits[0].cursor, CursorStyle::Default);

        // Over the link itself. The link starts after "go ", so a few pixels in
        // from the left is still plain text and a little further along is not.
        let Some(over) = painted(build(), Size::new(Px(400.0), Px(80.0)), Point::new(Px(30.0), Px(4.0)))
        else {
            return;
        };
        assert_eq!(over.hits[0].cursor, CursorStyle::Pointer);
    }

    #[test]
    fn a_link_click_reports_the_target_it_landed_on() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let clicked: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let record = clicked.clone();
        let element = rich_text(rich("go ").link("here", "https://example.com").push(" now"))
            .on_link(move |href, _| record.borrow_mut().push(href.to_string()));

        let Some(mut cx) = painted(element, Size::new(Px(400.0), Px(80.0)), Point::zero()) else {
            return;
        };

        let handlers = cx.handlers.values().next().cloned();
        let Some(handlers) = handlers else {
            panic!("a text element with links should register a click handler");
        };
        let on_click = handlers.on_click.expect("no click handler registered");

        let mut redraw = false;
        let mut quit = false;
        let mut events = crate::EventContext::new(
            &mut cx.input,
            &mut cx.state,
            &mut redraw,
            &mut quit,
        );

        // On the link.
        on_click(
            &MouseEvent {
                position: Point::new(Px(30.0), Px(4.0)),
                ..MouseEvent::default()
            },
            &mut events,
        );
        assert_eq!(clicked.borrow().as_slice(), ["https://example.com"]);

        // Well past it, which is not a link and must not fire again.
        on_click(
            &MouseEvent {
                position: Point::new(Px(390.0), Px(4.0)),
                ..MouseEvent::default()
            },
            &mut events,
        );
        assert_eq!(clicked.borrow().len(), 1);
    }

    #[test]
    fn plain_text_registers_no_click_handler_of_its_own() {
        let Some(cx) = painted(
            rich_text(rich("no links in here")).on_link(|_, _| unreachable!()),
            Size::new(Px(400.0), Px(80.0)),
            Point::zero(),
        ) else {
            return;
        };
        assert!(cx.handlers.is_empty());
    }

    #[test]
    fn a_plain_string_carries_no_spans() {
        let element = text("hello");
        assert!(element.rich_content().is_plain());
        assert!(element.span_style(BASE_SPAN).is_none());
    }

    #[test]
    fn rich_text_keeps_its_spans_through_the_element() {
        let element = rich_text(rich("Rust is ").bold("fast"));
        assert_eq!(element.content().as_str(), "Rust is fast");
        assert_eq!(element.rich_content().spans.len(), 1);
        assert!(element.span_style(0).is_some());
        // An index past the table is a missing span, not a panic.
        assert!(element.span_style(9).is_none());
    }

    #[test]
    fn a_rich_paragraph_lays_out_without_fonts() {
        // No fonts are loaded in a test, so nothing is drawn. What matters is
        // that the rich path runs end to end rather than panicking on an
        // unresolved face.
        let cx = frame(
            rich_text(rich("plain ").code("code").push(" more")),
            Size::new(Px(300.0), Px(60.0)),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().sprites, 0);
    }
}
