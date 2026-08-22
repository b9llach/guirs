//! Fonts, shaping and text layout for guirs.
//!
//! The entry point is [`TextSystem`], which owns a font database, a shaping
//! cache and a glyph rasterizer:
//!
//! ```no_run
//! use guirs_core::{px, SharedString};
//! use guirs_text::{LayoutOptions, TextStyle, TextSystem};
//!
//! let mut text = TextSystem::with_system_fonts();
//! let layout = text.layout(
//!     &SharedString::from("Hello, world"),
//!     &TextStyle { size: px(16.0), ..Default::default() },
//!     &LayoutOptions { max_width: Some(px(240.0)), ..Default::default() },
//! );
//! assert!(layout.line_count() >= 1);
//! ```
//!
//! Everything below that is available directly when a caller needs it: shape a
//! single run, rasterize a single glyph, or query face metrics.
//!
//! # Rich text
//!
//! A [`RichText`] is one string plus a sorted list of spans over it, for a
//! paragraph that is partly bold, partly code and partly a link. A span
//! describes only what differs from the text around it:
//!
//! ```no_run
//! use guirs_text::{rich, LayoutOptions, TextStyle, TextSystem};
//!
//! let mut text = TextSystem::with_system_fonts();
//! let body = rich("Rust is ").bold("fast").push(" and ").code("safe").build();
//! let layout = text.layout_rich(&body, &TextStyle::default(), &LayoutOptions::default());
//! ```
//!
//! This is one laid out paragraph rather than several placed side by side,
//! which is the whole point: the line breaker measures each piece with its own
//! font, and a line takes its height and its baseline from the tallest face
//! actually on it. Plain text is the same path with an empty span table.
//!
//! # What is cached
//!
//! Shaping is the most expensive step, so every run is cached by its text,
//! face, size and direction, and every laid out block by everything that
//! affects the result. Both caches are bounded by bytes rather than by entry
//! count, because one entry is a word and another is a wrapped paragraph.
//! [`TextSystem::trim_caches`] enforces that and is meant to be called once a
//! frame.

pub mod font;
pub mod layout;
pub mod raster;
pub mod rich;
pub mod shape;

pub use font::{
    default_fallback_families, default_ui_family, FaceMetrics, FontId, FontSystem, LoadedFace,
    ScaledMetrics,
};
pub use layout::{LaidOutLine, LayoutOptions, TextLayout, TextSystem, ELLIPSIS};
pub use rich::{rich, RichText, RichTextBuilder, SpanStyle, TextSpan, BASE_SPAN};
pub use raster::{
    subpixel_phase, GlyphContent, GlyphKey, GlyphRasterizer, RasterizedGlyph, SUBPIXEL_PHASES,
};
pub use shape::{has_rtl, ShapedGlyph, ShapedLine, ShapedRun, Shaper, SpanStyles, TextStyle};
