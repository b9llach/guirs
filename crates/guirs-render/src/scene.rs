//! The list of things to draw this frame.
//!
//! A scene is rebuilt from scratch every frame, which sounds wasteful and is
//! not: the vectors keep their capacity, so steady state costs no allocation at
//! all, and having no retained draw state removes an entire class of bug where
//! the screen disagrees with the model.
//!
//! Primitives are recorded in painting order. Consecutive primitives of the
//! same kind under the same clip are merged into one batch, so a typical
//! interface collapses to a few dozen draw calls no matter how many elements it
//! contains.

use std::ops::Range;

use guirs_core::{Affine, Bounds, Corners, Point, Px, Rgba};

use crate::primitives::{Glyph, IconSprite, Image, Quad, Shadow, Underline};

/// Which pipeline draws a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchKind {
    Quad,
    Sprite,
}

/// A run of primitives that can be drawn with one call.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub kind: BatchKind,
    /// Indices into the scene's quad or sprite list, depending on `kind`.
    pub range: Range<u32>,
    /// Scissor rectangle in logical pixels. `None` means the whole surface.
    pub clip: Option<Bounds<Px>>,
    /// The rounded clip in force, if the innermost clip has corners.
    ///
    /// The scissor handles the straight edges, which are the bulk of it. This
    /// is what the shader needs in order to take the corners off as well.
    pub rounded_clip: Option<RoundedClip>,
    /// The transform in force, if it is not the identity.
    pub transform: Option<Affine>,
    /// Higher layers draw over lower ones regardless of tree order.
    pub layer: u8,
}

/// A clip with corners, evaluated in the fragment shader.
///
/// A scissor rectangle cannot describe a rounded corner, so a scroll view
/// inside a rounded card used to spill its content into the corner. This is the
/// shape the shader takes coverage from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedClip {
    pub bounds: Bounds<Px>,
    pub radii: Corners<Px>,
}

impl RoundedClip {
    /// Whether this actually rounds anything.
    #[inline]
    pub fn is_rounded(&self) -> bool {
        self.radii.top_left > Px::ZERO
            || self.radii.top_right > Px::ZERO
            || self.radii.bottom_right > Px::ZERO
            || self.radii.bottom_left > Px::ZERO
    }
}

/// One entry on the clip stack.
#[derive(Clone, Copy, Debug)]
struct ClipEntry {
    /// The rectangular intersection of everything pushed so far.
    bounds: Bounds<Px>,
    /// The innermost rounded clip in force, which may have been pushed by an
    /// ancestor rather than by this entry.
    rounded: Option<RoundedClip>,
}

/// A quad shaped primitive, before gradient ramps are resolved.
#[derive(Clone, Debug)]
pub enum QuadItem {
    Box(Quad),
    Shadow(Shadow),
}

/// A textured primitive, before atlas slots are resolved.
#[derive(Clone, Debug)]
pub enum SpriteItem {
    Glyph(Glyph),
    Image(Image),
    Icon(IconSprite),
}

/// Counts for diagnostics and for the frame overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub quads: usize,
    pub sprites: usize,
    pub batches: usize,
    /// Primitives skipped because they fell outside the clip.
    pub culled: usize,
}

/// Everything to draw for one frame.
#[derive(Debug, Default)]
pub struct Scene {
    quads: Vec<QuadItem>,
    sprites: Vec<SpriteItem>,
    batches: Vec<Batch>,
    clips: Vec<ClipEntry>,
    /// Clip stacks belonging to enclosing layers, parked while an overlay is
    /// open. An overlay is not clipped by whatever contains it in the tree.
    parked_clips: Vec<Vec<ClipEntry>>,
    /// Transforms in force, already composed with their ancestors so the top
    /// of the stack is the one to apply.
    transforms: Vec<Affine>,
    layer: u8,
    /// Color the surface is cleared to before anything is drawn.
    pub background: Rgba,
    culled: usize,
}

impl Scene {
    pub fn new() -> Self {
        Scene::default()
    }

