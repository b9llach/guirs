//! Fills, gradients, borders and shadows.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::color::Rgba;
use crate::geometry::Point;
use crate::units::Px;

/// One color stop along a gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient line, `0.0..=1.0`.
    pub offset: f32,
    pub color: Rgba,
}

impl GradientStop {
    #[inline]
    pub const fn new(offset: f32, color: Rgba) -> Self {
        GradientStop { offset, color }
    }
}

impl Eq for GradientStop {}

impl Hash for GradientStop {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.offset.to_bits().hash(state);
        self.color.hash(state);
    }
}

/// The geometry a gradient is projected along.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    /// A linear gradient. Following the CSS convention, `0` degrees points up
    /// and the angle increases clockwise.
    Linear { angle_deg: f32 },
    /// A radial gradient from the center of the box out to its farthest corner.
    Radial,
}

impl Eq for GradientKind {}

impl Hash for GradientKind {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            GradientKind::Linear { angle_deg } => {
                0u8.hash(state);
                angle_deg.to_bits().hash(state);
            }
            GradientKind::Radial => 1u8.hash(state),
        }
    }
}

/// A multi stop gradient.
///
/// Gradients are rasterized once into a shared 256 pixel ramp texture and
/// referenced by row from the quad shader, so the number of stops has no effect
/// on per instance cost.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
}

impl Gradient {
    pub fn linear(angle_deg: f32, stops: Vec<GradientStop>) -> Self {
        Gradient {
            kind: GradientKind::Linear { angle_deg },
            stops,
        }
    }

    pub fn radial(stops: Vec<GradientStop>) -> Self {
        Gradient {
            kind: GradientKind::Radial,
            stops,
        }
    }

    /// A two stop vertical gradient, top to bottom.
    pub fn vertical(top: Rgba, bottom: Rgba) -> Self {
        Gradient::linear(
            180.0,
            vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        )
    }

    /// A two stop horizontal gradient, left to right.
    pub fn horizontal(left: Rgba, right: Rgba) -> Self {
        Gradient::linear(
            90.0,
            vec![GradientStop::new(0.0, left), GradientStop::new(1.0, right)],
        )
    }

