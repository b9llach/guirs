//! What the renderer knows how to draw.
//!
//! There are exactly two shapes on the GPU: a rounded box and a textured
//! sprite. Everything the framework paints reduces to one of those, which is
//! what keeps the number of pipelines and state changes at two.

use bytemuck::{Pod, Zeroable};
use guirs_core::{Bounds, BoxShadow, Corners, Edges, Icon, Paint, Point, Px, Rgba};
use guirs_text::GlyphKey;

/// A filled, bordered, rounded rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    pub bounds: Bounds<Px>,
    pub corner_radii: Corners<Px>,
    pub background: Paint,
    pub border_widths: Edges<Px>,
    pub border_color: Rgba,
    /// Multiplied into every channel's alpha.
    pub opacity: f32,
}

impl Default for Quad {
    fn default() -> Self {
        Quad {
            bounds: Bounds::zero(),
            corner_radii: Corners::zero(),
            background: Paint::None,
            border_widths: Edges::zero(),
            border_color: Rgba::TRANSPARENT,
            opacity: 1.0,
        }
    }
}

impl Quad {
    pub fn new(bounds: Bounds<Px>) -> Self {
        Quad {
            bounds,
            ..Default::default()
        }
    }

    /// Whether this quad would put anything on screen.
    pub fn is_visible(&self) -> bool {
        if self.opacity <= 0.001 || self.bounds.is_empty() {
            return false;
        }
        self.background.is_visible()
            || (!self.border_widths.is_zero() && !self.border_color.is_transparent())
    }
}

/// A blurred silhouette of a rounded box, drawn behind or inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// The box casting the shadow, before offset and spread.
    pub bounds: Bounds<Px>,
    pub corner_radii: Corners<Px>,
    pub shadow: BoxShadow,
    pub opacity: f32,
}

impl Shadow {
    /// How far past `bounds` this shadow reaches, used to size the rasterized
    /// area so the blur is not clipped.
    pub fn padding(&self) -> Px {
        self.shadow.extent() + Px(2.0)
    }

    pub fn is_visible(&self) -> bool {
        self.opacity > 0.001 && self.shadow.is_visible() && !self.bounds.is_empty()
    }
}

/// One glyph, placed at a baseline position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub key: GlyphKey,
    /// Pen position: the left edge on the baseline, in logical pixels.
    pub position: Point<Px>,
    /// Tint for alpha glyphs. Color glyphs use only its alpha.
    pub color: Rgba,
}

/// Identifies an image already handed to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub u32);

/// A textured rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Image {
    pub id: ImageId,
    pub bounds: Bounds<Px>,
    pub corner_radii: Corners<Px>,
    /// Multiplied into the sampled color, so this both tints and fades.
    pub tint: Rgba,
    /// The part of the image to draw, as `u0, v0, u1, v1` in the image's own
    /// space, where the whole image is `0, 0, 1, 1`.
    ///
    /// This is how a picture fills a box of a different shape without being
    /// stretched: the box keeps its dimensions and the source is cropped to
    /// match. Ignored by everything else, which draws the whole image.
    pub crop: [f32; 4],
}

impl Image {
    /// The whole of an image, drawn into `bounds`.
    pub fn new(id: ImageId, bounds: Bounds<Px>) -> Self {
        Image {
            id,
            bounds,
            corner_radii: Corners::all(Px::ZERO),
            tint: Rgba::WHITE,
            crop: NO_CROP,
        }
    }
}

/// The whole image, which is what a crop is when nothing has cropped it.
pub const NO_CROP: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A vector icon, filled with a single color.
///
/// Rasterized to a coverage mask at device resolution and cached in the same
/// atlas as glyphs, so an icon costs no more to draw than a letter and stays
/// crisp at any size.
#[derive(Clone, Debug, PartialEq)]
pub struct IconSprite {
    pub icon: Icon,
    pub bounds: Bounds<Px>,
    pub color: Rgba,
}

/// A straight line, used for underlines, strikethrough and separators.
///
/// Lines are rasterized as quads, which keeps the pipeline count down and
/// gives antialiasing for free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underline {
    pub bounds: Bounds<Px>,
    pub color: Rgba,
}

// ---------------------------------------------------------------------------
// GPU instance layouts
// ---------------------------------------------------------------------------

/// Which shape a quad instance describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QuadKind {
    /// Background plus border.
    Box = 0,
    /// A blurred shape drawn outside the box.
    DropShadow = 1,
    /// A blurred shape drawn inside the box.
    InsetShadow = 2,
}

/// How a quad's interior is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum FillKind {
    Solid = 0,
    LinearGradient = 1,
    RadialGradient = 2,
}

