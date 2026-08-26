//! Line breaking, alignment and hit testing.
//!
//! Wrapping is greedy and works on break units from the Unicode word
//! segmentation algorithm, so it breaks in the places a reader expects and
//! never inside a grapheme cluster. A word wider than the whole line is broken
//! by grapheme instead of being allowed to overflow.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use guirs_core::{Bounds, Point, Px, SharedString, Size};
use guirs_style::{LineHeight, TextAlign, TextOverflow, WhiteSpace};
use unicode_bidi::Level;
use unicode_segmentation::UnicodeSegmentation;

use crate::font::{FontSystem, ScaledMetrics};
use crate::raster::GlyphRasterizer;
use crate::rich::{RichText, TextSpan};
use crate::shape::{has_rtl, ShapedLine, Shaper, SpanStyles, TextStyle};

/// The character appended when text is truncated.
pub const ELLIPSIS: &str = "\u{2026}";

/// How a block of text should be broken and positioned.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutOptions {
    /// The width to wrap at. `None` means never wrap.
    pub max_width: Option<Px>,
    pub line_height: LineHeight,
    pub align: TextAlign,
    pub white_space: WhiteSpace,
    pub overflow: TextOverflow,
    /// Stop after this many lines, truncating the last one if the overflow
    /// setting asks for it.
    pub max_lines: Option<usize>,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            max_width: None,
            line_height: LineHeight::Normal,
            align: TextAlign::Start,
            white_space: WhiteSpace::Normal,
            overflow: TextOverflow::Clip,
            max_lines: None,
        }
    }
}

/// One line, positioned within the text block.
#[derive(Clone, Debug)]
pub struct LaidOutLine {
    pub shaped: ShapedLine,
    /// Top left of the line box, relative to the text block's origin.
    pub origin: Point<Px>,
    /// Distance from the line box top down to the baseline, with any extra
    /// line height already shared out above and below.
    ///
    /// This is per line rather than per block because a line carrying a larger
    /// span sits on a lower baseline than its neighbours, and every glyph on a
    /// line has to share one.
    pub baseline: Px,
    pub height: Px,
    pub width: Px,
    /// Tallest ascent and deepest descent among the faces used on this line.
    pub ascent: Px,
    pub descent: Px,
}

impl LaidOutLine {
    /// Byte range this line covers within the laid out text.
    #[inline]
    pub fn range(&self) -> Range<usize> {
        self.shaped.range.clone()
    }
}

/// A fully laid out block of text.
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    /// The text as laid out. When whitespace collapsing applies this differs
    /// from the input, and every byte offset here refers to this string.
    pub text: SharedString,
    pub lines: Vec<LaidOutLine>,
    pub size: Size<Px>,
    pub line_height: Px,
    pub ascent: Px,
    pub descent: Px,
    /// Whether the block reads right to left overall.
    pub rtl: bool,
    /// Whether anything was dropped to fit the constraints.
    pub truncated: bool,
}

impl TextLayout {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[inline]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The index of the line containing `y`, clamped into range.
    pub fn line_at(&self, y: Px) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        for (i, line) in self.lines.iter().enumerate() {
            if y < line.origin.y + line.height {
                return i;
            }
        }
        self.lines.len() - 1
    }

    /// Where the caret sits for a byte offset, relative to the block origin.
    pub fn position_for_index(&self, index: usize) -> Point<Px> {
        // Inclusive at the line's end on purpose. An offset at the end of one
        // line is also the start of the next, and putting the caret on the
        // following line means a caret typed to the end of a line jumps down
        // before anything has been typed there. It also made moving down out
        // of such a position skip a line, because the caret was already
        // reported to be on the one below.
        let Some(line_index) = self
            .lines
            .iter()
            .position(|line| index <= line.shaped.range.end)
            .or(self.lines.len().checked_sub(1))
        else {
            return Point::zero();
        };
        let line = &self.lines[line_index];
        let y = line.origin.y;

        let mut x = line.origin.x;
        for run in &line.shaped.runs {
            for glyph in &run.glyphs {
                if glyph.cluster as usize >= index {
                    // In a right to left run the caret for a cluster sits on
                    // its trailing, that is right hand, edge.
                    return Point::new(if run.rtl { x + glyph.advance } else { x }, y);
                }
                x += glyph.advance;
            }
        }
        Point::new(x, y)
    }

    /// The byte offset nearest to a point, for click and drag selection.
    pub fn index_for_position(&self, point: Point<Px>) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let line = &self.lines[self.line_at(point.y)];
        let mut x = line.origin.x;

        for run in &line.shaped.runs {
            for glyph in &run.glyphs {
                let next = x + glyph.advance;
                if point.x < next {
                    // Snap to whichever edge of the glyph is closer.
                    let past_middle = point.x > x + glyph.advance * 0.5;
                    let leading = glyph.cluster as usize;
                    return if past_middle != run.rtl {
                        next_cluster(&self.text, leading)
                    } else {
                        leading
                    };
                }
                x = next;
            }
        }
        line.shaped.range.end
    }

    /// Rectangles covering a byte range, one per line it spans.
    ///
    /// Selection is drawn per line rather than as one shape, because a range
    /// that wraps covers a ragged region that no single rectangle describes.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Bounds<Px>> {
        let mut rects = Vec::new();
        if range.start >= range.end {
            return rects;
        }

        for line in &self.lines {
            let line_range = line.shaped.range.clone();
            let start = range.start.max(line_range.start);
            let end = range.end.min(line_range.end);
            if start >= end {
                continue;
            }

            // Walk the line's glyphs and take the horizontal span of every one
            // whose cluster falls inside the range. Doing it from the glyphs
            // rather than from two caret positions is what makes it correct
            // when a line mixes directions.
            let mut x = line.origin.x;
            let mut left: Option<Px> = None;
            let mut right = line.origin.x;

            for run in &line.shaped.runs {
                for glyph in &run.glyphs {
                    let cluster = glyph.cluster as usize;
                    let next = x + glyph.advance;
                    if cluster >= start && cluster < end {
                        if left.is_none() {
                            left = Some(x);
                        }
                        right = next;
                    }
                    x = next;
                }
            }

            if let Some(left) = left {
                rects.push(Bounds::from_xywh(
                    left,
                    line.origin.y,
                    (right - left).max(Px(1.0)),
                    line.height,
                ));
            } else if end > start {
                // The range covers only characters with no glyphs, such as a
                // trailing space at a wrap point. Show a thin marker so a
                // selection is never invisible.
                rects.push(Bounds::from_xywh(
                    line.origin.x,
                    line.origin.y,
                    Px(2.0),
                    line.height,
                ));
            }
        }
        rects
    }

    /// The word surrounding a byte offset, for double click selection.
    pub fn word_at(&self, index: usize) -> Range<usize> {
        use unicode_segmentation::UnicodeSegmentation;
        let text = self.text.as_str();
        if text.is_empty() {
            return 0..0;
        }
        let index = index.min(text.len().saturating_sub(1));
        for (start, word) in text.split_word_bound_indices() {
            let end = start + word.len();
            if index >= start && index < end {
                return start..end;
            }
        }
        0..text.len()
    }

    /// The line surrounding a byte offset, for triple click selection.
    pub fn line_range_at(&self, index: usize) -> Range<usize> {
        for line in &self.lines {
            let range = line.shaped.range.clone();
            if index >= range.start && index <= range.end {
                return range;
            }
        }
        0..self.text.len()
    }

    /// The rectangle a caret should occupy at `index`.
    pub fn caret(&self, index: usize) -> (Point<Px>, Px) {
        let position = self.position_for_index(index);
        (position, self.line_height)
    }
}

