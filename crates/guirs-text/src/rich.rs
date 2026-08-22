//! Rich text: one block of text, more than one style in it.
//!
//! A paragraph of prose is rarely uniform. Part of it is bold, a word is a
//! piece of code, a phrase is a link. Laying that out is not the same problem
//! as laying out a uniform string, because the line breaker has to measure each
//! piece with its own font and the baseline of a line has to accommodate the
//! tallest face on it.
//!
//! A [`RichText`] is one string plus a sorted, non overlapping list of spans
//! over it. Bytes no span covers use the surrounding element's style, so a
//! caller only describes what differs:
//!
//! ```
//! # use guirs_text::rich;
//! let body = rich("Rust is ").bold("fast").push(" and ").italic("safe").build();
//! assert_eq!(body.as_str(), "Rust is fast and safe");
//! assert_eq!(body.spans.len(), 2);
//! ```
//!
//! Spans carry color and decoration as well as font selection. That is a paint
//! concern rather than a shaping one, but keeping it here means a caller
//! describes a styled run once instead of maintaining a parallel table that has
//! to stay in step with the ranges.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use guirs_core::{Px, Rgba, SharedString};
use guirs_style::{FontFamily, FontStyle, FontWeight, TextDecoration};

use crate::shape::TextStyle;

/// The span index reported by a run that no span covers.
///
/// Such a run is drawn with the element's own style, so the painter needs to
/// tell it apart from span zero.
pub const BASE_SPAN: u32 = u32::MAX;

/// What a span changes about the text it covers.
///
/// Every field is an override. `None` means "whatever the surrounding text
/// uses", which is what keeps a span describing only boldness in step with a
/// stylesheet that later changes the font.
#[derive(Clone, Debug, Default)]
pub struct SpanStyle {
    pub family: Option<FontFamily>,
    pub weight: Option<FontWeight>,
    pub style: Option<FontStyle>,
    /// An absolute size, in logical pixels.
    pub size: Option<Px>,
    /// A size relative to the surrounding text. Applied after `size`, so a
    /// heading scales with the element it sits in rather than pinning itself.
    pub size_scale: Option<f32>,
    pub letter_spacing: Option<Px>,
    pub color: Option<Rgba>,
    /// Drawn behind the span, inset to the line box. Used for code chips.
    pub background: Option<Rgba>,
    pub decoration: TextDecoration,
    /// Where this span points, if anywhere. The text layer never follows it;
    /// it is carried so the element painting the text can make the span
    /// clickable without a second table keyed by the same ranges.
    pub link: Option<SharedString>,
}

impl SpanStyle {
    /// Resolve against the style of the text around the span.
    pub fn apply(&self, base: &TextStyle) -> TextStyle {
        let mut size = self.size.unwrap_or(base.size);
        if let Some(scale) = self.size_scale {
            size = Px(size.0 * scale);
        }
        TextStyle {
            family: self.family.clone().unwrap_or_else(|| base.family.clone()),
            weight: self.weight.unwrap_or(base.weight),
            style: self.style.unwrap_or(base.style),
            size,
            letter_spacing: self.letter_spacing.unwrap_or(base.letter_spacing),
        }
    }

    pub fn bold() -> Self {
        SpanStyle {
            weight: Some(FontWeight::BOLD),
            ..Default::default()
        }
    }

    pub fn italic() -> Self {
        SpanStyle {
            style: Some(FontStyle::Italic),
            ..Default::default()
        }
    }

    pub fn underline() -> Self {
        SpanStyle {
            decoration: TextDecoration::UNDERLINE,
            ..Default::default()
        }
    }

    pub fn strikethrough() -> Self {
        SpanStyle {
            decoration: TextDecoration {
                underline: false,
                strikethrough: true,
            },
            ..Default::default()
        }
    }

    pub fn colored(color: Rgba) -> Self {
        SpanStyle {
            color: Some(color),
            ..Default::default()
        }
    }

    pub fn with_weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn with_family(mut self, family: impl Into<FontFamily>) -> Self {
        self.family = Some(family.into());
        self
    }

    pub fn with_size(mut self, size: Px) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.size_scale = Some(scale);
        self
    }

    pub fn with_color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn with_background(mut self, color: Rgba) -> Self {
        self.background = Some(color);
        self
    }

    pub fn with_link(mut self, href: impl Into<SharedString>) -> Self {
        self.link = Some(href.into());
        self
    }

    /// Whether this span changes anything about how the text is shaped.
    ///
    /// A span that only recolors can share a shaped run with its neighbours,
    /// which keeps a highlighted phrase from splitting a ligature.
    pub fn affects_shaping(&self) -> bool {
        self.family.is_some()
            || self.weight.is_some()
            || self.style.is_some()
            || self.size.is_some()
            || self.size_scale.is_some()
            || self.letter_spacing.is_some()
    }
}

fn color_bits(color: &Option<Rgba>) -> Option<[u32; 4]> {
    color.map(|c| [c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits()])
}