/// One instanced rounded box. Mirrors `Quad` in `quad.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct QuadInstance {
    /// x, y, width, height in logical pixels.
    pub bounds: [f32; 4],
    /// Top left, top right, bottom right, bottom left.
    pub radii: [f32; 4],
    /// Top, right, bottom, left.
    pub border: [f32; 4],
    /// Solid background, or the fallback color for a gradient.
    pub background: [f32; 4],
    pub border_color: [f32; 4],
    /// offset x, offset y, blur, spread.
    pub shadow: [f32; 4],
    /// quad kind, fill kind, gradient row, gradient angle in radians.
    pub params: [f32; 4],
    /// padding around the shape, opacity, rounded clip slot, transform slot.
    ///
    /// The two slots are indices into the frame's clip and transform tables,
    /// or `NO_CLIP` and `NO_TRANSFORM`. Carrying indices rather than the
    /// matrices themselves keeps the instance the size it already was, which
    /// matters when a busy frame has tens of thousands of them.
    pub extra: [f32; 4],
}

/// The clip slot an instance carries when nothing rounds it.
pub const NO_CLIP: f32 = -1.0;

/// The transform slot an instance carries when nothing moves it.
pub const NO_TRANSFORM: f32 = -1.0;

/// How many transforms one frame can use.
///
/// A transform belongs to a painted subtree rather than to a primitive, so the
/// count is the number of transformed elements on screen, not the number of
/// boxes they contain. Beyond this a subtree draws where it was laid out.
pub const MAX_TRANSFORMS: usize = 32;

/// How many rounded clips one frame can use.
///
/// Rounded clips come from elements that both have a radius and clip their
/// children, which is a handful in any real interface. Beyond this the corners
/// fall back to the scissor, which squares them.
pub const MAX_ROUNDED_CLIPS: usize = 16;

// A zeroed instance would be clipped by slot zero rather than by nothing,
// because zero is a valid slot. Both defaults say so explicitly.
impl Default for QuadInstance {
    fn default() -> Self {
        QuadInstance {
            bounds: [0.0; 4],
            radii: [0.0; 4],
            border: [0.0; 4],
            background: [0.0; 4],
            border_color: [0.0; 4],
            shadow: [0.0; 4],
            params: [0.0; 4],
            extra: [0.0, 0.0, NO_CLIP, NO_TRANSFORM],
        }
    }
}

impl QuadInstance {
    /// Vertex attribute layout, one `vec4` per location.
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x4,
        1 => Float32x4,
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// Build the instance for a filled, bordered box.
    ///
    /// `gradient_row` is the row of the ramp texture holding this quad's
    /// gradient, or `None` for a solid fill.
    pub fn from_quad(quad: &Quad, gradient: Option<GradientRef>) -> QuadInstance {
        let radii = quad.corner_radii.clamp_to(quad.bounds.size);
        let (fill, row, angle, color) = match (&quad.background, gradient) {
            (Paint::Gradient(g), Some(reference)) => {
                let (kind, angle) = match g.kind {
                    guirs_core::paint::GradientKind::Linear { angle_deg } => {
                        (FillKind::LinearGradient, angle_deg.to_radians())
                    }
                    guirs_core::paint::GradientKind::Radial => (FillKind::RadialGradient, 0.0),
                };
                (kind, reference.row as f32, angle, g.sample(0.5))
            }
            (Paint::Solid(c), _) => (FillKind::Solid, 0.0, 0.0, *c),
            _ => (FillKind::Solid, 0.0, 0.0, Rgba::TRANSPARENT),
        };

        QuadInstance {
            bounds: [
                quad.bounds.origin.x.0,
                quad.bounds.origin.y.0,
                quad.bounds.size.width.0,
                quad.bounds.size.height.0,
            ],
            radii: [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ],
            border: [
                quad.border_widths.top.0,
                quad.border_widths.right.0,
                quad.border_widths.bottom.0,
                quad.border_widths.left.0,
            ],
            background: color.to_array(),
            border_color: quad.border_color.to_array(),
            shadow: [0.0; 4],
            params: [QuadKind::Box as u32 as f32, fill as u32 as f32, row, angle],
            // One pixel of padding gives the antialiasing ramp somewhere to go.
            extra: [1.0, quad.opacity, NO_CLIP, NO_TRANSFORM],
        }
    }

    /// Build the instance for a blurred shadow.
    pub fn from_shadow(shadow: &Shadow) -> QuadInstance {
        let radii = shadow.corner_radii.clamp_to(shadow.bounds.size);
        let kind = if shadow.shadow.inset {
            QuadKind::InsetShadow
        } else {
            QuadKind::DropShadow
        };
        QuadInstance {
            bounds: [
                shadow.bounds.origin.x.0,
                shadow.bounds.origin.y.0,
                shadow.bounds.size.width.0,
                shadow.bounds.size.height.0,
            ],
            radii: [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ],
            border: [0.0; 4],
            background: shadow.shadow.color.to_array(),
            border_color: [0.0; 4],
            shadow: [
                shadow.shadow.offset.x.0,
                shadow.shadow.offset.y.0,
                shadow.shadow.blur.0.max(0.0),
                shadow.shadow.spread.0,
            ],
            params: [kind as u32 as f32, FillKind::Solid as u32 as f32, 0.0, 0.0],
            extra: [shadow.padding().0, shadow.opacity, NO_CLIP, NO_TRANSFORM],
        }
    }
}