/// Roughly how much one laid out block is holding.
fn laid_out_bytes(laid_out: &TextLayout) -> usize {
    std::mem::size_of::<TextLayout>()
        + laid_out
            .lines
            .iter()
            .map(|line| {
                std::mem::size_of::<LaidOutLine>()
                    + line
                        .shaped
                        .runs
                        .iter()
                        .map(|run| {
                            std::mem::size_of::<crate::shape::ShapedRun>()
                                + run.glyphs.len() * std::mem::size_of::<crate::shape::ShapedGlyph>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}

fn next_cluster(text: &str, from: usize) -> usize {
    if from >= text.len() {
        return text.len();
    }
    let mut index = from + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// How much the laid out block cache aims to hold.
///
/// A count is the wrong bound. One entry is a word and another is a wrapped
/// paragraph of a hundred lines, so four thousand entries is anywhere between
/// a megabyte and forty. Bytes are what has to stay bounded, so bytes are what
/// is counted.
///
/// A target rather than a hard limit, and enforced only at a frame boundary.
/// What the frame being laid out right now has already asked for is never
/// thrown away, however far over this that puts the total. Dropping it would
/// mean laying the same block out again a few calls later, in the same frame,
/// which is not a saving of anything.
const LAYOUT_CACHE_BUDGET: usize = 2 * 1024 * 1024;

/// The point at which even one frame's own working set is too much to keep.
///
/// Reaching this means a single frame laid out tens of thousands of blocks,
/// which is what happens when a very long list is built in full rather than
/// virtualized. Holding all of it would trade an unbounded amount of memory
/// for the speed of a screen nobody can read at once, so past here the cache
/// is dropped wholesale and the frame pays full price.
const LAYOUT_CACHE_CEILING: usize = 32 * 1024 * 1024;

/// Fonts, shaping and rasterization, wired together.
#[derive(Debug, Default)]
pub struct TextSystem {
    pub fonts: FontSystem,
    pub shaper: Shaper,
    pub rasterizer: GlyphRasterizer,
    /// Laid out blocks, keyed by everything that affects the result.
    ///
    /// Laying text out is the single most expensive thing a frame does, and a
    /// frame asks for the same block at least twice: once while measuring for
    /// layout, once while painting. Across frames nothing changes at all for
    /// static text, so this turns the steady state cost of text into a hash
    /// lookup.
    layout_cache: HashMap<(SharedString, TextStyle, LayoutOptions), CacheEntry>,
    /// The same, for blocks carrying spans. Kept apart from the plain cache so
    /// that laying out ordinary text never pays for an empty span table.
    rich_cache: HashMap<(RichText, TextStyle, LayoutOptions), CacheEntry>,
    /// Roughly what the two caches above are holding, tallied as entries go in
    /// rather than measured by walking them, which a frame cannot afford.
    layout_bytes: usize,
    /// Which frame is being laid out, so eviction can tell what is on screen
    /// now from what was on screen a moment ago.
    frame: u64,
    cache_hits: u64,
    cache_misses: u64,
}

/// A laid out block, with what it cost and when it was last wanted.
#[derive(Debug, Clone)]
struct CacheEntry {
    layout: Arc<TextLayout>,
    /// Roughly what this entry adds to `layout_bytes`, remembered so that
    /// evicting it can subtract exactly what inserting it added.
    cost: usize,
    last_used: u64,
}

impl TextSystem {
    pub fn new() -> Self {
        TextSystem {
            fonts: FontSystem::new(),
            shaper: Shaper::new(),
            rasterizer: GlyphRasterizer::new(),
            layout_cache: HashMap::new(),
            rich_cache: HashMap::new(),
            layout_bytes: 0,
            frame: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }

    /// Load the system fonts and get to work.
    pub fn with_system_fonts() -> Self {
        TextSystem {
            fonts: FontSystem::with_system_fonts(),
            ..TextSystem::new()
        }
    }

    /// Layout cache hits and misses since construction.
    pub fn layout_cache_stats(&self) -> (u64, u64) {
        (self.cache_hits, self.cache_misses)
    }

    /// Roughly how much the laid out block cache is holding.
    ///
    /// A running tally rather than a walk, because this is read every frame to
    /// report memory and walking thousands of entries to answer it would cost
    /// more than the cache saves.
    #[inline]
    pub fn layout_cache_bytes(&self) -> usize {
        self.layout_bytes
    }

    /// Everything the text system holds: fonts, shaped runs, laid out blocks.
    pub fn memory_bytes(&self) -> usize {
        self.fonts.memory_bytes() + self.shaper.memory_bytes() + self.layout_cache_bytes()
    }

    pub fn cached_layouts(&self) -> usize {
        self.layout_cache.len() + self.rich_cache.len()
    }

    /// Drop every cached layout. Call after loading fonts.
    pub fn clear_caches(&mut self) {
        self.clear_layout_caches();
        self.shaper.clear_cache();
    }

    /// Drop cached work the frame just finished did not ask for.
    ///
    /// Called once a frame, at the end of one. Both caches are keyed by the
    /// text itself, so a session that shows a lot of different strings, a log
    /// or a counter that ticks, fills them with entries that will never be
    /// asked for again.
    ///
    /// Only entries older than the frame that just ran are dropped. Evicting
    /// anything the current frame has used would be self defeating: the block
    /// is measured several times over while layout settles on a width, so what
    /// was thrown away to make room is asked for again a few calls later, in
    /// the same frame, and laid out from scratch each time. Enforcing the
    /// budget from inside the insert did exactly that, and turned a screenful
    /// of text that no longer fit the budget from a cache that held it into
    /// one that missed on nearly every lookup.
    pub fn trim_caches(&mut self) {
        if self.layout_bytes > LAYOUT_CACHE_BUDGET {
            let frame = self.frame;
            let mut live = 0;
            self.layout_cache.retain(|_, entry| {
                let keep = entry.last_used == frame;
                if keep {
                    live += entry.cost;
                }
                keep
            });
            self.rich_cache.retain(|_, entry| {
                let keep = entry.last_used == frame;
                if keep {
                    live += entry.cost;
                }
                keep
            });
            self.layout_bytes = live;

            // What one frame alone wants is over the ceiling, so keeping it is
            // no longer a bounded amount of memory. Nothing on screen can be
            // read all at once at this size; the list wanting it should be
            // virtualized.
            if self.layout_bytes > LAYOUT_CACHE_CEILING {
                log::warn!(
                    "one frame laid out {} bytes of text, past the {} byte ceiling; virtualize the list that built it",
                    self.layout_bytes,
                    LAYOUT_CACHE_CEILING,
                );
                self.clear_layout_caches();
            }
        }
        self.frame += 1;
        self.shaper.trim_cache();
    }

    /// Metrics for the face a style resolves to, at that style's size.
    pub fn metrics(&mut self, style: &TextStyle) -> ScaledMetrics {
        match self.fonts.resolve(&style.family, style.weight, style.style) {
            Some(id) => self.fonts.face(id).metrics.scaled(style.size),
            None => ScaledMetrics {
                // A plausible stand in so layout still produces sane boxes when
                // no font could be loaded at all.
                ascent: style.size * 0.8,
                descent: style.size * 0.2,
                ..Default::default()
            },
        }
    }

    /// Measure a single line of text without laying it out.
    pub fn measure(&mut self, text: &str, style: &TextStyle) -> Px {
        self.shaper.measure(&mut self.fonts, text, style)
    }

    /// Lay out a block of text, reusing a cached result when there is one.
    pub fn layout(
        &mut self,
        text: &SharedString,
        style: &TextStyle,
        options: &LayoutOptions,
    ) -> Arc<TextLayout> {
        let key = (text.clone(), style.clone(), options.clone());
        let frame = self.frame;
        if let Some(cached) = self.layout_cache.get_mut(&key) {
            // Touched, so the frame boundary knows this one is on screen.
            cached.last_used = frame;
            self.cache_hits += 1;
            return cached.layout.clone();
        }

        self.cache_misses += 1;
        let laid_out = Arc::new(self.layout_uncached(text, style, options));

        let cost = text.len() + laid_out_bytes(&laid_out);
        self.layout_bytes += cost;
        self.layout_cache.insert(
            key,
            CacheEntry {
                layout: laid_out.clone(),
                cost,
                last_used: frame,
            },
        );
        laid_out
    }

    /// Drop both laid out block caches and reset the tally.
    fn clear_layout_caches(&mut self) {
        self.layout_cache.clear();
        self.rich_cache.clear();
        self.layout_bytes = 0;
    }

    /// Lay out a block with styled spans, reusing a cached result when there
    /// is one.
    ///
    /// A block with no spans is handed to the plain path, so wrapping text in
    /// a [`RichText`] costs nothing until it actually carries a span.
    pub fn layout_rich(
        &mut self,
        rich: &RichText,
        style: &TextStyle,
        options: &LayoutOptions,
    ) -> Arc<TextLayout> {
        if rich.is_plain() {
            return self.layout(&rich.text, style, options);
        }

        let key = (rich.clone(), style.clone(), options.clone());
        let frame = self.frame;
        if let Some(cached) = self.rich_cache.get_mut(&key) {
            cached.last_used = frame;
            self.cache_hits += 1;
            return cached.layout.clone();
        }

        self.cache_misses += 1;
        let laid_out = Arc::new(self.layout_spans(&rich.text, &rich.spans, style, options));

        let cost = rich.as_str().len()
            + rich.spans.len() * std::mem::size_of::<TextSpan>()
            + laid_out_bytes(&laid_out);
        self.layout_bytes += cost;
        self.rich_cache.insert(
            key,
            CacheEntry {
                layout: laid_out.clone(),
                cost,
                last_used: frame,
            },
        );
        laid_out
    }

    /// Lay out a string slice. Convenient, but never cached.
    pub fn layout_str(
        &mut self,
        text: &str,
        style: &TextStyle,
        options: &LayoutOptions,
    ) -> TextLayout {
        self.layout_uncached(text, style, options)
    }

    /// Lay out a block of text from scratch.
    pub fn layout_uncached(
        &mut self,
        text: &str,
        style: &TextStyle,
        options: &LayoutOptions,
    ) -> TextLayout {
        self.layout_spans(text, &[], style, options)
    }

    /// Lay out a block from scratch, with spans overriding parts of it.
    ///
    /// The uniform case is this with an empty span table, so there is one line
    /// breaker and one shaper rather than a plain pair and a rich pair that
    /// gradually disagree.
    pub fn layout_spans(
        &mut self,
        text: &str,
        spans: &[TextSpan],
        style: &TextStyle,
        options: &LayoutOptions,
    ) -> TextLayout {
        // Collapsing whitespace moves bytes, and the spans were written against
        // the original string, so they have to move with it. The map that makes
        // that possible is only built when there is something to remap.
        let mut map = Vec::new();
        let source = if spans.is_empty() {
            preprocess(text, options.white_space)
        } else {
            preprocess_with_map(text, options.white_space, Some(&mut map))
        };
        let spans = remap_spans(spans, &map);
        let styles = SpanStyles::new(&mut self.fonts, style, &spans);

        let metrics = self.metrics(style);
        let natural_line_height = metrics.line_height();
        let base_line_height = options
            .line_height
            .resolve(style.size, natural_line_height)
            .max(Px(1.0));

        let rtl = has_rtl(&source);
        let base_level = if rtl && starts_rtl(&source) {
            Level::rtl()
        } else {
            Level::ltr()
        };

        // Break into logical lines, then shape each one.
        let breaks = self.compute_line_breaks(&source, &styles, options);
        let mut truncated = breaks.truncated;

        let mut lines: Vec<LaidOutLine> = Vec::with_capacity(breaks.ranges.len());
        let mut widest = Px::ZERO;

        for (index, range) in breaks.ranges.iter().enumerate() {
            let is_last = index + 1 == breaks.ranges.len();
            let mut slice = source[range.clone()].to_string();

            // Ellipsize the final line when content was dropped, or when a non
            // wrapping line simply does not fit.
            let needs_ellipsis = options.overflow == TextOverflow::Ellipsis
                && is_last
                && (truncated || self.exceeds(&slice, range.start, &styles, options.max_width));
            if needs_ellipsis {
                slice = self.ellipsize(&slice, range.start, &styles, options.max_width);
                truncated = true;
            }

            let shaped =
                self.shaper
                    .shape_line_with(&mut self.fonts, &slice, range.start, &styles, base_level);
            widest = widest.max(shaped.width);

            // A line box is sized by the faces actually on it, with the
            // element's own font as a floor. A line carrying a larger span is
            // taller than its neighbours and sits on a lower baseline, and
            // every glyph on the line then shares that one baseline.
            let (ascent, descent, natural) = self.line_metrics(&shaped, &metrics);
            let height = if matches!(options.line_height, LineHeight::Normal) {
                natural.max(base_line_height)
            } else {
                base_line_height
            };
            let half_leading = ((height - (ascent + descent)) * 0.5).max(Px::ZERO);

            lines.push(LaidOutLine {
                width: shaped.width,
                shaped,
                origin: Point::zero(),
                baseline: half_leading + ascent,
                height,
                ascent,
                descent,
            });
        }

        // Lines are aligned against the constraint, but the block reports its
        // intrinsic width. Layout needs the intrinsic value to size the box;
        // alignment needs the constraint to know where the free space is.
        let align_width = options.max_width.unwrap_or(widest).max(widest);
        let mut y = Px::ZERO;
        for line in &mut lines {
            let free = (align_width - line.width).max(Px::ZERO);
            let x = match resolve_align(options.align, rtl) {
                ResolvedAlign::Left => Px::ZERO,
                ResolvedAlign::Center => free * 0.5,
                ResolvedAlign::Right => free,
            };
            line.origin = Point::new(x, y);
            y += line.height;
        }

        TextLayout {
            text: SharedString::from(source.as_str()),
            lines,
            size: Size::new(widest, y),
            line_height: base_line_height,
            ascent: metrics.ascent,
            descent: metrics.descent,
            rtl,
            truncated,
        }
    }

    /// Vertical metrics for one shaped line, from the faces it actually used.
    ///
    /// The element's own metrics are a floor rather than the answer, so a line
    /// that fell back to a face with a taller ascent gets a box that fits it
    /// instead of clipping.
    fn line_metrics(&self, line: &ShapedLine, base: &ScaledMetrics) -> (Px, Px, Px) {
        let mut ascent = base.ascent;
        let mut descent = base.descent;
        let mut natural = base.line_height();
        for run in &line.runs {
            let Some(face) = self.fonts.try_face(run.font) else {
                continue;
            };
            let metrics = face.metrics.scaled(run.size);
            ascent = ascent.max(metrics.ascent);
            descent = descent.max(metrics.descent);
            natural = natural.max(metrics.line_height());
        }
        (ascent, descent, natural)
    }

    fn exceeds(
        &mut self,
        text: &str,
        offset: usize,
        styles: &SpanStyles<'_>,
        max_width: Option<Px>,
    ) -> bool {
        match max_width {
            Some(limit) => {
                self.shaper
                    .measure_spans(&mut self.fonts, text, offset, styles)
                    > limit
            }
            None => false,
        }
    }

    /// Trim from the end until the text plus an ellipsis fits.
    fn ellipsize(
        &mut self,
        text: &str,
        offset: usize,
        styles: &SpanStyles<'_>,
        max_width: Option<Px>,
    ) -> String {
        let Some(limit) = max_width else {
            return format!("{}{ELLIPSIS}", text.trim_end());
        };

        // The ellipsis itself is drawn in the element's own style, whatever
        // the span it replaces was.
        let ellipsis_width = self.measure(ELLIPSIS, styles.base());
        if ellipsis_width > limit {
            return String::new();
        }

        let budget = limit - ellipsis_width;
        let mut end = text.len();
        // Walk back a grapheme at a time so a truncated emoji or accented
        // letter never gets cut in half.
        let boundaries: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();

        for boundary in boundaries.iter().rev() {
            end = *boundary;
            let kept = text[..end].trim_end();
            if self
                .shaper
                .measure_spans(&mut self.fonts, kept, offset, styles)
                <= budget
            {
                break;
            }
        }

        format!("{}{ELLIPSIS}", text[..end].trim_end())
    }

    fn compute_line_breaks(
        &mut self,
        source: &str,
        styles: &SpanStyles<'_>,
        options: &LayoutOptions,
    ) -> LineBreaks {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        let mut truncated = false;

        // Hard breaks always split, whatever the wrapping mode.
        for paragraph in split_paragraphs(source) {
            let text = &source[paragraph.clone()];
            if !options.white_space.wraps() || options.max_width.is_none() {
                ranges.push(paragraph);
                continue;
            }
            let limit = options.max_width.unwrap();
            self.wrap_paragraph(source, text, paragraph.start, styles, limit, &mut ranges);
        }

        if ranges.is_empty() {
            ranges.push(0..source.len());
        }

        if let Some(max_lines) = options.max_lines {
            if ranges.len() > max_lines.max(1) {
                ranges.truncate(max_lines.max(1));
                truncated = true;
            }
        }

        LineBreaks { ranges, truncated }
    }

    fn wrap_paragraph(
        &mut self,
        source: &str,
        text: &str,
        offset: usize,
        styles: &SpanStyles<'_>,
        limit: Px,
        out: &mut Vec<Range<usize>>,
    ) {
        if text.is_empty() {
            out.push(offset..offset);
            return;
        }

        let mut line_start = 0usize;
        let mut line_width = Px::ZERO;
        // Where the current line would end if we broke here, excluding the
        // trailing whitespace that a break swallows.
        let mut content_end = 0usize;

        for (index, unit) in text.split_word_bound_indices() {
            let is_space = unit.chars().all(char::is_whitespace);
            let width = self
                .shaper
                .measure_spans(&mut self.fonts, unit, offset + index, styles);

            // A break before this unit is possible only once the line has
            // content, otherwise an over-long first word would loop forever.
            if !is_space && line_width + width > limit && content_end > line_start {
                out.push(offset + line_start..offset + content_end);
                line_start = index;
                line_width = Px::ZERO;
                content_end = index;
            }

            // A single unit wider than the whole line has to be split inside.
            if !is_space && width > limit && line_width == Px::ZERO {
                let mut piece_start = index;
                let mut piece_width = Px::ZERO;
                for (grapheme_index, grapheme) in unit.grapheme_indices(true) {
                    let absolute = index + grapheme_index;
                    let grapheme_width = self.shaper.measure_spans(
                        &mut self.fonts,
                        grapheme,
                        offset + absolute,
                        styles,
                    );
                    if piece_width + grapheme_width > limit && absolute > piece_start {
                        out.push(offset + piece_start..offset + absolute);
                        piece_start = absolute;
                        piece_width = Px::ZERO;
                    }
                    piece_width += grapheme_width;
                }
                line_start = piece_start;
                line_width = piece_width;
                content_end = index + unit.len();
                continue;
            }

            line_width += width;
            if !is_space {
                content_end = index + unit.len();
            }
        }

        let tail_end = text.len();
        if line_start < tail_end || out.is_empty() {
            // The last line keeps its trailing whitespace. A break swallows
            // the spaces before it, which is why `content_end` stops at the
            // last non-space, but nothing breaks after the end of the text:
            // those spaces are characters somebody has just typed. Cutting
            // them here leaves the caret unable to move past a space, so a
            // field looks like it is ignoring the key.
            out.push(offset + line_start..offset + tail_end);
        }

        let _ = source;
    }
}

struct LineBreaks {
    ranges: Vec<Range<usize>>,
    truncated: bool,
}

/// Collapse whitespace when the style calls for it.
fn preprocess(text: &str, white_space: WhiteSpace) -> String {
    preprocess_with_map(text, white_space, None)
}

/// The same, optionally recording where every input byte ended up.
///
/// Rich text needs the map because its spans were written against the string
/// the caller supplied, and collapsing whitespace moves bytes out from under
/// them. Plain text passes `None` and pays nothing for it.
fn preprocess_with_map(
    text: &str,
    white_space: WhiteSpace,
    mut map: Option<&mut Vec<u32>>,
) -> String {
    if let Some(map) = map.as_mut() {
        map.clear();
        map.resize(text.len() + 1, 0);
    }

    let mut out = String::with_capacity(text.len());

    if white_space.preserves() {
        let mut chars = text.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if let Some(map) = map.as_mut() {
                for offset in 0..ch.len_utf8() {
                    map[index + offset] = out.len() as u32;
                }
            }
            // A carriage return before a newline disappears into it.
            if ch == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
                continue;
            }
            out.push(ch);
        }
        if let Some(map) = map.as_mut() {
            map[text.len()] = out.len() as u32;
        }
        return out;
    }

    let mut in_whitespace = false;
    for (index, ch) in text.char_indices() {
        let at = out.len() as u32;
        if ch.is_whitespace() {
            if !in_whitespace {
                out.push(' ');
                in_whitespace = true;
            }
        } else {
            out.push(ch);
            in_whitespace = false;
        }
        if let Some(map) = map.as_mut() {
            for offset in 0..ch.len_utf8() {
                map[index + offset] = at;
            }
        }
    }
    if let Some(map) = map.as_mut() {
        map[text.len()] = out.len() as u32;
    }

    // Leading and trailing whitespace has no visual effect once collapsed.
    let trimmed = out.trim();
    if let Some(map) = map.as_mut() {
        let lead = (trimmed.as_ptr() as usize - out.as_ptr() as usize) as u32;
        let len = trimmed.len() as u32;
        for entry in map.iter_mut() {
            *entry = entry.saturating_sub(lead).min(len);
        }
    }
    trimmed.to_string()
}

/// Move span ranges from the caller's text onto the preprocessed text.
///
/// A span whose text collapsed away entirely is dropped rather than kept as an
/// empty range, because an empty span would still split a shaping run.
fn remap_spans(spans: &[TextSpan], map: &[u32]) -> Vec<TextSpan> {
    if spans.is_empty() || map.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let start = *map.get(span.range.start).unwrap_or(&0) as usize;
        let end = *map.get(span.range.end).unwrap_or(&0) as usize;
        if start >= end {
            continue;
        }
        out.push(TextSpan {
            range: start..end,
            style: span.style.clone(),
        });
    }
    out
}

/// Byte ranges between hard line breaks.
fn split_paragraphs(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            out.push(start..index);
            start = index + 1;
        }
    }
    out.push(start..text.len());
    out
}

fn starts_rtl(text: &str) -> bool {
    for ch in text.chars() {
        let code = ch as u32;
        let is_rtl = matches!(code,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF);
        if is_rtl {
            return true;
        }
        if ch.is_alphabetic() {
            return false;
        }
    }
    false
}

enum ResolvedAlign {
    Left,
    Center,
    Right,
}

fn resolve_align(align: TextAlign, rtl: bool) -> ResolvedAlign {
    match align {
        TextAlign::Center => ResolvedAlign::Center,
        TextAlign::Start | TextAlign::Justify => {
            if rtl {
                ResolvedAlign::Right
            } else {
                ResolvedAlign::Left
            }
        }
        TextAlign::End => {
            if rtl {
                ResolvedAlign::Left
            } else {
                ResolvedAlign::Right
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich::{rich, SpanStyle, BASE_SPAN};
    use guirs_style::FontWeight;

    /// A text system with real fonts, or `None` where the machine has none.
    ///
    /// Shaping without a face produces no runs, so the tests that need runs
    /// have nothing to assert on a bare CI image and say so by skipping rather
    /// than by failing.
    fn with_fonts() -> Option<TextSystem> {
        let mut text = TextSystem::with_system_fonts();
        let style = TextStyle::default();
        text.fonts
            .resolve(&style.family, style.weight, style.style)
            .map(|_| text)
    }

    /// Enough distinct blocks that keeping them all is past the byte target.
    ///
    /// Counted from one block's measured cost rather than hard coded, so
    /// raising the budget does not quietly turn these into tests of nothing.
    /// The count is worked out on a throwaway system, because a cache that has
    /// already been driven past its budget is the thing under test.
    fn a_working_set_over_the_budget(text: &mut TextSystem) -> Vec<SharedString> {
        let style = TextStyle::default();
        let options = LayoutOptions::default();
        let block = |index: usize| {
            SharedString::from(format!(
                "Conversation {index} about something or other, long enough to lay out"
            ))
        };

        let mut probe = TextSystem::with_system_fonts();
        probe.layout(&block(0), &style, &options);
        let each = probe.layout_cache_bytes().max(1);
        let count = LAYOUT_CACHE_BUDGET / each + 64;

        let blocks: Vec<SharedString> = (0..count).map(block).collect();
        for block in &blocks {
            text.layout(block, &style, &options);
        }
        blocks
    }

    #[test]
    fn a_screenful_larger_than_the_budget_still_hits_the_cache() {
        // The bug this pins down: the budget used to be enforced from inside
        // the insert, so a frame whose own text did not fit threw away entries
        // it was about to ask for again. Every lookup then missed and the text
        // was laid out from scratch over and over inside the one frame, which
        // took a list that was merely long and made it unusable.
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();
        let blocks = a_working_set_over_the_budget(&mut text);

        // A frame that asks for all of it, then the boundary, then the same
        // frame again. The second pass must be free.
        text.trim_caches();
        for block in &blocks {
            text.layout(block, &style, &options);
        }
        text.trim_caches();

        let (hits, misses) = text.layout_cache_stats();
        for block in &blocks {
            text.layout(block, &style, &options);
        }
        let (hits, misses) = (
            text.layout_cache_stats().0 - hits,
            text.layout_cache_stats().1 - misses,
        );
        assert_eq!(
            misses, 0,
            "{misses} of {} blocks were laid out again despite being on screen the whole time",
            blocks.len()
        );
        assert_eq!(hits as usize, blocks.len());
    }

    #[test]
    fn asking_for_the_same_block_twice_in_a_frame_lays_it_out_once() {
        // Layout measures a block several times over while it settles on a
        // width, so this is the case that made the old policy so expensive.
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();
        let blocks = a_working_set_over_the_budget(&mut text);

        let before = text.layout_cache_stats().1;
        for _ in 0..4 {
            for block in &blocks {
                text.layout(block, &style, &options);
            }
        }
        assert_eq!(
            text.layout_cache_stats().1,
            before,
            "a block on screen was laid out again within the same frame"
        );
    }

    #[test]
    fn text_nothing_asked_for_is_dropped_at_the_frame_boundary() {
        // The other half of the bargain. Keeping the current frame's work is
        // only affordable because everything else goes.
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();
        let blocks = a_working_set_over_the_budget(&mut text);
        text.trim_caches();

        // A frame that wants one block out of all of them.
        text.layout(&blocks[0], &style, &options);
        text.trim_caches();

        assert_eq!(
            text.cached_layouts(),
            1,
            "the cache kept blocks the last frame never asked for"
        );
        assert!(text.layout_cache_bytes() < LAYOUT_CACHE_BUDGET);
    }

    #[test]
    fn a_cache_within_its_budget_is_left_alone() {
        // Trimming is for a cache that has outgrown the target. Below it,
        // nothing is dropped, however old.
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();
        text.layout(&SharedString::from("kept"), &style, &options);
        for _ in 0..10 {
            text.trim_caches();
        }
        assert_eq!(text.cached_layouts(), 1);
        let before = text.layout_cache_stats().1;
        text.layout(&SharedString::from("kept"), &style, &options);
        assert_eq!(text.layout_cache_stats().1, before, "an idle entry was dropped");
    }

    #[test]
    fn spans_move_with_the_bytes_when_whitespace_collapses() {
        let original = "  Rust   is  fast ";
        let mut map = Vec::new();
        let source = preprocess_with_map(original, WhiteSpace::Normal, Some(&mut map));
        assert_eq!(source, "Rust is fast");

        // "fast" sits at 13..17 in the original and at 8..12 once collapsed.
        let moved = remap_spans(
            &[TextSpan {
                range: 13..17,
                style: SpanStyle::bold(),
            }],
            &map,
        );
        assert_eq!(moved.len(), 1);
        assert_eq!(&source[moved[0].range.clone()], "fast");
    }

    #[test]
    fn a_span_that_collapses_away_entirely_is_dropped() {
        let original = "a    b";
        let mut map = Vec::new();
        let source = preprocess_with_map(original, WhiteSpace::Normal, Some(&mut map));
        assert_eq!(source, "a b");

        // The last three spaces of the run become nothing at all.
        let moved = remap_spans(
            &[TextSpan {
                range: 2..5,
                style: SpanStyle::bold(),
            }],
            &map,
        );
        assert!(moved.is_empty());
    }

    #[test]
    fn spans_move_past_a_swallowed_carriage_return() {
        let original = "a\r\nb";
        let mut map = Vec::new();
        let source = preprocess_with_map(original, WhiteSpace::Pre, Some(&mut map));
        assert_eq!(source, "a\nb");

        let moved = remap_spans(
            &[TextSpan {
                range: 3..4,
                style: SpanStyle::bold(),
            }],
            &map,
        );
        assert_eq!(&source[moved[0].range.clone()], "b");
    }

    #[test]
    fn a_map_is_only_built_when_something_needs_remapping() {
        // The plain entry point takes no map at all, and still collapses.
        assert_eq!(preprocess("  a  b  ", WhiteSpace::Normal), "a b");
    }

    #[test]
    fn runs_are_tagged_with_the_span_they_came_from() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let content = rich("plain ").bold("bold").push(" plain").build();
        let laid_out = text.layout_rich(&content, &TextStyle::default(), &LayoutOptions::default());

        let runs = &laid_out.lines[0].shaped.runs;
        assert!(runs.iter().any(|run| run.span == BASE_SPAN));

        let tagged: Vec<_> = runs.iter().filter(|run| run.span == 0).collect();
        assert!(!tagged.is_empty(), "the bold span produced no runs");
        assert_eq!(tagged.first().unwrap().range.start, 6);
        assert_eq!(tagged.last().unwrap().range.end, 10);
    }

    #[test]
    fn a_taller_span_raises_the_line_box_and_lowers_the_baseline() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();

        let plain = text.layout_rich(&RichText::plain("Hello there"), &style, &options);
        let mixed = rich("Hello ")
            .styled("there", SpanStyle::default().with_scale(3.0))
            .build();
        let mixed = text.layout_rich(&mixed, &style, &options);

        assert!(mixed.lines[0].height > plain.lines[0].height);
        assert!(mixed.lines[0].baseline > plain.lines[0].baseline);
        assert!(mixed.lines[0].ascent > plain.lines[0].ascent);
        // Every glyph on the line still shares the one baseline.
        assert_eq!(mixed.lines.len(), 1);
    }

    #[test]
    fn line_breaking_measures_each_span_with_its_own_font() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let prose = "the quick brown fox jumps over the lazy dog";
        let style = TextStyle::default();
        let options = LayoutOptions {
            max_width: Some(Px(200.0)),
            ..Default::default()
        };

        let plain = text.layout_rich(&RichText::plain(prose), &style, &options);
        let enlarged = RichText::builder("")
            .styled(prose, SpanStyle::default().with_scale(2.5))
            .build();
        let enlarged = text.layout_rich(&enlarged, &style, &options);

        assert!(
            enlarged.lines.len() > plain.lines.len(),
            "a larger span should wrap sooner, not later"
        );
    }

    #[test]
    fn a_caret_at_the_end_of_a_line_stays_on_that_line() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let laid_out = text.layout(
            &SharedString::from("first\nsecond"),
            &TextStyle::default(),
            // The default collapses whitespace, which would turn the newline
            // into a space and leave nothing to test.
            &LayoutOptions {
                white_space: WhiteSpace::Pre,
                ..Default::default()
            },
        );
        assert_eq!(laid_out.line_count(), 2);

        let first_line_top = laid_out.lines[0].origin.y;
        let second_line_top = laid_out.lines[1].origin.y;
        assert!(second_line_top > first_line_top);

        // Offset five is the end of "first". It is also, in byte terms, just
        // before the newline that starts the second line, and reporting it as
        // being on the second line makes a caret typed to the end of a line
        // jump down before anything has been typed there.
        assert_eq!(laid_out.position_for_index(5).y, first_line_top);
        // Six is the start of "second", which is on the second line.
        assert_eq!(laid_out.position_for_index(6).y, second_line_top);
        // And the beginning is still the beginning.
        assert_eq!(laid_out.position_for_index(0).y, first_line_top);
    }

    #[test]
    fn a_span_inside_right_to_left_text_stays_on_the_correct_side() {
        let Some(mut text) = with_fonts() else {
            return;
        };

        // Two Hebrew words. A span on the second one splits what would
        // otherwise be a single run, and the pieces have to come out in visual
        // order, which for this text means the later bytes are drawn first.
        let first = "\u{05e9}\u{05dc}\u{05d5}\u{05dd}";
        let second = "\u{05e2}\u{05d5}\u{05dc}\u{05dd}";
        let source = format!("{first} {second}");
        let marked = source.len() - second.len();

        let content = RichText::new(
            source.as_str(),
            vec![TextSpan {
                range: marked..source.len(),
                style: SpanStyle::bold(),
            }],
        );
        let laid_out = text.layout_rich(&content, &TextStyle::default(), &LayoutOptions::default());

        assert!(laid_out.rtl);
        let runs = &laid_out.lines[0].shaped.runs;
        assert!(runs.len() >= 2, "the span should have split the line");

        let spanned = runs
            .iter()
            .position(|run| run.span == 0)
            .expect("the span produced no run");
        let plain = runs
            .iter()
            .position(|run| run.span == BASE_SPAN)
            .expect("the unstyled text produced no run");

        // The spanned word is later in the string and so further left on
        // screen, which puts its run first in visual order.
        assert!(
            spanned < plain,
            "right to left segments came out in logical order"
        );
    }

    #[test]
    fn left_to_right_spans_stay_in_reading_order() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let content = rich("first ").bold("second").build();
        let laid_out = text.layout_rich(&content, &TextStyle::default(), &LayoutOptions::default());

        let runs = &laid_out.lines[0].shaped.runs;
        let mut previous = 0usize;
        for run in runs {
            assert!(
                run.range.start >= previous,
                "runs should advance through the text"
            );
            previous = run.range.start;
        }
    }

    #[test]
    fn a_block_with_no_spans_takes_the_plain_path() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let style = TextStyle::default();
        let options = LayoutOptions::default();

        let plain = text.layout(&SharedString::from("identical"), &style, &options);
        let wrapped = text.layout_rich(&RichText::plain("identical"), &style, &options);

        // Same cache entry, not merely an equal one.
        assert!(Arc::ptr_eq(&plain, &wrapped));
    }

    #[test]
    fn a_weight_only_span_still_splits_the_run() {
        let Some(mut text) = with_fonts() else {
            return;
        };
        let content = RichText::new(
            "aaaa",
            vec![TextSpan {
                range: 2..4,
                style: SpanStyle::default().with_weight(FontWeight::BLACK),
            }],
        );
        let laid_out = text.layout_rich(&content, &TextStyle::default(), &LayoutOptions::default());
        let runs = &laid_out.lines[0].shaped.runs;
        assert!(runs.len() >= 2, "a span boundary has to end a shaping run");
    }

    #[test]
    fn whitespace_collapsing_matches_the_mode() {
        assert_eq!(
            preprocess("  hello   world \n more  ", WhiteSpace::Normal),
            "hello world more"
        );
        assert_eq!(
            preprocess("  hello \n world", WhiteSpace::Pre),
            "  hello \n world"
        );
        assert_eq!(preprocess("a\r\nb", WhiteSpace::Pre), "a\nb");
    }

    #[test]
    fn paragraph_splitting_keeps_empty_lines() {
        assert_eq!(split_paragraphs("a\nb"), vec![0..1, 2..3]);
        assert_eq!(split_paragraphs("a\n\nb"), vec![0..1, 2..2, 3..4]);
        assert_eq!(split_paragraphs(""), vec![0..0]);
        assert_eq!(split_paragraphs("a\n"), vec![0..1, 2..2]);
    }

    #[test]
    fn base_direction_follows_the_first_strong_character() {
        assert!(!starts_rtl("hello \u{05e9}"));
        assert!(starts_rtl("\u{05e9}\u{05dc} hello"));
        // Neutral leading characters do not decide the direction.
        assert!(starts_rtl("  \u{0645}\u{0631}"));
        assert!(!starts_rtl("123 abc"));
    }

    #[test]
    fn alignment_flips_with_base_direction() {
        assert!(matches!(
            resolve_align(TextAlign::Start, false),
            ResolvedAlign::Left
        ));
        assert!(matches!(
            resolve_align(TextAlign::Start, true),
            ResolvedAlign::Right
        ));
        assert!(matches!(
            resolve_align(TextAlign::End, true),
            ResolvedAlign::Left
        ));
        assert!(matches!(
            resolve_align(TextAlign::Center, true),
            ResolvedAlign::Center
        ));
    }

    #[test]
    fn cluster_stepping_respects_char_boundaries() {
        let text = "a\u{00e9}\u{65e5}";
        assert_eq!(next_cluster(text, 0), 1);
        assert_eq!(next_cluster(text, 1), 3);
        assert_eq!(next_cluster(text, 3), 6);
        assert_eq!(next_cluster(text, 6), 6);
    }

    #[test]
    fn layout_without_fonts_still_produces_lines() {
        let mut system = TextSystem::new();
        let layout = system.layout_str(
            "hello world",
            &TextStyle::default(),
            &LayoutOptions::default(),
        );
        assert_eq!(layout.line_count(), 1);
        assert_eq!(layout.text, "hello world");
    }

    #[test]
    fn hard_breaks_split_even_without_wrapping() {
        let mut system = TextSystem::new();
        let layout = system.layout_str(
            "one\ntwo\nthree",
            &TextStyle::default(),
            &LayoutOptions {
                white_space: WhiteSpace::Pre,
                ..Default::default()
            },
        );
        assert_eq!(layout.line_count(), 3);
        assert_eq!(&layout.text[layout.lines[1].range()], "two");
    }

    #[test]
    fn max_lines_truncates_and_reports_it() {
        let mut system = TextSystem::new();
        let layout = system.layout_str(
            "a\nb\nc\nd",
            &TextStyle::default(),
            &LayoutOptions {
                white_space: WhiteSpace::Pre,
                max_lines: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(layout.line_count(), 2);
        assert!(layout.truncated);
    }

    #[test]
    fn word_boundaries_come_from_unicode_segmentation() {
        let layout = TextLayout {
            text: SharedString::from("hello brave world"),
            ..Default::default()
        };
        assert_eq!(layout.word_at(0), 0..5);
        assert_eq!(layout.word_at(3), 0..5);
        // The space between words is its own run.
        assert_eq!(layout.word_at(5), 5..6);
        assert_eq!(layout.word_at(7), 6..11);
    }

    #[test]
    fn word_selection_handles_multi_byte_text() {
        let layout = TextLayout {
            text: SharedString::from("caf\u{00e9} au lait"),
            ..Default::default()
        };
        let word = layout.word_at(2);
        assert_eq!(&layout.text[word.clone()], "caf\u{00e9}");
        assert!(layout.text.is_char_boundary(word.end));
    }

    #[test]
    fn an_empty_word_lookup_does_not_panic() {
        let layout = TextLayout::default();
        assert_eq!(layout.word_at(0), 0..0);
        assert_eq!(layout.word_at(999), 0..0);
    }

    #[test]
    fn an_empty_selection_produces_no_rectangles() {
        let layout = TextLayout::default();
        assert!(layout.selection_rects(0..0).is_empty());
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = 5..3;
        assert!(layout.selection_rects(backwards).is_empty());
    }

    #[test]
    fn an_empty_layout_hit_tests_to_zero() {
        let layout = TextLayout::default();
        assert_eq!(layout.index_for_position(Point::new(Px(50.0), Px(50.0))), 0);
        assert_eq!(layout.position_for_index(10), Point::zero());
        assert_eq!(layout.line_at(Px(100.0)), 0);
    }
}

#[cfg(test)]
mod trailing_space_tests {
    use super::*;

    /// Where the caret sits at the end of a string, in a field that preserves
    /// whitespace.
    fn caret_x(text: &str) -> f32 {
        // Real fonts, or every measurement is zero and the test cannot tell
        // a moving caret from a stuck one.
        let mut system = TextSystem::new();
        system.fonts.load_system_fonts();
        let layout = system.layout_str(
            text,
            &TextStyle::default(),
            &LayoutOptions {
                white_space: WhiteSpace::Pre,
                ..Default::default()
            },
        );
        layout.position_for_index(text.len()).x.0
    }

    /// The caret at the end of a string, in a wrapping field like a text area.
    fn wrapping_caret_x(text: &str) -> f32 {
        let mut system = TextSystem::new();
        system.fonts.load_system_fonts();
        let layout = system.layout_str(
            text,
            &TextStyle::default(),
            &LayoutOptions {
                white_space: WhiteSpace::PreWrap,
                max_width: Some(Px(400.0)),
                ..Default::default()
            },
        );
        layout.position_for_index(text.len()).x.0
    }

    #[test]
    fn the_caret_moves_when_a_space_is_typed_into_a_wrapping_field() {
        // A text area wraps, and a line's trailing whitespace is normally
        // dropped at a wrap point. That must not apply to whitespace somebody
        // is in the middle of typing at the end of the text.
        let none = wrapping_caret_x("ab");
        let one = wrapping_caret_x("ab ");
        let four = wrapping_caret_x("ab    ");
        assert!(one > none, "one space did not move the caret: {none} -> {one}");
        assert!(four > one, "more spaces did not move it: {one} -> {four}");
    }

    #[test]
    fn a_trailing_space_survives_being_laid_out() {
        let mut system = TextSystem::new();
        let layout = system.layout_str(
            "ab  ",
            &TextStyle::default(),
            &LayoutOptions {
                white_space: WhiteSpace::Pre,
                ..Default::default()
            },
        );
        assert_eq!(layout.text.as_str(), "ab  ", "the spaces were dropped");
    }

    #[test]
    fn the_caret_moves_when_a_space_is_typed() {
        // Typing a space at the end of a field has to move the caret, or the
        // field looks like it swallowed the key.
        let none = caret_x("ab");
        let one = caret_x("ab ");
        let two = caret_x("ab  ");
        assert!(one > none, "one space did not move the caret: {none} -> {one}");
        assert!(two > one, "a second space did not move it: {one} -> {two}");
    }
}
