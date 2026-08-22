//! Two dimensional affine transforms.
//!
//! Two shapes, for two jobs.
//!
//! [`Transform2D`] is the decomposed form: a translation, a rotation, a scale
//! and an origin to turn about. That is what a stylesheet writes, and what an
//! animation interpolates, because interpolating a matrix is not the same as
//! interpolating the movement it describes. Halfway between no rotation and a
//! half turn is a matrix that scales to nothing, which is not what anyone
//! writing `rotate(180deg)` had in mind.
//!
//! [`Affine`] is the composed matrix, which is what the vertex shader and hit
//! testing need. Going from one to the other happens once per element per
//! frame, against bounds only paint knows.

use std::hash::{Hash, Hasher};

use crate::geometry::{Bounds, Point, Size};
use crate::units::Px;

/// A 2 by 2 matrix with a translation, in row major order.
///
/// A point maps to `(a x + c y + tx, b x + d y + ty)`, the same convention CSS
/// and Core Graphics use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn translate(x: f32, y: f32) -> Self {
        Affine {
            tx: x,
            ty: y,
            ..Affine::IDENTITY
        }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Affine {
            a: x,
            d: y,
            ..Affine::IDENTITY
        }
    }

    pub fn rotate(degrees: f32) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Affine {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    #[inline]
    pub fn is_identity(&self) -> bool {
        *self == Affine::IDENTITY
    }

    /// Apply `self` first, then `other`.
    pub fn then(&self, other: &Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            tx: self.tx * other.a + self.ty * other.c + other.tx,
            ty: self.tx * other.b + self.ty * other.d + other.ty,
        }
    }

    pub fn apply(&self, point: Point<Px>) -> Point<Px> {
        let x = point.x.0;
        let y = point.y.0;
        Point::new(
            Px(self.a * x + self.c * y + self.tx),
            Px(self.b * x + self.d * y + self.ty),
        )
    }

    #[inline]
    pub fn determinant(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse, or `None` when the transform collapses to nothing.
    ///
    /// A zero scale is a legitimate thing to animate through, so this returns
    /// an option rather than dividing by zero and producing infinities that
    /// would poison hit testing.
    pub fn invert(&self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            tx: (self.c * self.ty - self.d * self.tx) * inv,
            ty: (self.b * self.tx - self.a * self.ty) * inv,
        })
    }

    /// The smallest axis aligned rectangle containing the transformed corners.
    ///
    /// A rotated rectangle is not a rectangle, so anything that needs one, a
    /// scissor rectangle above all, has to settle for the box around it.
    pub fn transform_bounds(&self, bounds: Bounds<Px>) -> Bounds<Px> {
        let corners = [
            self.apply(bounds.origin),
            self.apply(Point::new(bounds.right(), bounds.top())),
            self.apply(Point::new(bounds.right(), bounds.bottom())),
            self.apply(Point::new(bounds.left(), bounds.bottom())),
        ];
        let mut left = corners[0].x;
        let mut top = corners[0].y;
        let mut right = corners[0].x;
        let mut bottom = corners[0].y;
        for corner in &corners[1..] {
            left = left.min(corner.x);
            top = top.min(corner.y);
            right = right.max(corner.x);
            bottom = bottom.max(corner.y);
        }
        Bounds {
            origin: Point::new(left, top),
            size: Size::new(right - left, bottom - top),
        }
    }

    /// Roughly how much this magnifies a length.
    ///
    /// Antialiasing works in device pixels, so a scaled shape needs its
    /// coverage ramp widened to match or its edge goes hard. The geometric
    /// mean of the two axis scales is exact for a uniform scale and a sensible
    /// compromise for anything else.
    pub fn average_scale(&self) -> f32 {
        let determinant = self.determinant().abs();
        if determinant <= 0.0 {
            return 1.0;
        }
        determinant.sqrt()
    }
}

/// A transform as a stylesheet describes it.
///
/// Kept decomposed rather than multiplied out, because this is the form that
/// animates correctly and the form a reader recognises.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2D {
    pub translate_x: Px,
    pub translate_y: Px,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Clockwise, in degrees, matching CSS.
    pub rotate: f32,
    /// The point everything turns and scales about, as a fraction of the
    /// element's own box. The center by default, which is what makes a button
    /// growing on press look like it grew rather than slid.
    pub origin_x: f32,
    pub origin_y: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Transform2D::IDENTITY
    }
}