/// Where a gradient's ramp lives in the ramp texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GradientRef {
    pub row: u32,
}

/// What a sprite instance samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SpriteKind {
    /// Single channel coverage from the mono atlas, tinted by `color`. Icons
    /// use this too: a filled path is a coverage mask like any glyph.
    AlphaGlyph = 0,
    /// Full color from the color atlas, as emoji use.
    ColorGlyph = 1,
    /// Full color from the image atlas, with optional rounding.
    Image = 2,
}

/// One instanced textured rectangle. Mirrors `Sprite` in `sprite.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SpriteInstance {
    /// x, y, width, height in logical pixels.
    pub bounds: [f32; 4],
    /// Normalized atlas coordinates: u0, v0, u1, v1.
    pub uv: [f32; 4],
    /// Tint, premultiplied into the sample.
    pub color: [f32; 4],
    /// sprite kind, corner radius, opacity, atlas layer.
    pub params: [f32; 4],
    /// Rounded clip slot, transform slot, then two reserved.
    ///
    /// A sprite's other four vectors are all spoken for, so this is its own.
    /// Sixteen bytes per glyph is worth paying to stop text spilling out of a
    /// rounded corner, and to let a subtree be moved without laying it out
    /// again.
    pub extra: [f32; 4],
}

impl Default for SpriteInstance {
    fn default() -> Self {
        SpriteInstance {
            bounds: [0.0; 4],
            uv: [0.0; 4],
            color: [0.0; 4],
            params: [0.0; 4],
            extra: [NO_CLIP, NO_TRANSFORM, 0.0, 0.0],
        }
    }
}