    /// Empty the scene while keeping every allocation.
    pub fn clear(&mut self) {
        self.quads.clear();
        self.sprites.clear();
        self.batches.clear();
        self.clips.clear();
        self.parked_clips.clear();
        self.transforms.clear();
        self.layer = 0;
        self.culled = 0;
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() && self.sprites.is_empty()
    }

    #[inline]
    pub fn quads(&self) -> &[QuadItem] {
        &self.quads
    }

    #[inline]
    pub fn sprites(&self) -> &[SpriteItem] {
        &self.sprites
    }

    #[inline]
    pub fn batches(&self) -> &[Batch] {
        &self.batches
    }

    pub fn stats(&self) -> SceneStats {
        SceneStats {
            quads: self.quads.len(),
            sprites: self.sprites.len(),
            batches: self.batches.len(),
            culled: self.culled,
        }
    }

    /// The rectangular clip currently in force, if any.
    #[inline]
    pub fn clip(&self) -> Option<Bounds<Px>> {
        self.clips.last().map(|entry| entry.bounds)
    }

    /// The rounded clip currently in force, if the innermost one has corners.
    #[inline]
    pub fn rounded_clip(&self) -> Option<RoundedClip> {
        self.clips.last().and_then(|entry| entry.rounded)
    }

    /// The transform currently in force, if it is not the identity.
    #[inline]
    pub fn transform(&self) -> Option<Affine> {
        self.transforms.last().copied()
    }

    /// Move everything drawn until the matching pop.
    ///
    /// Composed with whatever is already in force, so a transformed element
    /// inside a transformed one moves with both. Transforms apply at paint
    /// time and change nothing about layout, which is what makes them cheap
    /// enough to animate every frame.
    pub fn push_transform(&mut self, transform: Affine) {
        let composed = match self.transforms.last() {
            Some(current) => transform.then(current),
            None => transform,
        };
        self.transforms.push(composed);
    }

    pub fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    /// Run `f` with an additional transform in force.
    pub fn with_transform<R>(&mut self, transform: Affine, f: impl FnOnce(&mut Scene) -> R) -> R {
        self.push_transform(transform);
        let result = f(self);
        self.pop_transform();
        result
    }

    /// The layer primitives are currently being recorded into.
    #[inline]
    pub fn layer(&self) -> u8 {
        self.layer
    }

    /// Begin drawing into a higher layer.
    ///
    /// An overlay escapes both the painting order and the clip of whatever
    /// contains it, which is what a dropdown or a tooltip needs: it is a child
    /// in the tree but it belongs on top of the window.
    pub fn push_layer(&mut self, layer: u8) {
        self.parked_clips.push(std::mem::take(&mut self.clips));
        self.layer = self.layer.max(layer);
    }

    pub fn pop_layer(&mut self) {
        if let Some(clips) = self.parked_clips.pop() {
            self.clips = clips;
        }
        self.layer = self.parked_clips.len() as u8;
    }

    /// Batches in the order they should be drawn: by layer, then by the order
    /// they were recorded. The sort is stable, so painting order is preserved
    /// within a layer.
    pub fn ordered_batches(&self) -> Vec<&Batch> {
        let mut ordered: Vec<&Batch> = self.batches.iter().collect();
        ordered.sort_by_key(|batch| batch.layer);
        ordered
    }

    /// Narrow the clip to the intersection of the current one and `bounds`.
    pub fn push_clip(&mut self, bounds: Bounds<Px>) {
        self.push_clip_entry(bounds, Corners::all(Px::ZERO));
    }

    /// Narrow the clip to a rounded rectangle.
    ///
    /// The straight edges intersect with whatever is already in force, the
    /// same as a plain clip. The corners replace any rounding an ancestor had:
    /// the intersection of two rounded rectangles is not a rounded rectangle,
    /// and the innermost one is the one a reader notices. Nesting two of them
    /// leaves the outer corners to the scissor, which squares them off.
    pub fn push_rounded_clip(&mut self, bounds: Bounds<Px>, radii: Corners<Px>) {
        self.push_clip_entry(bounds, radii);
    }