// Rgba and f32 have no total ordering, so the derived traits are unavailable.
// Comparing and hashing the bit patterns is exactly what a cache key wants: two
// styles are interchangeable only if they are identical, and NaN never reaches
// these fields.
impl PartialEq for SpanStyle {
    fn eq(&self, other: &Self) -> bool {
        self.family == other.family
            && self.weight == other.weight
            && self.style == other.style
            && self.size == other.size
            && self.size_scale.map(f32::to_bits) == other.size_scale.map(f32::to_bits)
            && self.letter_spacing == other.letter_spacing
            && color_bits(&self.color) == color_bits(&other.color)
            && color_bits(&self.background) == color_bits(&other.background)
            && self.decoration == other.decoration
            && self.link == other.link
    }
}

impl Eq for SpanStyle {}

impl Hash for SpanStyle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.family.hash(state);
        self.weight.hash(state);
        self.style.hash(state);
        self.size.hash(state);
        self.size_scale.map(f32::to_bits).hash(state);
        self.letter_spacing.hash(state);
        color_bits(&self.color).hash(state);
        color_bits(&self.background).hash(state);
        self.decoration.underline.hash(state);
        self.decoration.strikethrough.hash(state);
        self.link.hash(state);
    }
}

/// A styled slice of a rich text block.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextSpan {
    /// Byte range in the block's text. Spans are sorted and never overlap.
    pub range: Range<usize>,
    pub style: SpanStyle,
}

/// A block of text with styled spans over it.
///
/// Cloning is cheap: the text and the span table are both shared, which matters
/// because a declarative interface rebuilds its tree every frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RichText {
    pub text: SharedString,
    pub spans: Arc<[TextSpan]>,
}

impl Default for RichText {
    fn default() -> Self {
        RichText {
            text: SharedString::default(),
            spans: no_spans(),
        }
    }
}

/// The one empty span table, shared by every plain string.
///
/// An empty `Arc<[T]>` still allocates its header, and every plain label in a
/// tree would pay for one every frame. Handing out clones of a single one
/// makes wrapping a string cost a refcount bump.
fn no_spans() -> Arc<[TextSpan]> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<Arc<[TextSpan]>> = OnceLock::new();
    EMPTY.get_or_init(|| Arc::from(Vec::new())).clone()
}

impl RichText {
    /// Wrap a plain string. No spans, so it lays out exactly like plain text.
    pub fn plain(text: impl Into<SharedString>) -> Self {
        RichText {
            text: text.into(),
            spans: no_spans(),
        }
    }