impl Transform2D {
    pub const IDENTITY: Transform2D = Transform2D {
        translate_x: Px::ZERO,
        translate_y: Px::ZERO,
        scale_x: 1.0,
        scale_y: 1.0,
        rotate: 0.0,
        origin_x: 0.5,
        origin_y: 0.5,
    };

    #[inline]
    pub fn is_identity(&self) -> bool {
        self.translate_x == Px::ZERO
            && self.translate_y == Px::ZERO
            && self.scale_x == 1.0
            && self.scale_y == 1.0
            && self.rotate == 0.0
    }

    pub fn translate(x: Px, y: Px) -> Self {
        Transform2D {
            translate_x: x,
            translate_y: y,
            ..Transform2D::IDENTITY
        }
    }

    pub fn scale(factor: f32) -> Self {
        Transform2D::scale_xy(factor, factor)
    }

    pub fn scale_xy(x: f32, y: f32) -> Self {
        Transform2D {
            scale_x: x,
            scale_y: y,
            ..Transform2D::IDENTITY
        }
    }

    pub fn rotate(degrees: f32) -> Self {
        Transform2D {
            rotate: degrees,
            ..Transform2D::IDENTITY
        }
    }

    /// Compose into a matrix, about an element's own box.
    ///
    /// The order is the one CSS applies: translate, then rotate, then scale,
    /// all about the origin point rather than about the top left corner.
    pub fn to_affine(&self, bounds: Bounds<Px>) -> Affine {
        if self.is_identity() {
            return Affine::IDENTITY;
        }
        let pivot = Point::new(
            bounds.origin.x + bounds.size.width * self.origin_x,
            bounds.origin.y + bounds.size.height * self.origin_y,
        );

        Affine::translate(-pivot.x.0, -pivot.y.0)
            .then(&Affine::scale(self.scale_x, self.scale_y))
            .then(&Affine::rotate(self.rotate))
            .then(&Affine::translate(pivot.x.0, pivot.y.0))
            .then(&Affine::translate(self.translate_x.0, self.translate_y.0))
    }

    /// Interpolate componentwise.
    ///
    /// This is why the decomposed form is kept: halfway between no rotation
    /// and a half turn is a quarter turn here, where interpolating the two
    /// matrices instead would pass through a shape squashed to a line.
    pub fn lerp(&self, other: &Transform2D, t: f32) -> Transform2D {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Transform2D {
            translate_x: Px(mix(self.translate_x.0, other.translate_x.0)),
            translate_y: Px(mix(self.translate_y.0, other.translate_y.0)),
            scale_x: mix(self.scale_x, other.scale_x),
            scale_y: mix(self.scale_y, other.scale_y),
            rotate: mix(self.rotate, other.rotate),
            origin_x: mix(self.origin_x, other.origin_x),
            origin_y: mix(self.origin_y, other.origin_y),
        }
    }
}

// f32 has no total ordering, so these are written against the bit patterns.
// Two transforms are interchangeable only when identical, which is what a
// cache key and a batch comparison both want.
impl Eq for Transform2D {}

impl Hash for Transform2D {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.translate_x.hash(state);
        self.translate_y.hash(state);
        self.scale_x.to_bits().hash(state);
        self.scale_y.to_bits().hash(state);
        self.rotate.to_bits().hash(state);
        self.origin_x.to_bits().hash(state);
        self.origin_y.to_bits().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(x: f32, y: f32, w: f32, h: f32) -> Bounds<Px> {
        Bounds::from_xywh(Px(x), Px(y), Px(w), Px(h))
    }

    fn close(a: Point<Px>, b: Point<Px>) -> bool {
        (a.x.0 - b.x.0).abs() < 0.001 && (a.y.0 - b.y.0).abs() < 0.001
    }

    #[test]
    fn the_identity_leaves_a_point_alone() {
        let point = Point::new(Px(3.0), Px(7.0));
        assert_eq!(Affine::IDENTITY.apply(point), point);
        assert!(Affine::IDENTITY.is_identity());
    }

