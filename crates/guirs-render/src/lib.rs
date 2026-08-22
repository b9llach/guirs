//! The guirs GPU renderer.
//!
//! A frame is described by a [`Scene`], a flat list of rounded boxes, shadows,
//! glyphs and images in painting order. [`Renderer`] turns that into instance
//! buffers and draws it with two pipelines.
//!
//! Nothing here knows about elements, layout or styling. The renderer's whole
//! vocabulary is the handful of primitives in [`primitives`], which is what
//! makes it replaceable.
//!
//! # Two pipelines
//!
//! One draws rounded boxes, the other draws sprites. Borders, per corner
//! radii, gradients and Gaussian shadows are all evaluated analytically in the
//! box shader from a signed distance field, so there is no tessellation and
//! antialiasing is exact at any scale factor. Glyphs, icons and images come
//! from array atlases, so a line of text with an inline emoji is still one
//! draw call.
//!
//! # Clips and transforms
//!
//! A [`Scene`] carries a clip stack and a transform stack. Clips are recorded
//! in screen space, so the scissor rectangle and hit testing agree without
//! either having to know about the other.
//!
//! A scissor rectangle cannot describe a corner, so a rounded clip is finished
//! in the fragment shader from the same distance field the shapes use. Both
//! clips and transforms are per frame tables that instances index into rather
//! than copies on every instance, which is why neither instance layout grew to
//! hold them. The tables are bounded: past [`MAX_ROUNDED_CLIPS`] a clip keeps
//! its scissor and loses its corners, and past [`MAX_TRANSFORMS`] a subtree
//! draws where it was laid out.
//!
//! # Memory
//!
//! Atlases start at a single page and grow only when the packer spills, and the
//! renderer asks for one graphics backend rather than every backend, which on a
//! machine with more than one graphics card is the difference between tens of
//! megabytes of driver and hundreds. [`Renderer::memory`] reports what is
//! actually held.

pub mod atlas;
pub mod primitives;
pub mod renderer;
pub mod scene;

pub use atlas::{Atlas, AtlasSlot, PageAllocator};
pub use primitives::{
    FillKind, Glyph, GradientRef, IconSprite, Image, ImageId, Quad, QuadInstance, QuadKind, Shadow,
    NO_CROP,
    SpriteInstance, SpriteKind, Underline, MAX_ROUNDED_CLIPS, MAX_TRANSFORMS,
};
pub use renderer::{
    FrameOutcome, GraphicsBackend, RenderTimings, Renderer, RendererError, RendererMemory,
    ATLAS_LAYERS, ATLAS_SIZE, IMAGE_ATLAS_SIZE,
};
pub use scene::{glyph_bounds, Batch, BatchKind, QuadItem, Scene, SceneStats, SpriteItem};
