//! Text shaping.
//!
//! Turning a string into positioned glyphs takes four passes:
//!
//! 1. Bidi analysis assigns an embedding level to every character.
//! 2. The text is split into runs of uniform level, script and font. Font
//!    changes come from fallback: the moment a character has no glyph in the
//!    requested face, a new run starts in whichever fallback face covers it.
//! 3. Each run is shaped, applying the font's own substitution and positioning
//!    tables, which is what produces ligatures, mark attachment and the
//!    contextual forms complex scripts need.
//! 4. Runs are reordered into visual order for display.
//!
//! Shaping is the most expensive step in text rendering, so every run is cached
//! by its text, face, size and direction.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use guirs_core::{Px, SharedString};
use guirs_style::{FontFamily, FontStyle, FontWeight};
use swash::shape::{Direction, ShapeContext};
use swash::text::{Codepoint, Script};
use unicode_bidi::{BidiInfo, Level};

use crate::font::{FontId, FontSystem};
use crate::rich::{find_span, TextSpan, BASE_SPAN};

/// The subset of a computed style that affects shaping.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub family: FontFamily,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub size: Px,
    pub letter_spacing: Px,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            family: FontFamily::default(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            size: Px(14.0),
            letter_spacing: Px::ZERO,
        }
    }
}

/// One positioned glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    /// Index of the glyph within its face.
    pub id: u16,
    /// How far the pen moves after drawing this glyph.
    pub advance: Px,
    /// Horizontal adjustment applied to this glyph only.
    pub x_offset: Px,
    /// Vertical adjustment applied to this glyph only.
    pub y_offset: Px,
    /// Byte offset in the original text this glyph came from.
    pub cluster: u32,
}

/// A run of glyphs sharing one face, size and direction.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: FontId,
    pub size: Px,
    /// True when the run reads right to left.
    pub rtl: bool,
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance of the run.
    pub width: Px,
    /// Byte range in the original text.
    pub range: Range<usize>,
    /// Which rich text span this run came from, or [`BASE_SPAN`] when no span
    /// covers it and it should be drawn in the element's own style.
    pub span: u32,
}

impl ShapedRun {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// A line of runs, already in visual order.
#[derive(Clone, Debug, Default)]
pub struct ShapedLine {
    pub runs: Vec<ShapedRun>,
    pub width: Px,
    pub range: Range<usize>,
}

/// The style in force at each byte of a block, resolved once.
///
/// Plain text carries no spans, so the lookup collapses to returning the base
/// and the rich and uniform paths stay one piece of code rather than two that
/// drift apart.
pub struct SpanStyles<'a> {
    base: &'a TextStyle,
    base_font: Option<FontId>,
    spans: &'a [TextSpan],
    /// Style and face per span, in the same order. Resolving a family to a
    /// face touches the font database, so it is done once here rather than
    /// once per character.
    resolved: Vec<(TextStyle, Option<FontId>)>,
}

impl<'a> SpanStyles<'a> {
    /// One style for the whole block.
    pub fn uniform(fonts: &mut FontSystem, base: &'a TextStyle) -> Self {
        SpanStyles {
            base_font: fonts.resolve(&base.family, base.weight, base.style),
            base,
            spans: &[],
            resolved: Vec::new(),
        }
    }

    /// A base style with spans overriding parts of it.
    pub fn new(fonts: &mut FontSystem, base: &'a TextStyle, spans: &'a [TextSpan]) -> Self {
        let resolved = spans
            .iter()
            .map(|span| {
                let style = span.style.apply(base);
                let font = fonts.resolve(&style.family, style.weight, style.style);
                (style, font)
            })
            .collect();
        SpanStyles {
            base_font: fonts.resolve(&base.family, base.weight, base.style),
            base,
            spans,
            resolved,
        }
    }

    #[inline]
    pub fn is_uniform(&self) -> bool {
        self.spans.is_empty()
    }

    #[inline]
    pub fn base(&self) -> &TextStyle {
        self.base
    }

    /// The span index, style and face that apply at a byte offset.
    ///
    /// The offset is in the coordinates the spans use, which is the whole
    /// block rather than the line being shaped.
    #[inline]
    fn at(&self, byte: usize) -> (u32, &TextStyle, Option<FontId>) {
        if self.spans.is_empty() {
            return (BASE_SPAN, self.base, self.base_font);
        }
        match find_span(self.spans, byte) {
            Some(index) => {
                let (style, font) = &self.resolved[index];
                (index as u32, style, *font)
            }
            None => (BASE_SPAN, self.base, self.base_font),
        }
    }