    /// Sample the ramp at `t`, with stops treated as sorted.
    pub fn sample(&self, t: f32) -> Rgba {
        match self.stops.len() {
            0 => Rgba::TRANSPARENT,
            1 => self.stops[0].color,
            _ => {
                let t = t.clamp(0.0, 1.0);
                if t <= self.stops[0].offset {
                    return self.stops[0].color;
                }
                let last = self.stops.len() - 1;
                if t >= self.stops[last].offset {
                    return self.stops[last].color;
                }
                for w in self.stops.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    if t >= a.offset && t <= b.offset {
                        let span = b.offset - a.offset;
                        let local = if span.abs() < f32::EPSILON {
                            0.0
                        } else {
                            (t - a.offset) / span
                        };
                        return a.color.lerp(b.color, local);
                    }
                }
                self.stops[last].color
            }
        }
    }

    /// Rasterize into a 256 entry ramp of packed RGBA8 values.
    pub fn to_ramp(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 * 4);
        for i in 0..256 {
            let c = self.sample(i as f32 / 255.0);
            out.push((c.r.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((c.g.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((c.b.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((c.a.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        out
    }
}

/// How a box is filled.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Paint {
    /// Draw nothing.
    #[default]
    None,
    Solid(Rgba),
    /// Shared so that cloning a style stays cheap.
    Gradient(Arc<Gradient>),
}

impl Paint {
    #[inline]
    pub fn solid(color: Rgba) -> Self {
        Paint::Solid(color)
    }

    pub fn linear(angle_deg: f32, stops: Vec<GradientStop>) -> Self {
        Paint::Gradient(Arc::new(Gradient::linear(angle_deg, stops)))
    }

    pub fn radial(stops: Vec<GradientStop>) -> Self {
        Paint::Gradient(Arc::new(Gradient::radial(stops)))
    }

    /// Whether this paint would put anything on screen.
    pub fn is_visible(&self) -> bool {
        match self {
            Paint::None => false,
            Paint::Solid(c) => !c.is_transparent(),
            Paint::Gradient(g) => g.stops.iter().any(|s| !s.color.is_transparent()),
        }
    }

    /// The color at the center of the paint, used where a single
    /// representative color is needed, such as a caret or a focus ring.
    pub fn representative(&self) -> Rgba {
        match self {
            Paint::None => Rgba::TRANSPARENT,
            Paint::Solid(c) => *c,
            Paint::Gradient(g) => g.sample(0.5),
        }
    }

    /// Scale alpha across the whole paint.
    pub fn fade(&self, opacity: f32) -> Paint {
        match self {
            Paint::None => Paint::None,
            Paint::Solid(c) => Paint::Solid(c.fade(opacity)),
            Paint::Gradient(g) => Paint::Gradient(Arc::new(Gradient {
                kind: g.kind,
                stops: g
                    .stops
                    .iter()
                    .map(|s| GradientStop::new(s.offset, s.color.fade(opacity)))
                    .collect(),
            })),
        }
    }

    /// Interpolate for a style transition.
    ///
    /// Solid to solid interpolates componentwise. Gradients interpolate stop by
    /// stop when their shapes match. Anything else has no meaningful in between
    /// representation in a single draw call, so it swaps at the midpoint.
    pub fn lerp(&self, other: &Paint, t: f32) -> Paint {
        match (self, other) {
            (Paint::Solid(a), Paint::Solid(b)) => Paint::Solid(a.lerp(*b, t)),
            (Paint::None, Paint::Solid(b)) => Paint::Solid(b.with_alpha(b.a * t)),
            (Paint::Solid(a), Paint::None) => Paint::Solid(a.with_alpha(a.a * (1.0 - t))),
            (Paint::Gradient(a), Paint::Gradient(b))
                if a.kind_matches(b) && a.stops.len() == b.stops.len() =>
            {
                let kind = match (a.kind, b.kind) {
                    (
                        GradientKind::Linear { angle_deg: x },
                        GradientKind::Linear { angle_deg: y },
                    ) => GradientKind::Linear {
                        angle_deg: x + (y - x) * t,
                    },
                    _ => a.kind,
                };
                let stops = a
                    .stops
                    .iter()
                    .zip(b.stops.iter())
                    .map(|(sa, sb)| {
                        GradientStop::new(
                            sa.offset + (sb.offset - sa.offset) * t,
                            sa.color.lerp(sb.color, t),
                        )
                    })
                    .collect();
                Paint::Gradient(Arc::new(Gradient { kind, stops }))
            }
            _ => {
                if t < 0.5 {
                    self.clone()
                } else {
                    other.clone()
                }
            }
        }
    }
}

impl Gradient {
    fn kind_matches(&self, other: &Gradient) -> bool {
        matches!(
            (self.kind, other.kind),
            (GradientKind::Linear { .. }, GradientKind::Linear { .. })
                | (GradientKind::Radial, GradientKind::Radial)
        )
    }
}

impl From<Rgba> for Paint {
    fn from(c: Rgba) -> Paint {
        Paint::Solid(c)
    }
}

/// How a border line is drawn.
///
/// `Solid` is rendered analytically by the quad shader. `Dashed` and `Dotted`
/// currently fall back to a solid line; they parse and round trip so that
/// stylesheets stay forward compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BorderStyle {
    #[default]
    None,
    Solid,
    Dashed,
    Dotted,
}

impl BorderStyle {
    #[inline]
    pub fn draws(self) -> bool {
        !matches!(self, BorderStyle::None)
    }
}

/// A drop shadow or inner shadow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct BoxShadow {
    pub color: Rgba,
    pub offset: Point<Px>,
    /// Gaussian blur radius. The shader approximates the true Gaussian with an
    /// error function fit, which is accurate to well under one pixel.
    pub blur: Px,
    /// Grows the shadow shape before blurring.
    pub spread: Px,
    /// When set, the shadow is drawn inside the box instead of outside it.
    pub inset: bool,
}

impl BoxShadow {
    pub fn new(offset: Point<Px>, blur: Px, spread: Px, color: Rgba) -> Self {
        BoxShadow {
            color,
            offset,
            blur,
            spread,
            inset: false,
        }
    }

    pub fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    #[inline]
    pub fn is_visible(&self) -> bool {
        !self.color.is_transparent()
    }

    /// How far past the box edge this shadow can reach, used to size the quad
    /// the shader rasterizes into.
    #[inline]
    pub fn extent(&self) -> Px {
        // A Gaussian is effectively zero beyond three standard deviations, and
        // the blur radius is two standard deviations by CSS convention.
        self.blur * 1.5 + self.spread + Px(self.offset.x.0.abs().max(self.offset.y.0.abs()))
    }

    pub fn lerp(&self, other: &BoxShadow, t: f32) -> BoxShadow {
        BoxShadow {
            color: self.color.lerp(other.color, t),
            offset: Point::new(
                self.offset.x + (other.offset.x - self.offset.x) * t,
                self.offset.y + (other.offset.y - self.offset.y) * t,
            ),
            blur: self.blur + (other.blur - self.blur) * t,
            spread: self.spread + (other.spread - self.spread) * t,
            inset: if t < 0.5 { self.inset } else { other.inset },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_sampling_hits_the_stops_exactly() {
        let g = Gradient::vertical(Rgba::BLACK, Rgba::WHITE);
        assert_eq!(g.sample(0.0), Rgba::BLACK);
        assert_eq!(g.sample(1.0), Rgba::WHITE);
        let mid = g.sample(0.5);
        assert!((mid.r - 0.5).abs() < 0.01);
    }

    #[test]
    fn sampling_clamps_outside_the_stop_range() {
        let g = Gradient::linear(
            0.0,
            vec![
                GradientStop::new(0.25, Rgba::BLACK),
                GradientStop::new(0.75, Rgba::WHITE),
            ],
        );
        assert_eq!(g.sample(0.0), Rgba::BLACK);
        assert_eq!(g.sample(1.0), Rgba::WHITE);
    }

    #[test]
    fn ramp_is_the_expected_size() {
        assert_eq!(Gradient::vertical(Rgba::BLACK, Rgba::WHITE).to_ramp().len(), 1024);
    }

    #[test]
    fn paint_transitions_between_solids() {
        let a = Paint::solid(Rgba::BLACK);
        let b = Paint::solid(Rgba::WHITE);
        match a.lerp(&b, 0.5) {
            Paint::Solid(c) => assert!((c.r - 0.5).abs() < 0.01),
            other => panic!("expected a solid, got {other:?}"),
        }
    }

    #[test]
    fn mismatched_paints_swap_at_the_midpoint() {
        let a = Paint::solid(Rgba::BLACK);
        let b = Paint::linear(0.0, vec![GradientStop::new(0.0, Rgba::WHITE)]);
        assert_eq!(a.lerp(&b, 0.25), a);
        assert_eq!(a.lerp(&b, 0.75), b);
    }

    #[test]
    fn shadow_extent_covers_blur_spread_and_offset() {
        let s = BoxShadow::new(Point::new(Px(0.0), Px(4.0)), Px(12.0), Px(2.0), Rgba::BLACK);
        assert!(s.extent() > Px(20.0));
    }
}