impl SpriteInstance {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x4,
        1 => Float32x4,
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SpriteInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Per frame uniforms shared by both pipelines.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FrameUniforms {
    /// Surface size in logical pixels.
    pub viewport: [f32; 2],
    /// Device pixels per logical pixel.
    pub scale_factor: f32,
    pub _padding: f32,
    /// Mono atlas size in texels, then color atlas size in texels.
    pub atlas_sizes: [f32; 4],
    /// Rounded clips, two entries each: bounds, then corner radii.
    pub clips: [[f32; 4]; MAX_ROUNDED_CLIPS * 2],
    /// Transforms, two entries each: the matrix, then the translation and the
    /// scale the antialiasing ramp has to be widened by.
    pub transforms: [[f32; 4]; MAX_TRANSFORMS * 2],
}

impl Default for FrameUniforms {
    fn default() -> Self {
        FrameUniforms {
            viewport: [0.0, 0.0],
            scale_factor: 1.0,
            _padding: 0.0,
            atlas_sizes: [0.0; 4],
            clips: [[0.0; 4]; MAX_ROUNDED_CLIPS * 2],
            transforms: [[0.0; 4]; MAX_TRANSFORMS * 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs_core::{Gradient, GradientStop, Size};

    fn bounds() -> Bounds<Px> {
        Bounds::from_xywh(Px(10.0), Px(20.0), Px(100.0), Px(40.0))
    }

    #[test]
    fn instance_layouts_are_the_size_the_shader_expects() {
        // The clip slot rides in space the instance already had, so neither
        // layout grew when rounded clipping arrived.
        assert_eq!(std::mem::size_of::<QuadInstance>(), 128);
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 80);
        // The uniform block has to satisfy the 16 byte alignment rule.
        assert_eq!(std::mem::size_of::<FrameUniforms>() % 16, 0);
        // Small enough that it is never the reason a frame is slow, and well
        // inside the 64KB every backend guarantees for a uniform block.
        assert!(std::mem::size_of::<FrameUniforms>() < 2048);
    }

    #[test]
    fn zero_is_a_real_slot_and_so_cannot_mean_none() {
        // This is the whole reason the sentinels exist, and the reason every
        // instance is built from `Default` rather than writing its own `extra`.
        //
        // A slot left at zero does not mean "no transform", it means "the
        // first transform", and on a frame that has no transforms that entry
        // is a matrix of zeros. Every vertex then maps to the same point and
        // the shape collapses to nothing. It cost an afternoon of a window
        // that drew every box and not one glyph.
        // Checked at compile time, because these are constants and the
        // point is that they can never drift to zero.
        const _: () = assert!(NO_CLIP < 0.0);
        const _: () = assert!(NO_TRANSFORM < 0.0);
        assert_ne!(NO_CLIP, 0.0);
        assert_ne!(NO_TRANSFORM, 0.0);
    }

    #[test]
    fn a_fresh_instance_is_clipped_by_nothing() {
        // Zero is a valid clip slot, so an instance nobody stamped has to say
        // so rather than being left zeroed.
        assert_eq!(
            QuadInstance::from_quad(&Quad::new(bounds()), None).extra[2],
            NO_CLIP
        );
        assert_eq!(QuadInstance::default().extra[2], NO_CLIP);
        assert_eq!(QuadInstance::default().extra[3], NO_TRANSFORM);
        assert_eq!(SpriteInstance::default().extra[0], NO_CLIP);
        assert_eq!(SpriteInstance::default().extra[1], NO_TRANSFORM);
        let shadow = Shadow {
            bounds: bounds(),
            corner_radii: Corners::all(Px(4.0)),
            shadow: guirs_core::BoxShadow::default(),
            opacity: 1.0,
        };
        assert_eq!(QuadInstance::from_shadow(&shadow).extra[2], NO_CLIP);
    }

    #[test]
    fn an_empty_quad_is_not_visible() {
        assert!(!Quad::new(bounds()).is_visible());
        assert!(!Quad {
            background: Paint::Solid(Rgba::BLACK),
            opacity: 0.0,
            ..Quad::new(bounds())
        }
        .is_visible());
        assert!(!Quad {
            background: Paint::Solid(Rgba::BLACK),
            ..Quad::new(Bounds::zero())
        }
        .is_visible());
    }

    #[test]
    fn a_filled_or_bordered_quad_is_visible() {
        assert!(Quad {
            background: Paint::Solid(Rgba::BLACK),
            ..Quad::new(bounds())
        }
        .is_visible());
        assert!(Quad {
            border_widths: Edges::all(Px(1.0)),
            border_color: Rgba::WHITE,
            ..Quad::new(bounds())
        }
        .is_visible());
    }

    #[test]
    fn corner_radii_are_clamped_when_building_the_instance() {
        let quad = Quad {
            corner_radii: Corners::all(Px(999.0)),
            background: Paint::Solid(Rgba::BLACK),
            ..Quad::new(Bounds::new(
                Point::zero(),
                Size::new(Px(40.0), Px(20.0)),
            ))
        };
        let instance = QuadInstance::from_quad(&quad, None);
        // Half the shorter side.
        assert_eq!(instance.radii, [10.0, 10.0, 10.0, 10.0]);
    }

    #[test]
    fn gradients_encode_their_ramp_row_and_angle() {
        let quad = Quad {
            background: Paint::Gradient(std::sync::Arc::new(Gradient::linear(
                90.0,
                vec![
                    GradientStop::new(0.0, Rgba::BLACK),
                    GradientStop::new(1.0, Rgba::WHITE),
                ],
            ))),
            ..Quad::new(bounds())
        };
        let instance = QuadInstance::from_quad(&quad, Some(GradientRef { row: 3 }));
        assert_eq!(instance.params[1], FillKind::LinearGradient as u32 as f32);
        assert_eq!(instance.params[2], 3.0);
        assert!((instance.params[3] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn a_gradient_with_no_ramp_falls_back_to_a_solid() {
        let quad = Quad {
            background: Paint::Gradient(std::sync::Arc::new(Gradient::vertical(
                Rgba::BLACK,
                Rgba::WHITE,
            ))),
            ..Quad::new(bounds())
        };
        let instance = QuadInstance::from_quad(&quad, None);
        assert_eq!(instance.params[1], FillKind::Solid as u32 as f32);
    }

    #[test]
    fn shadow_padding_grows_with_blur_and_spread() {
        let small = Shadow {
            bounds: bounds(),
            corner_radii: Corners::zero(),
            shadow: BoxShadow::new(Point::zero(), Px(2.0), Px(0.0), Rgba::BLACK),
            opacity: 1.0,
        };
        let large = Shadow {
            shadow: BoxShadow::new(Point::zero(), Px(40.0), Px(8.0), Rgba::BLACK),
            ..small
        };
        assert!(large.padding() > small.padding());
        assert!(small.padding() > Px::ZERO);
    }

    #[test]
    fn inset_shadows_encode_a_distinct_kind() {
        let base = Shadow {
            bounds: bounds(),
            corner_radii: Corners::zero(),
            shadow: BoxShadow::new(Point::zero(), Px(4.0), Px(0.0), Rgba::BLACK),
            opacity: 1.0,
        };
        let outer = QuadInstance::from_shadow(&base);
        let inner = QuadInstance::from_shadow(&Shadow {
            shadow: base.shadow.inset(),
            ..base
        });
        assert_eq!(outer.params[0], QuadKind::DropShadow as u32 as f32);
        assert_eq!(inner.params[0], QuadKind::InsetShadow as u32 as f32);
    }
}