    /// The byte at or after `from` where the style changes, within `limit`.
    ///
    /// The line breaker measures piece by piece, and a piece that straddles a
    /// span boundary has to be measured in parts.
    pub fn next_boundary(&self, from: usize, limit: usize) -> usize {
        if self.spans.is_empty() {
            return limit;
        }
        for span in self.spans {
            if span.range.start > from {
                return span.range.start.min(limit);
            }
            if span.range.end > from {
                return span.range.end.min(limit);
            }
        }
        limit
    }

    /// The style covering a byte, for callers outside shaping.
    #[inline]
    pub fn style_at(&self, byte: usize) -> &TextStyle {
        self.at(byte).1
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    text: SharedString,
    font: FontId,
    size: Px,
    letter_spacing: Px,
    rtl: bool,
    script: u8,
}

/// How much the shaped run cache may hold before it is dropped wholesale.
///
/// Bounded by bytes rather than by entries, because a run is anywhere from one
/// glyph to a whole line and a count says nothing about what that costs.
const RUN_CACHE_BUDGET: usize = 1024 * 1024;

/// Shapes text and caches the result.
///
/// A run is cached by its text, face, size and direction, so the same word in
/// the same style is shaped once however many times it appears. The cache is
/// bounded by bytes and cleared wholesale by [`Shaper::trim_cache`]: anything
/// still on screen is reshaped on the next frame, and one screenful of text
/// refills it in that frame.
pub struct Shaper {
    ctx: ShapeContext,
    cache: HashMap<RunKey, Arc<ShapedRun>>,
    /// Roughly what the cache is holding, tallied as runs go in rather than
    /// measured by walking it, which a frame cannot afford.
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl Default for Shaper {
    fn default() -> Self {
        Shaper::new()
    }
}

impl std::fmt::Debug for Shaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shaper")
            .field("cached_runs", &self.cache.len())
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish()
    }
}

impl Shaper {
    pub fn new() -> Self {
        Shaper {
            ctx: ShapeContext::new(),
            cache: HashMap::new(),
            bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Roughly how much the shaped run cache is holding.
    #[inline]
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }

    pub fn cached_runs(&self) -> usize {
        self.cache.len()
    }

    /// Cache hits and misses since construction.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Drop every cached run. Call after changing the font database.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.bytes = 0;
    }

    /// Keep the cache from growing without bound in long running sessions.
    ///
    /// Wholesale rather than by eviction: a run still on screen is reshaped on
    /// the next frame, and the working set of a shaping cache is one screenful
    /// of text, which refills in a single frame.
    pub fn trim_cache(&mut self) {
        if self.bytes > RUN_CACHE_BUDGET {
            self.clear_cache();
        }
    }

    /// Total advance width of a string, without producing a layout.
    pub fn measure(&mut self, fonts: &mut FontSystem, text: &str, style: &TextStyle) -> Px {
        if text.is_empty() {
            return Px::ZERO;
        }
        self.shape_line(fonts, text, 0, style, Level::ltr()).width
    }

    /// Total advance width of a slice of a styled block.
    ///
    /// `offset` is where the slice starts in the block, which is what makes
    /// the spans line up. A slice straddling a boundary is measured in parts,
    /// because a span boundary ends a shaping run.
    pub fn measure_spans(
        &mut self,
        fonts: &mut FontSystem,
        text: &str,
        offset: usize,
        styles: &SpanStyles<'_>,
    ) -> Px {
        if text.is_empty() {
            return Px::ZERO;
        }
        if styles.is_uniform() {
            return self.measure(fonts, text, styles.base());
        }

        let end = offset + text.len();
        let mut width = Px::ZERO;
        let mut cursor = offset;
        while cursor < end {
            let mut boundary = styles.next_boundary(cursor, end).max(cursor + 1).min(end);
            // Never split inside a character.
            while boundary < end && !text.is_char_boundary(boundary - offset) {
                boundary += 1;
            }
            let piece = &text[cursor - offset..boundary - offset];
            width += self
                .shape_line_with(fonts, piece, cursor, styles, Level::ltr())
                .width;
            cursor = boundary;
        }
        width
    }

    /// Shape one line of text into visually ordered runs.
    ///
    /// `byte_offset` is added to every reported byte range, so a caller
    /// shaping a slice of a larger document gets ranges in document
    /// coordinates.
    pub fn shape_line(
        &mut self,
        fonts: &mut FontSystem,
        text: &str,
        byte_offset: usize,
        style: &TextStyle,
        base_level: Level,
    ) -> ShapedLine {
        let styles = SpanStyles::uniform(fonts, style);
        self.shape_line_with(fonts, text, byte_offset, &styles, base_level)
    }