    fn push_clip_entry(&mut self, bounds: Bounds<Px>, radii: Corners<Px>) {
        // Clips are kept in screen space, because that is where the scissor
        // rectangle and hit testing both work. A clip pushed inside a
        // transform therefore moves with it here, once, rather than every
        // consumer having to remember to.
        //
        // Under a rotation the result is the box around the turned rectangle,
        // since a scissor cannot be anything else. Translation and uniform
        // scale, which is what an animation actually uses, stay exact.
        let (bounds, radii) = match self.transforms.last() {
            Some(transform) => (
                transform.transform_bounds(bounds),
                scale_radii(radii, transform.average_scale()),
            ),
            None => (bounds, radii),
        };

        let intersected = match self.clips.last() {
            Some(current) => current.bounds.intersect(&bounds),
            None => bounds,
        };
        let candidate = RoundedClip { bounds, radii };
        let rounded = if candidate.is_rounded() {
            Some(candidate)
        } else {
            self.clips.last().and_then(|entry| entry.rounded)
        };
        self.clips.push(ClipEntry {
            bounds: intersected,
            rounded,
        });
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// Run `f` with an additional clip in force.
    pub fn with_clip<R>(&mut self, bounds: Bounds<Px>, f: impl FnOnce(&mut Scene) -> R) -> R {
        self.push_clip(bounds);
        let result = f(self);
        self.pop_clip();
        result
    }

    /// Run `f` with an additional rounded clip in force.
    pub fn with_rounded_clip<R>(
        &mut self,
        bounds: Bounds<Px>,
        radii: Corners<Px>,
        f: impl FnOnce(&mut Scene) -> R,
    ) -> R {
        self.push_rounded_clip(bounds, radii);
        let result = f(self);
        self.pop_clip();
        result
    }

    pub fn push_quad(&mut self, quad: Quad) {
        if !quad.is_visible() {
            return;
        }
        if self.is_culled(&quad.bounds) {
            self.culled += 1;
            return;
        }
        self.push_quad_item(QuadItem::Box(quad));
    }

    pub fn push_shadow(&mut self, shadow: Shadow) {
        if !shadow.is_visible() {
            return;
        }
        // A shadow paints outside its box, so cull against the padded area.
        let reach = shadow.bounds.expand(shadow.padding());
        if self.is_culled(&reach) {
            self.culled += 1;
            return;
        }
        self.push_quad_item(QuadItem::Shadow(shadow));
    }

    /// Draw an underline or strikethrough. Lines are quads, which gives them
    /// antialiasing and lets them join the surrounding batch.
    pub fn push_underline(&mut self, underline: Underline) {
        self.push_quad(Quad {
            bounds: underline.bounds,
            background: guirs_core::Paint::Solid(underline.color),
            ..Quad::default()
        });
    }

    pub fn push_glyph(&mut self, glyph: Glyph) {
        if glyph.color.is_transparent() {
            return;
        }
        self.push_sprite_item(SpriteItem::Glyph(glyph));
    }

    pub fn push_icon(&mut self, icon: IconSprite) {
        if icon.color.is_transparent() || icon.bounds.is_empty() {
            return;
        }
        if self.is_culled(&icon.bounds) {
            self.culled += 1;
            return;
        }
        self.push_sprite_item(SpriteItem::Icon(icon));
    }

    pub fn push_image(&mut self, image: Image) {
        if image.tint.is_transparent() || image.bounds.is_empty() {
            return;
        }
        if self.is_culled(&image.bounds) {
            self.culled += 1;
            return;
        }
        self.push_sprite_item(SpriteItem::Image(image));
    }

    fn is_culled(&self, bounds: &Bounds<Px>) -> bool {
        // A transformed primitive can be moved anywhere, including back into
        // view from outside it, so culling has to see where it will land.
        if let Some(transform) = self.transforms.last() {
            let moved = transform.transform_bounds(*bounds);
            return match self.clips.last() {
                Some(entry) => entry.bounds.is_empty() || !entry.bounds.intersects(&moved),
                None => false,
            };
        }
        match self.clips.last() {
            // Culling stays rectangular on purpose. It is a conservative test,
            // and a primitive that only the corners would have removed is
            // handled exactly by the shader instead.
            Some(entry) => entry.bounds.is_empty() || !entry.bounds.intersects(bounds),
            None => false,
        }
    }

    fn push_quad_item(&mut self, item: QuadItem) {
        let index = self.quads.len() as u32;
        self.quads.push(item);
        self.extend_batch(BatchKind::Quad, index);
    }

    fn push_sprite_item(&mut self, item: SpriteItem) {
        let index = self.sprites.len() as u32;
        self.sprites.push(item);
        self.extend_batch(BatchKind::Sprite, index);
    }

    fn extend_batch(&mut self, kind: BatchKind, index: u32) {
        let clip = self.clip();
        let rounded_clip = self.rounded_clip();
        let transform = self.transform();
        let layer = self.layer;
        if let Some(last) = self.batches.last_mut() {
            if last.kind == kind
                && last.clip == clip
                && last.rounded_clip == rounded_clip
                && last.transform == transform
                && last.layer == layer
                && last.range.end == index
            {
                last.range.end = index + 1;
                return;
            }
        }
        self.batches.push(Batch {
            kind,
            range: index..index + 1,
            clip,
            rounded_clip,
            transform,
            layer,
        });
    }
}

fn scale_radii(radii: Corners<Px>, factor: f32) -> Corners<Px> {
    if (factor - 1.0).abs() < 1e-6 {
        return radii;
    }
    Corners {
        top_left: radii.top_left * factor,
        top_right: radii.top_right * factor,
        bottom_right: radii.bottom_right * factor,
        bottom_left: radii.bottom_left * factor,
    }
}

/// A glyph's destination rectangle, given its rasterized placement.
///
/// The rasterizer works in device pixels while layout works in logical ones, so
/// this is where the two meet. Snapping the pen position to the physical grid
/// before adding the bitmap offset is what keeps text from shimmering as a
/// window moves between displays with different scale factors.
pub fn glyph_bounds(
    pen: Point<Px>,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    scale: f32,
) -> Bounds<Px> {
    let device_x = (pen.x.0 * scale).floor() + left as f32;
    let device_y = (pen.y.0 * scale).round() - top as f32;
    Bounds::from_xywh(
        Px(device_x / scale),
        Px(device_y / scale),
        Px(width as f32 / scale),
        Px(height as f32 / scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs_core::{BoxShadow, Corners, Paint};

    fn filled(x: f32, y: f32, w: f32, h: f32) -> Quad {
        Quad {
            bounds: Bounds::from_xywh(Px(x), Px(y), Px(w), Px(h)),
            background: Paint::Solid(Rgba::BLACK),
            ..Quad::default()
        }
    }

    fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> (Bounds<Px>, Corners<Px>) {
        (
            Bounds::from_xywh(Px(x), Px(y), Px(w), Px(h)),
            Corners::all(Px(r)),
        )
    }

    #[test]
    fn a_transform_reaches_the_batch_that_needs_it() {
        let mut scene = Scene::new();
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        let moved = Affine::translate(30.0, 0.0);
        scene.push_transform(moved);
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));
        scene.pop_transform();

        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        let batches = scene.batches();
        assert_eq!(batches.len(), 3);
        assert!(batches[0].transform.is_none());
        assert_eq!(batches[1].transform, Some(moved));
        assert!(batches[2].transform.is_none());
    }

    #[test]
    fn nested_transforms_compose_outwards() {
        let mut scene = Scene::new();
        scene.push_transform(Affine::translate(10.0, 0.0));
        scene.push_transform(Affine::translate(5.0, 0.0));
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        let composed = scene.batches()[0].transform.expect("no transform");
        // Both translations, not just the inner one.
        assert_eq!(
            composed.apply(Point::new(Px(0.0), Px(0.0))),
            Point::new(Px(15.0), Px(0.0))
        );
    }

    #[test]
    fn culling_sees_where_a_primitive_will_land() {
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(100.0), Px(0.0), Px(100.0), Px(100.0)));