    /// Assemble from text and spans directly.
    ///
    /// The spans are sorted and clipped to the text, and overlaps resolved in
    /// favour of whichever span starts first, because the layout code depends
    /// on both and a caller assembling ranges by hand should not have to
    /// guarantee them.
    pub fn new(text: impl Into<SharedString>, spans: Vec<TextSpan>) -> Self {
        let text = text.into();
        if spans.is_empty() {
            return RichText::plain(text);
        }
        let normalized = normalize(spans, text.len());
        if normalized.is_empty() {
            return RichText::plain(text);
        }
        RichText {
            spans: Arc::from(normalized),
            text,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    /// Whether anything here needs the rich text path at all.
    #[inline]
    pub fn is_plain(&self) -> bool {
        self.spans.is_empty()
    }

    /// The span covering a byte offset, if any.
    pub fn span_at(&self, byte: usize) -> Option<&TextSpan> {
        find_span(&self.spans, byte).map(|index| &self.spans[index])
    }

    /// The link at a byte offset, if the span there is one.
    pub fn link_at(&self, byte: usize) -> Option<&SharedString> {
        self.span_at(byte).and_then(|span| span.style.link.as_ref())
    }

    /// Start building. The initial text is unstyled.
    pub fn builder(text: impl AsRef<str>) -> RichTextBuilder {
        RichTextBuilder::new().push(text)
    }
}

impl From<&str> for RichText {
    fn from(text: &str) -> Self {
        RichText::plain(text)
    }
}

impl From<String> for RichText {
    fn from(text: String) -> Self {
        RichText::plain(text)
    }
}

impl From<SharedString> for RichText {
    fn from(text: SharedString) -> Self {
        RichText::plain(text)
    }
}

impl From<RichTextBuilder> for RichText {
    fn from(builder: RichTextBuilder) -> Self {
        builder.build()
    }
}

/// Which span covers a byte, by binary search over sorted, disjoint ranges.
pub(crate) fn find_span(spans: &[TextSpan], byte: usize) -> Option<usize> {
    use std::cmp::Ordering;
    spans
        .binary_search_by(|span| {
            if byte < span.range.start {
                Ordering::Greater
            } else if byte >= span.range.end {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .ok()
}

/// Sort, clip and de-overlap a span list.
fn normalize(mut spans: Vec<TextSpan>, len: usize) -> Vec<TextSpan> {
    spans.sort_by_key(|span| (span.range.start, span.range.end));
    let mut out: Vec<TextSpan> = Vec::with_capacity(spans.len());
    let mut cursor = 0usize;
    for mut span in spans {
        span.range.start = span.range.start.max(cursor).min(len);
        span.range.end = span.range.end.min(len);
        if span.range.start >= span.range.end {
            continue;
        }
        cursor = span.range.end;
        out.push(span);
    }
    out
}

/// Assembles a [`RichText`] a piece at a time.
///
/// Each method appends text and records the range it occupies, so a caller
/// never computes a byte offset by hand.
#[derive(Clone, Debug, Default)]
pub struct RichTextBuilder {
    text: String,
    spans: Vec<TextSpan>,
}

impl RichTextBuilder {
    pub fn new() -> Self {
        RichTextBuilder::default()
    }

    /// Append text in the surrounding style.
    pub fn push(mut self, text: impl AsRef<str>) -> Self {
        self.text.push_str(text.as_ref());
        self
    }

    /// Append text in a style of its own.
    pub fn styled(mut self, text: impl AsRef<str>, style: SpanStyle) -> Self {
        let text = text.as_ref();
        if text.is_empty() {
            return self;
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.spans.push(TextSpan {
            range: start..self.text.len(),
            style,
        });
        self
    }

    pub fn bold(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::bold())
    }

    pub fn italic(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::italic())
    }

    pub fn underline(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::underline())
    }

    pub fn strikethrough(self, text: impl AsRef<str>) -> Self {
        self.styled(text, SpanStyle::strikethrough())
    }

    pub fn colored(self, text: impl AsRef<str>, color: Rgba) -> Self {
        self.styled(text, SpanStyle::colored(color))
    }

    /// Append a link: underlined, carrying its target.
    pub fn link(self, text: impl AsRef<str>, href: impl Into<SharedString>) -> Self {
        self.styled(
            text,
            SpanStyle {
                decoration: TextDecoration::UNDERLINE,
                link: Some(href.into()),
                ..Default::default()
            },
        )
    }

    /// Append a run of code: monospace, slightly smaller so it sits level with
    /// the prose around it rather than towering over it.
    pub fn code(self, text: impl AsRef<str>) -> Self {
        self.styled(
            text,
            SpanStyle {
                family: Some(FontFamily::from(["monospace"])),
                size_scale: Some(0.92),
                ..Default::default()
            },
        )
    }

    /// Append a line break.
    pub fn newline(self) -> Self {
        self.push("\n")
    }

    pub fn build(self) -> RichText {
        RichText::new(self.text, self.spans)
    }
}

/// Start building rich text from an unstyled opening.
pub fn rich(text: impl AsRef<str>) -> RichTextBuilder {
    RichText::builder(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builder_records_the_ranges_it_wrote() {
        let built = rich("Rust is ")
            .bold("fast")
            .push(" and ")
            .italic("safe")
            .build();
        assert_eq!(built.as_str(), "Rust is fast and safe");
        assert_eq!(built.spans.len(), 2);
        assert_eq!(&built.as_str()[built.spans[0].range.clone()], "fast");
        assert_eq!(&built.as_str()[built.spans[1].range.clone()], "safe");
    }

    #[test]
    fn an_empty_piece_records_no_span() {
        let built = rich("a").bold("").push("b").build();
        assert_eq!(built.as_str(), "ab");
        assert!(built.spans.is_empty());
        assert!(built.is_plain());
    }

    #[test]
    fn overlapping_spans_are_resolved_in_favour_of_the_first() {
        let built = RichText::new(
            "abcdef",
            vec![
                TextSpan {
                    range: 0..4,
                    style: SpanStyle::bold(),
                },
                TextSpan {
                    range: 2..6,
                    style: SpanStyle::italic(),
                },
            ],
        );
        assert_eq!(built.spans[0].range, 0..4);
        assert_eq!(built.spans[1].range, 4..6);
    }

    #[test]
    fn spans_are_clipped_to_the_text() {
        let built = RichText::new(
            "abc",
            vec![TextSpan {
                range: 1..99,
                style: SpanStyle::bold(),
            }],
        );
        assert_eq!(built.spans[0].range, 1..3);
    }

    #[test]
    fn lookup_finds_the_covering_span_and_nothing_else() {
        let built = rich("go ")
            .link("here", "https://example.com")
            .push(" now")
            .build();
        assert!(built.span_at(0).is_none());
        assert_eq!(
            built.link_at(4).map(|href| href.as_str()),
            Some("https://example.com")
        );
        assert!(built.link_at(7).is_none());
    }

    #[test]
    fn a_span_style_resolves_against_the_text_around_it() {
        let base = TextStyle {
            size: Px(16.0),
            weight: FontWeight::NORMAL,
            ..Default::default()
        };
        let resolved = SpanStyle::bold().with_scale(0.5).apply(&base);
        assert_eq!(resolved.weight, FontWeight::BOLD);
        assert_eq!(resolved.size, Px(8.0));
        // Untouched fields come from the base.
        assert_eq!(resolved.family, base.family);
    }

    #[test]
    fn recoloring_alone_does_not_change_shaping() {
        assert!(!SpanStyle::colored(Rgba::hex(0xff0000)).affects_shaping());
        assert!(SpanStyle::bold().affects_shaping());
        assert!(!SpanStyle::underline().affects_shaping());
    }
}