    /// Shape one line whose style varies along it.
    ///
    /// Bidi runs first and span splitting second, which is the order that
    /// matters: reordering is a property of the text, not of how it is styled,
    /// so a bold word inside an Arabic phrase must not change where the
    /// direction changes.
    pub fn shape_line_with(
        &mut self,
        fonts: &mut FontSystem,
        text: &str,
        byte_offset: usize,
        styles: &SpanStyles<'_>,
        base_level: Level,
    ) -> ShapedLine {
        if text.is_empty() {
            return ShapedLine {
                runs: Vec::new(),
                width: Px::ZERO,
                range: byte_offset..byte_offset,
            };
        }

        let bidi = BidiInfo::new(text, Some(base_level));
        let mut runs = Vec::new();

        if bidi.paragraphs.is_empty() {
            // No paragraph at all means no directional content worth reordering.
            self.append_logical_runs(fonts, text, 0, byte_offset, styles, false, &mut runs);
        } else {
            let para = &bidi.paragraphs[0];
            let (levels, visual_ranges) = bidi.visual_runs(para, 0..text.len());
            for range in visual_ranges {
                let level = levels[range.start];
                self.append_logical_runs(
                    fonts,
                    &text[range.clone()],
                    range.start,
                    byte_offset,
                    styles,
                    level.is_rtl(),
                    &mut runs,
                );
            }
        }

        let width = runs.iter().fold(Px::ZERO, |acc, run| acc + run.width);
        ShapedLine {
            runs,
            width,
            range: byte_offset..byte_offset + text.len(),
        }
    }