    #[test]
    fn composition_applies_the_first_transform_first() {
        // Scale then translate moves by the untouched translation; translate
        // then scale multiplies the translation too. Getting these the wrong
        // way round is the classic transform bug.
        let scale_then_move = Affine::scale(2.0, 2.0).then(&Affine::translate(10.0, 0.0));
        assert!(close(
            scale_then_move.apply(Point::new(Px(1.0), Px(0.0))),
            Point::new(Px(12.0), Px(0.0))
        ));

        let move_then_scale = Affine::translate(10.0, 0.0).then(&Affine::scale(2.0, 2.0));
        assert!(close(
            move_then_scale.apply(Point::new(Px(1.0), Px(0.0))),
            Point::new(Px(22.0), Px(0.0))
        ));
    }

    #[test]
    fn a_quarter_turn_sends_right_to_down() {
        // Clockwise, with y pointing down the screen.
        let turned = Affine::rotate(90.0).apply(Point::new(Px(1.0), Px(0.0)));
        assert!(close(turned, Point::new(Px(0.0), Px(1.0))));
    }

    #[test]
    fn inverting_undoes_the_transform() {
        let transform = Affine::translate(5.0, -3.0)
            .then(&Affine::rotate(37.0))
            .then(&Affine::scale(2.0, 0.5));
        let inverse = transform.invert().expect("should be invertible");
        let point = Point::new(Px(11.0), Px(4.0));
        assert!(close(inverse.apply(transform.apply(point)), point));
    }

    #[test]
    fn a_collapsed_transform_has_no_inverse() {
        assert!(Affine::scale(0.0, 1.0).invert().is_none());
        assert!(Affine::scale(1.0, 0.0).invert().is_none());
    }

    #[test]
    fn scaling_about_the_center_keeps_the_center_still() {
        let bounds = box_at(100.0, 50.0, 200.0, 100.0);
        let affine = Transform2D::scale(2.0).to_affine(bounds);
        let center = Point::new(Px(200.0), Px(100.0));
        assert!(close(affine.apply(center), center));

        // And the box doubles about it.
        let grown = affine.transform_bounds(bounds);
        assert!((grown.size.width.0 - 400.0).abs() < 0.001);
        assert!((grown.origin.x.0 - 0.0).abs() < 0.001);
    }

    #[test]
    fn the_origin_moves_what_stays_still() {
        let bounds = box_at(0.0, 0.0, 100.0, 100.0);
        let top_left = Transform2D {
            origin_x: 0.0,
            origin_y: 0.0,
            ..Transform2D::scale(2.0)
        };
        let affine = top_left.to_affine(bounds);
        assert!(close(affine.apply(Point::zero()), Point::zero()));
        assert!(close(
            affine.apply(Point::new(Px(100.0), Px(100.0))),
            Point::new(Px(200.0), Px(200.0))
        ));
    }

    #[test]
    fn a_rotated_box_reports_the_box_around_it() {
        let bounds = box_at(0.0, 0.0, 100.0, 100.0);
        let affine = Transform2D::rotate(45.0).to_affine(bounds);
        let around = affine.transform_bounds(bounds);
        // A square turned an eighth of a turn spans its diagonal.
        assert!((around.size.width.0 - 141.42).abs() < 0.1);
        assert!((around.size.height.0 - 141.42).abs() < 0.1);
    }

    #[test]
    fn interpolation_goes_through_the_movement_not_the_matrix() {
        let half = Transform2D::IDENTITY.lerp(&Transform2D::rotate(180.0), 0.5);
        assert_eq!(half.rotate, 90.0);
        // The matrix midpoint of those two would be a shape squashed flat.
        assert_eq!(half.scale_x, 1.0);
        assert_eq!(half.scale_y, 1.0);
    }

    #[test]
    fn an_identity_transform_is_recognised_whatever_its_origin() {
        assert!(Transform2D::IDENTITY.is_identity());
        assert!(Transform2D {
            origin_x: 0.0,
            origin_y: 1.0,
            ..Transform2D::IDENTITY
        }
        .is_identity());
        assert!(!Transform2D::scale(1.001).is_identity());
        assert!(!Transform2D::rotate(0.5).is_identity());
    }

    #[test]
    fn the_average_scale_matches_a_uniform_one() {
        assert!((Affine::scale(3.0, 3.0).average_scale() - 3.0).abs() < 0.001);
        assert!((Affine::IDENTITY.average_scale() - 1.0).abs() < 0.001);
        // Rotation does not change how big anything is.
        assert!((Affine::rotate(33.0).average_scale() - 1.0).abs() < 0.001);
    }
}
