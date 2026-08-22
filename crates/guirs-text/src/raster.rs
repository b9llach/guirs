//! Glyph rasterization.
//!
//! Glyphs are rasterized at device resolution and at one of a small number of
//! horizontal subpixel phases. Quantizing the phase is what makes a glyph cache
//! possible at all: without it, a glyph landing at x = 10.03 and the same glyph
//! at x = 10.04 would be different bitmaps and the cache would never hit. Four
//! phases is the usual compromise, close enough that the eye cannot tell and
//! coarse enough that the cache stays small.

use guirs_core::Px;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};

use crate::font::{FontId, FontSystem};

/// How many horizontal subpixel phases a glyph is rasterized at.
pub const SUBPIXEL_PHASES: u8 = 4;

/// Quantize a horizontal position into a subpixel phase.
#[inline]
pub fn subpixel_phase(x: f32) -> u8 {
    let fraction = x - x.floor();
    ((fraction * SUBPIXEL_PHASES as f32) as u8).min(SUBPIXEL_PHASES - 1)
}

/// Everything that identifies one rasterized glyph bitmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: FontId,
    pub glyph: u16,
    /// Size in device pixels, not logical pixels.
    pub size: Px,
    pub phase: u8,
}

/// What a rasterized glyph's samples mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphContent {
    /// One byte per pixel, a coverage mask to be tinted with the text color.
    Alpha,
    /// Four bytes per pixel of straight RGBA, drawn as is. Color emoji.
    Color,
}

/// A rasterized glyph bitmap.
#[derive(Clone, Debug)]
pub struct RasterizedGlyph {
    pub width: u32,
    pub height: u32,
    /// Offset from the pen position to the left edge of the bitmap.
    pub left: i32,
    /// Offset from the baseline up to the top edge of the bitmap.
    pub top: i32,
    pub content: GlyphContent,
    pub data: Vec<u8>,
}

impl RasterizedGlyph {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Bytes per pixel for this glyph's content.
    #[inline]
    pub fn bytes_per_pixel(&self) -> usize {
        match self.content {
            GlyphContent::Alpha => 1,
            GlyphContent::Color => 4,
        }
    }
}

/// Rasterizes glyphs. Holds swash's scratch buffers so repeated calls do not
/// reallocate.
pub struct GlyphRasterizer {
    ctx: ScaleContext,
}

impl Default for GlyphRasterizer {
    fn default() -> Self {
        GlyphRasterizer::new()
    }
}

impl std::fmt::Debug for GlyphRasterizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GlyphRasterizer")
    }
}

impl GlyphRasterizer {
    pub fn new() -> Self {
        GlyphRasterizer {
            ctx: ScaleContext::new(),
        }
    }

    /// Rasterize one glyph, or `None` when it has no outline (a space, say).
    pub fn rasterize(&mut self, fonts: &FontSystem, key: GlyphKey) -> Option<RasterizedGlyph> {
        let face = fonts.try_face(key.font)?;
        let font_ref = face.font_ref()?;

        let mut scaler = self
            .ctx
            .builder(font_ref)
            .size(key.size.0)
            .hint(true)
            .build();

        let offset = Vector::new(key.phase as f32 / SUBPIXEL_PHASES as f32, 0.0);

        // Sources are tried in order. Color first so emoji come out in color,
        // then plain outlines, which is what almost every glyph uses.
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(offset)
        .render(&mut scaler, key.glyph)?;

        let content = match image.content {
            swash::scale::image::Content::Color => GlyphContent::Color,
            _ => GlyphContent::Alpha,
        };

        let glyph = RasterizedGlyph {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            content,
            data: image.data,
        };

        if glyph.is_empty() {
            return None;
        }

        // A malformed face could report a size that does not match its data.
        // Refusing it here keeps the atlas uploader from reading out of bounds.
        let expected = glyph.width as usize * glyph.height as usize * glyph.bytes_per_pixel();
        if glyph.data.len() < expected {
            return None;
        }

        Some(glyph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subpixel_phases_partition_the_unit_interval() {
        assert_eq!(subpixel_phase(10.0), 0);
        assert_eq!(subpixel_phase(10.2), 0);
        assert_eq!(subpixel_phase(10.26), 1);
        assert_eq!(subpixel_phase(10.5), 2);
        assert_eq!(subpixel_phase(10.75), 3);
        // Never overflows the bucket count, even arbitrarily close to the next
        // integer. Values nearer than f32 can represent round to 11.0 and land
        // back in phase 0, which is also correct.
        assert_eq!(subpixel_phase(10.999), SUBPIXEL_PHASES - 1);
        for step in 0..1000 {
            let x = 10.0 + step as f32 / 1000.0;
            assert!(subpixel_phase(x) < SUBPIXEL_PHASES);
        }
    }

    #[test]
    fn negative_positions_still_land_in_range() {
        for x in [-0.1f32, -1.6, -10.9] {
            let phase = subpixel_phase(x);
            assert!(phase < SUBPIXEL_PHASES, "{x} produced phase {phase}");
        }
    }

    #[test]
    fn glyph_keys_distinguish_size_and_phase() {
        let base = GlyphKey {
            font: FontId(0),
            glyph: 42,
            size: Px(16.0),
            phase: 0,
        };
        let bigger = GlyphKey {
            size: Px(17.0),
            ..base
        };
        let shifted = GlyphKey { phase: 1, ..base };
        assert_ne!(base, bigger);
        assert_ne!(base, shifted);
        assert_eq!(base, GlyphKey { ..base });
    }

    #[test]
    fn rasterizing_without_a_font_returns_nothing() {
        let fonts = FontSystem::new();
        let mut rasterizer = GlyphRasterizer::new();
        assert!(rasterizer
            .rasterize(
                &fonts,
                GlyphKey {
                    font: FontId(0),
                    glyph: 1,
                    size: Px(16.0),
                    phase: 0,
                }
            )
            .is_none());
    }
}