    /// Split one directional span into script, font and style runs, then shape
    /// each.
    #[allow(clippy::too_many_arguments)]
    fn append_logical_runs(
        &mut self,
        fonts: &mut FontSystem,
        text: &str,
        span_start: usize,
        byte_offset: usize,
        styles: &SpanStyles<'_>,
        rtl: bool,
        out: &mut Vec<ShapedRun>,
    ) {
        if text.is_empty() {
            return;
        }

        // Segments are found in logical order. Inside a right to left range the
        // first one logically is the rightmost one visually, so the batch this
        // call produces is reversed before it joins the line.
        let first = out.len();

        let mut segment_start = 0usize;
        let mut segment_font = None;
        let mut segment_script = Script::Latin;
        let mut segment_span = BASE_SPAN;
        let mut segment_style = styles.base().clone();
        let mut have_real_script = false;

        for (index, ch) in text.char_indices() {
            let absolute = span_start + index + byte_offset;
            let (span, style, base_font) = styles.at(absolute);
            let Some(base_font) = base_font else {
                // No face at all for this style. Skip rather than panic, so a
                // missing font degrades to blank text instead of a crash.
                continue;
            };
            let font = fonts.resolve_for_char(base_font, ch, style.weight, style.style);
            let raw_script = ch.script();
            // Common and inherited characters (spaces, digits, punctuation,
            // combining marks) join whatever run they find themselves in
            // rather than starting one of their own.
            let script = if raw_script.is_real() {
                raw_script
            } else if have_real_script {
                segment_script
            } else {
                Script::Latin
            };

            let starts_new = match segment_font {
                None => true,
                Some(current) => {
                    current != font || script != segment_script || span != segment_span
                }
            };

            if starts_new {
                if let Some(previous) = segment_font {
                    self.shape_segment(
                        fonts,
                        &text[segment_start..index],
                        span_start + segment_start + byte_offset,
                        &segment_style,
                        previous,
                        segment_script,
                        rtl,
                        segment_span,
                        out,
                    );
                    segment_start = index;
                }
                segment_font = Some(font);
                segment_script = script;
                segment_span = span;
                segment_style = style.clone();
            }
            if raw_script.is_real() {
                have_real_script = true;
            }
        }

        if let Some(font) = segment_font {
            self.shape_segment(
                fonts,
                &text[segment_start..],
                span_start + segment_start + byte_offset,
                &segment_style,
                font,
                segment_script,
                rtl,
                segment_span,
                out,
            );
        }

        if rtl {
            out[first..].reverse();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_segment(
        &mut self,
        fonts: &FontSystem,
        text: &str,
        absolute_start: usize,
        style: &TextStyle,
        font: FontId,
        script: Script,
        rtl: bool,
        span: u32,
        out: &mut Vec<ShapedRun>,
    ) {
        if text.is_empty() {
            return;
        }

        let key = RunKey {
            text: SharedString::from(text),
            font,
            size: style.size,
            letter_spacing: style.letter_spacing,
            rtl,
            script: script as u8,
        };

        let shaped = match self.cache.get(&key) {
            Some(hit) => {
                self.hits += 1;
                hit.clone()
            }
            None => {
                self.misses += 1;
                let run = Arc::new(self.shape_uncached(fonts, text, style, font, script, rtl));
                self.bytes += key.text.len()
                    + std::mem::size_of::<RunKey>()
                    + std::mem::size_of::<ShapedRun>()
                    + run.glyphs.len() * std::mem::size_of::<ShapedGlyph>();
                self.cache.insert(key, run.clone());
                run
            }
        };

        // The cached run is relative to its own text, so rebase the byte
        // ranges into the caller's coordinates. The span is assigned here
        // rather than cached, so two spans that resolve to the same font and
        // size share one shaped run.
        let mut run = (*shaped).clone();
        run.span = span;
        run.range = absolute_start..absolute_start + text.len();
        for glyph in &mut run.glyphs {
            glyph.cluster += absolute_start as u32;
        }
        out.push(run);
    }

    fn shape_uncached(
        &mut self,
        fonts: &FontSystem,
        text: &str,
        style: &TextStyle,
        font: FontId,
        script: Script,
        rtl: bool,
    ) -> ShapedRun {
        let empty = ShapedRun {
            font,
            size: style.size,
            rtl,
            glyphs: Vec::new(),
            width: Px::ZERO,
            range: 0..text.len(),
            span: BASE_SPAN,
        };

        let Some(face) = fonts.try_face(font) else {
            return empty;
        };
        let Some(font_ref) = face.font_ref() else {
            return empty;
        };

        let mut shaper = self
            .ctx
            .builder(font_ref)
            .script(script)
            .direction(if rtl {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            })
            .size(style.size.0)
            .build();

        shaper.add_str(text);

        let mut glyphs: Vec<ShapedGlyph> = Vec::new();
        let spacing = style.letter_spacing;
        shaper.shape_with(|cluster| {
            let cluster_start = cluster.source.start;
            for glyph in cluster.glyphs {
                glyphs.push(ShapedGlyph {
                    id: glyph.id,
                    advance: Px(glyph.advance) + spacing,
                    x_offset: Px(glyph.x),
                    y_offset: Px(glyph.y),
                    cluster: cluster_start,
                });
            }
        });

        let width = glyphs
            .iter()
            .fold(Px::ZERO, |acc, glyph| acc + glyph.advance);

        ShapedRun {
            font,
            size: style.size,
            rtl,
            glyphs,
            width,
            range: 0..text.len(),
            span: BASE_SPAN,
        }
    }
}

/// Whether a string contains anything that needs bidi reordering.
///
/// A fast pre-check: the overwhelming majority of UI strings are pure left to
/// right, and skipping bidi entirely for those is worth the scan.
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch as u32,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl_detection_finds_hebrew_and_arabic() {
        assert!(!has_rtl("hello world"));
        assert!(!has_rtl("\u{65e5}\u{672c}\u{8a9e}"));
        assert!(has_rtl("\u{05e9}\u{05dc}\u{05d5}\u{05dd}"));
        assert!(has_rtl("\u{0645}\u{0631}\u{062d}\u{0628}\u{0627}"));
        assert!(has_rtl("mixed \u{0645}\u{0631} text"));
    }

    #[test]
    fn shaping_with_no_fonts_yields_an_empty_line() {
        let mut fonts = FontSystem::new();
        let mut shaper = Shaper::new();
        let line = shaper.shape_line(&mut fonts, "hello", 0, &TextStyle::default(), Level::ltr());
        assert!(line.runs.is_empty());
        assert_eq!(line.width, Px::ZERO);
        assert_eq!(line.range, 0..5);
    }

    #[test]
    fn measuring_an_empty_string_is_zero() {
        let mut fonts = FontSystem::new();
        let mut shaper = Shaper::new();
        assert_eq!(shaper.measure(&mut fonts, "", &TextStyle::default()), Px::ZERO);
    }

    #[test]
    fn the_default_text_style_is_usable() {
        let style = TextStyle::default();
        assert_eq!(style.weight, FontWeight::NORMAL);
        assert_eq!(style.size, Px(14.0));
        assert_eq!(style.letter_spacing, Px::ZERO);
    }
}