        // Laid out well outside the clip, but moved into it.
        scene.push_transform(Affine::translate(120.0, 0.0));
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));
        scene.pop_transform();
        assert_eq!(scene.stats().quads, 1, "a primitive moved into view was culled");

        // And one moved out of it is still dropped.
        scene.push_transform(Affine::translate(-500.0, 0.0));
        scene.push_quad(filled(120.0, 0.0, 10.0, 10.0));
        scene.pop_transform();
        assert_eq!(scene.stats().quads, 1);
    }

    #[test]
    fn a_clip_pushed_inside_a_transform_is_recorded_where_it_lands() {
        let mut scene = Scene::new();
        scene.push_transform(Affine::translate(40.0, 0.0));
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0)));
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        // The scissor and hit testing both work in screen space, so the clip
        // has to have moved with the transform rather than staying put.
        assert_eq!(
            scene.batches()[0].clip,
            Some(Bounds::from_xywh(Px(40.0), Px(0.0), Px(50.0), Px(50.0)))
        );
    }

    #[test]
    fn a_rounded_clip_inside_a_scale_grows_its_corners_too() {
        let mut scene = Scene::new();
        scene.push_transform(Affine::scale(2.0, 2.0));
        scene.push_rounded_clip(
            Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0)),
            Corners::all(Px(5.0)),
        );
        scene.push_quad(filled(1.0, 1.0, 4.0, 4.0));

        let rounded = scene.batches()[0].rounded_clip.expect("no rounded clip");
        assert_eq!(rounded.bounds.size.width, Px(100.0));
        assert_eq!(rounded.radii.top_left, Px(10.0));
    }

    #[test]
    fn a_rounded_clip_reaches_the_batch_that_needs_it() {
        let mut scene = Scene::new();
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        let (bounds, radii) = rounded(0.0, 0.0, 100.0, 100.0, 12.0);
        scene.push_rounded_clip(bounds, radii);
        scene.push_quad(filled(1.0, 1.0, 5.0, 5.0));
        scene.pop_clip();

        scene.push_quad(filled(2.0, 2.0, 5.0, 5.0));

        let batches = scene.batches();
        assert_eq!(batches.len(), 3);
        assert!(batches[0].rounded_clip.is_none());
        assert_eq!(
            batches[1].rounded_clip,
            Some(RoundedClip { bounds, radii })
        );
        // Popping puts it back, rather than leaving it in force.
        assert!(batches[2].rounded_clip.is_none());
    }

    #[test]
    fn a_square_clip_inside_a_rounded_one_keeps_the_corners() {
        let mut scene = Scene::new();
        let (bounds, radii) = rounded(0.0, 0.0, 100.0, 100.0, 12.0);
        scene.push_rounded_clip(bounds, radii);
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(100.0)));
        scene.push_quad(filled(1.0, 1.0, 5.0, 5.0));

        let batch = &scene.batches()[0];
        // The rectangle narrowed the scissor and left the rounding alone.
        assert_eq!(batch.clip, Some(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(100.0))));
        assert_eq!(batch.rounded_clip, Some(RoundedClip { bounds, radii }));
    }

    #[test]
    fn the_innermost_rounded_clip_is_the_one_that_applies() {
        let mut scene = Scene::new();
        let (outer, outer_radii) = rounded(0.0, 0.0, 100.0, 100.0, 20.0);
        let (inner, inner_radii) = rounded(10.0, 10.0, 40.0, 40.0, 4.0);
        scene.push_rounded_clip(outer, outer_radii);
        scene.push_rounded_clip(inner, inner_radii);
        scene.push_quad(filled(12.0, 12.0, 5.0, 5.0));

        assert_eq!(
            scene.batches()[0].rounded_clip,
            Some(RoundedClip {
                bounds: inner,
                radii: inner_radii
            })
        );

        // And the outer one comes back when the inner is popped.
        scene.pop_clip();
        scene.push_quad(filled(13.0, 13.0, 5.0, 5.0));
        assert_eq!(
            scene.batches()[1].rounded_clip,
            Some(RoundedClip {
                bounds: outer,
                radii: outer_radii
            })
        );
    }

    #[test]
    fn a_zero_radius_clip_is_left_to_the_scissor() {
        let mut scene = Scene::new();
        scene.push_rounded_clip(
            Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)),
            Corners::all(Px::ZERO),
        );
        scene.push_quad(filled(1.0, 1.0, 5.0, 5.0));
        assert!(scene.batches()[0].rounded_clip.is_none());
        assert!(scene.batches()[0].clip.is_some());
    }

    #[test]
    fn an_overlay_escapes_the_rounded_clip_around_it() {
        let mut scene = Scene::new();
        let (bounds, radii) = rounded(0.0, 0.0, 100.0, 100.0, 12.0);
        scene.push_rounded_clip(bounds, radii);

        scene.push_layer(1);
        scene.push_quad(filled(200.0, 200.0, 5.0, 5.0));
        scene.pop_layer();

        // The overlay was drawn outside the clip entirely, uncorrupted by it.
        let overlay = scene
            .batches()
            .iter()
            .find(|batch| batch.layer == 1)
            .expect("no overlay batch");
        assert!(overlay.rounded_clip.is_none());
        assert!(overlay.clip.is_none());

        // And the clip is back once the layer closes.
        scene.push_quad(filled(1.0, 1.0, 5.0, 5.0));
        let after = scene.batches().last().unwrap();
        assert_eq!(after.rounded_clip, Some(RoundedClip { bounds, radii }));
    }

    #[test]
    fn sprites_are_clipped_the_same_way_quads_are() {
        let mut scene = Scene::new();
        let (bounds, radii) = rounded(0.0, 0.0, 100.0, 100.0, 12.0);
        scene.push_rounded_clip(bounds, radii);
        scene.push_glyph(glyph_at(4.0, 4.0));
        assert_eq!(
            scene.batches()[0].rounded_clip,
            Some(RoundedClip { bounds, radii })
        );
    }

    fn glyph_at(x: f32, y: f32) -> Glyph {
        Glyph {
            key: guirs_text::GlyphKey {
                font: guirs_text::FontId(0),
                glyph: 1,
                size: Px(16.0),
                phase: 0,
            },
            position: Point::new(Px(x), Px(y)),
            color: Rgba::WHITE,
        }
    }

    #[test]
    fn invisible_primitives_are_dropped() {
        let mut scene = Scene::new();
        scene.push_quad(Quad::default());
        scene.push_glyph(Glyph {
            color: Rgba::TRANSPARENT,
            ..glyph_at(0.0, 0.0)
        });
        assert!(scene.is_empty());
        assert_eq!(scene.batches().len(), 0);
    }

    #[test]
    fn consecutive_primitives_of_one_kind_share_a_batch() {
        let mut scene = Scene::new();
        for i in 0..5 {
            scene.push_quad(filled(i as f32 * 10.0, 0.0, 8.0, 8.0));
        }
        assert_eq!(scene.batches().len(), 1);
        assert_eq!(scene.batches()[0].range, 0..5);
        assert_eq!(scene.batches()[0].kind, BatchKind::Quad);
    }

    #[test]
    fn switching_kind_starts_a_new_batch_and_preserves_order() {
        let mut scene = Scene::new();
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));
        scene.push_glyph(glyph_at(1.0, 1.0));
        scene.push_quad(filled(20.0, 0.0, 10.0, 10.0));

        let batches = scene.batches();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].kind, BatchKind::Quad);
        assert_eq!(batches[1].kind, BatchKind::Sprite);
        assert_eq!(batches[2].kind, BatchKind::Quad);
        // The second quad batch indexes the second quad, not the first.
        assert_eq!(batches[2].range, 1..2);
    }

    #[test]
    fn changing_the_clip_starts_a_new_batch() {
        let mut scene = Scene::new();
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)));
        scene.push_quad(filled(20.0, 20.0, 10.0, 10.0));
        scene.pop_clip();
        assert_eq!(scene.batches().len(), 2);
        assert_eq!(scene.batches()[0].clip, None);
        assert!(scene.batches()[1].clip.is_some());
    }

    #[test]
    fn nested_clips_intersect() {
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)));
        scene.push_clip(Bounds::from_xywh(Px(50.0), Px(50.0), Px(100.0), Px(100.0)));
        let clip = scene.clip().unwrap();
        assert_eq!(clip.origin, Point::new(Px(50.0), Px(50.0)));
        assert_eq!(clip.size.width, Px(50.0));
        assert_eq!(clip.size.height, Px(50.0));
    }

    #[test]
    fn primitives_outside_the_clip_are_culled() {
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0)));
        scene.push_quad(filled(200.0, 200.0, 10.0, 10.0));
        scene.push_quad(filled(10.0, 10.0, 10.0, 10.0));
        scene.pop_clip();

        assert_eq!(scene.stats().quads, 1);
        assert_eq!(scene.stats().culled, 1);
    }

    #[test]
    fn a_shadow_is_culled_against_its_blurred_reach_not_its_box() {
        // The box is outside the clip, but its blur reaches inside, so it must
        // survive culling.
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)));
        scene.push_shadow(Shadow {
            bounds: Bounds::from_xywh(Px(110.0), Px(10.0), Px(50.0), Px(50.0)),
            corner_radii: Corners::zero(),
            shadow: BoxShadow::new(Point::zero(), Px(40.0), Px(0.0), Rgba::BLACK),
            opacity: 1.0,
        });
        scene.pop_clip();
        assert_eq!(scene.stats().quads, 1);
        assert_eq!(scene.stats().culled, 0);
    }

    #[test]
    fn an_overlay_layer_draws_last_and_ignores_the_enclosing_clip() {
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0)));
        scene.push_quad(filled(0.0, 0.0, 10.0, 10.0));

        // A popup inside a clipped container still paints in full, and on top.
        scene.push_layer(1);
        assert_eq!(scene.clip(), None);
        scene.push_quad(filled(200.0, 200.0, 40.0, 40.0));
        scene.pop_layer();

        scene.push_quad(filled(20.0, 20.0, 10.0, 10.0));
        scene.pop_clip();

        // Three quads survive: the overlay was not culled by the outer clip.
        assert_eq!(scene.stats().quads, 3);
        assert_eq!(scene.stats().culled, 0);

        // And the overlay batch is drawn after everything on layer zero.
        let ordered = scene.ordered_batches();
        assert_eq!(ordered.last().unwrap().layer, 1);
    }

    #[test]
    fn leaving_a_layer_restores_the_clip() {
        let mut scene = Scene::new();
        let clip = Bounds::from_xywh(Px(0.0), Px(0.0), Px(50.0), Px(50.0));
        scene.push_clip(clip);
        scene.push_layer(1);
        scene.pop_layer();
        assert_eq!(scene.clip(), Some(clip));
        assert_eq!(scene.layer(), 0);
    }

    #[test]
    fn clearing_keeps_the_scene_reusable() {
        let mut scene = Scene::new();
        scene.push_clip(Bounds::from_xywh(Px(0.0), Px(0.0), Px(10.0), Px(10.0)));
        scene.push_quad(filled(0.0, 0.0, 5.0, 5.0));
        scene.clear();

        assert!(scene.is_empty());
        assert_eq!(scene.clip(), None);
        assert_eq!(scene.stats(), SceneStats::default());
    }

    #[test]
    fn glyph_bounds_snap_to_the_physical_grid() {
        // At scale 2, a pen at 10.3 logical is 20.6 device, floored to 20.
        let bounds = glyph_bounds(Point::new(Px(10.3), Px(20.0)), 1, 12, 8, 10, 2.0);
        assert_eq!(bounds.origin.x, Px(21.0 / 2.0));
        assert_eq!(bounds.origin.y, Px((40.0 - 12.0) / 2.0));
        assert_eq!(bounds.size.width, Px(4.0));
        assert_eq!(bounds.size.height, Px(5.0));
    }
}
