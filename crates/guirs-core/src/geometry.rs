//! Generic 2D geometry.
//!
//! Every shape here is generic over a [`Scalar`], which in practice is either
//! [`Px`] for anything user facing or `f32` for internal math. The
//! generic parameter is what stops logical and physical coordinates from being
//! silently mixed.

use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

use crate::units::Px;

/// The numeric requirement for a coordinate.
pub trait Scalar:
    Copy
    + Default
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<f32, Output = Self>
    + Div<f32, Output = Self>
{
    const ZERO: Self;

    fn from_f32(v: f32) -> Self;
    fn to_f32(self) -> f32;

    #[inline]
    fn smaller(self, other: Self) -> Self {
        if self < other {
            self
        } else {
            other
        }
    }

    #[inline]
    fn larger(self, other: Self) -> Self {
        if self > other {
            self
        } else {
            other
        }
    }

    #[inline]
    fn clamped(self, lo: Self, hi: Self) -> Self {
        self.larger(lo).smaller(hi)
    }

    #[inline]
    fn half(self) -> Self {
        self / 2.0
    }
}

impl Scalar for f32 {
    const ZERO: f32 = 0.0;

    #[inline]
    fn from_f32(v: f32) -> f32 {
        v
    }

    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
}

impl Scalar for Px {
    const ZERO: Px = Px(0.0);

    #[inline]
    fn from_f32(v: f32) -> Px {
        Px(v)
    }

    #[inline]
    fn to_f32(self) -> f32 {
        self.0
    }
}

/// One of the two layout axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    #[inline]
    pub fn cross(self) -> Axis {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}

// ---------------------------------------------------------------------------
// Point
// ---------------------------------------------------------------------------

/// A position in 2D space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

/// Shorthand constructor for [`Point`].
#[inline]
pub const fn point<T>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Scalar> Point<T> {
    pub const fn new(x: T, y: T) -> Self {
        Point { x, y }
    }

    #[inline]
    pub fn zero() -> Self {
        Point {
            x: T::ZERO,
            y: T::ZERO,
        }
    }

    #[inline]
    pub fn splat(v: T) -> Self {
        Point { x: v, y: v }
    }

    #[inline]
    pub fn along(self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }

    #[inline]
    pub fn scale(self, factor: f32) -> Self {
        Point {
            x: self.x * factor,
            y: self.y * factor,
        }
    }

    #[inline]
    pub fn map<U: Scalar>(self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x),
            y: f(self.y),
        }
    }

    #[inline]
    pub fn to_f32(self) -> Point<f32> {
        Point {
            x: self.x.to_f32(),
            y: self.y.to_f32(),
        }
    }

    /// Straight line distance to `other`.
    #[inline]
    pub fn distance_to(self, other: Self) -> f32 {
        let dx = other.x.to_f32() - self.x.to_f32();
        let dy = other.y.to_f32() - self.y.to_f32();
        (dx * dx + dy * dy).sqrt()
    }
}

impl<T: Scalar> Add for Point<T> {
    type Output = Point<T>;
    #[inline]
    fn add(self, rhs: Point<T>) -> Point<T> {
        Point {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl<T: Scalar> Sub for Point<T> {
    type Output = Point<T>;
    #[inline]
    fn sub(self, rhs: Point<T>) -> Point<T> {
        Point {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl<T: Scalar> AddAssign for Point<T> {
    #[inline]
    fn add_assign(&mut self, rhs: Point<T>) {
        self.x = self.x + rhs.x;
        self.y = self.y + rhs.y;
    }
}

impl<T: Scalar> SubAssign for Point<T> {
    #[inline]
    fn sub_assign(&mut self, rhs: Point<T>) {
        self.x = self.x - rhs.x;
        self.y = self.y - rhs.y;
    }
}

impl<T: Scalar> Add<Size<T>> for Point<T> {
    type Output = Point<T>;
    #[inline]
    fn add(self, rhs: Size<T>) -> Point<T> {
        Point {
            x: self.x + rhs.width,
            y: self.y + rhs.height,
        }
    }
}

// ---------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------

/// A 2D extent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}

/// Shorthand constructor for [`Size`].
#[inline]
pub const fn size<T>(width: T, height: T) -> Size<T> {
    Size { width, height }
}

impl<T: Scalar> Size<T> {
    pub const fn new(width: T, height: T) -> Self {
        Size { width, height }
    }

    #[inline]
    pub fn zero() -> Self {
        Size {
            width: T::ZERO,
            height: T::ZERO,
        }
    }

    #[inline]
    pub fn splat(v: T) -> Self {
        Size {
            width: v,
            height: v,
        }
    }

    #[inline]
    pub fn along(self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.width,
            Axis::Vertical => self.height,
        }
    }

    #[inline]
    pub fn area(self) -> f32 {
        self.width.to_f32() * self.height.to_f32()
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.width <= T::ZERO || self.height <= T::ZERO
    }

    #[inline]
    pub fn scale(self, factor: f32) -> Self {
        Size {
            width: self.width * factor,
            height: self.height * factor,
        }
    }

    #[inline]
    pub fn map<U: Scalar>(self, f: impl Fn(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }

    #[inline]
    pub fn to_f32(self) -> Size<f32> {
        Size {
            width: self.width.to_f32(),
            height: self.height.to_f32(),
        }
    }

    /// Shrink by `edges` on all four sides, clamping at zero.
    #[inline]
    pub fn deflate(self, edges: Edges<T>) -> Self {
        Size {
            width: (self.width - edges.left - edges.right).larger(T::ZERO),
            height: (self.height - edges.top - edges.bottom).larger(T::ZERO),
        }
    }

    /// Grow by `edges` on all four sides.
    #[inline]
    pub fn inflate(self, edges: Edges<T>) -> Self {
        Size {
            width: self.width + edges.left + edges.right,
            height: self.height + edges.top + edges.bottom,
        }
    }
}

impl<T: Scalar> Add for Size<T> {
    type Output = Size<T>;
    #[inline]
    fn add(self, rhs: Size<T>) -> Size<T> {
        Size {
            width: self.width + rhs.width,
            height: self.height + rhs.height,
        }
    }
}

impl<T: Scalar> Sub for Size<T> {
    type Output = Size<T>;
    #[inline]
    fn sub(self, rhs: Size<T>) -> Size<T> {
        Size {
            width: self.width - rhs.width,
            height: self.height - rhs.height,
        }
    }
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// An axis aligned rectangle, stored as an origin plus an extent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bounds<T> {
    pub origin: Point<T>,
    pub size: Size<T>,
}

impl<T: Scalar> Bounds<T> {
    pub const fn new(origin: Point<T>, size: Size<T>) -> Self {
        Bounds { origin, size }
    }

    #[inline]
    pub fn zero() -> Self {
        Bounds {
            origin: Point::zero(),
            size: Size::zero(),
        }
    }

    #[inline]
    pub fn from_xywh(x: T, y: T, width: T, height: T) -> Self {
        Bounds {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    /// Build from two opposite corners, in any order.
    #[inline]
    pub fn from_corners(a: Point<T>, b: Point<T>) -> Self {
        let min = Point {
            x: a.x.smaller(b.x),
            y: a.y.smaller(b.y),
        };
        let max = Point {
            x: a.x.larger(b.x),
            y: a.y.larger(b.y),
        };
        Bounds {
            origin: min,
            size: Size {
                width: max.x - min.x,
                height: max.y - min.y,
            },
        }
    }

    #[inline]
    pub fn left(&self) -> T {
        self.origin.x
    }

    #[inline]
    pub fn top(&self) -> T {
        self.origin.y
    }

    #[inline]
    pub fn right(&self) -> T {
        self.origin.x + self.size.width
    }

    #[inline]
    pub fn bottom(&self) -> T {
        self.origin.y + self.size.height
    }

    #[inline]
    pub fn width(&self) -> T {
        self.size.width
    }

    #[inline]
    pub fn height(&self) -> T {
        self.size.height
    }

    #[inline]
    pub fn top_left(&self) -> Point<T> {
        self.origin
    }

    #[inline]
    pub fn top_right(&self) -> Point<T> {
        Point {
            x: self.right(),
            y: self.top(),
        }
    }

    #[inline]
    pub fn bottom_left(&self) -> Point<T> {
        Point {
            x: self.left(),
            y: self.bottom(),
        }
    }

    #[inline]
    pub fn bottom_right(&self) -> Point<T> {
        Point {
            x: self.right(),
            y: self.bottom(),
        }
    }

    #[inline]
    pub fn center(&self) -> Point<T> {
        Point {
            x: self.origin.x + self.size.width.half(),
            y: self.origin.y + self.size.height.half(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size.is_empty()
    }

    #[inline]
    pub fn contains(&self, p: Point<T>) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    #[inline]
    pub fn intersects(&self, other: &Bounds<T>) -> bool {
        self.left() < other.right()
            && other.left() < self.right()
            && self.top() < other.bottom()
            && other.top() < self.bottom()
    }

    /// The overlapping region, or an empty rect when they do not overlap.
    #[inline]
    pub fn intersect(&self, other: &Bounds<T>) -> Bounds<T> {
        let left = self.left().larger(other.left());
        let top = self.top().larger(other.top());
        let right = self.right().smaller(other.right());
        let bottom = self.bottom().smaller(other.bottom());
        Bounds {
            origin: Point { x: left, y: top },
            size: Size {
                width: (right - left).larger(T::ZERO),
                height: (bottom - top).larger(T::ZERO),
            },
        }
    }

    /// The smallest rect containing both.
    #[inline]
    pub fn union(&self, other: &Bounds<T>) -> Bounds<T> {
        Bounds::from_corners(
            Point {
                x: self.left().smaller(other.left()),
                y: self.top().smaller(other.top()),
            },
            Point {
                x: self.right().larger(other.right()),
                y: self.bottom().larger(other.bottom()),
            },
        )
    }

    /// Shrink inward by `edges`.
    #[inline]
    pub fn inset(&self, edges: Edges<T>) -> Bounds<T> {
        Bounds {
            origin: Point {
                x: self.origin.x + edges.left,
                y: self.origin.y + edges.top,
            },
            size: self.size.deflate(edges),
        }
    }

    /// Grow outward by `edges`.
    #[inline]
    pub fn outset(&self, edges: Edges<T>) -> Bounds<T> {
        Bounds {
            origin: Point {
                x: self.origin.x - edges.left,
                y: self.origin.y - edges.top,
            },
            size: self.size.inflate(edges),
        }
    }

    /// Grow outward by a uniform amount.
    #[inline]
    pub fn expand(&self, amount: T) -> Bounds<T> {
        self.outset(Edges::all(amount))
    }

    #[inline]
    pub fn translate(&self, delta: Point<T>) -> Bounds<T> {
        Bounds {
            origin: self.origin + delta,
            size: self.size,
        }
    }

    #[inline]
    pub fn scale(&self, factor: f32) -> Bounds<T> {
        Bounds {
            origin: self.origin.scale(factor),
            size: self.size.scale(factor),
        }
    }

    #[inline]
    pub fn map<U: Scalar>(&self, f: impl Fn(T) -> U + Copy) -> Bounds<U> {
        Bounds {
            origin: self.origin.map(f),
            size: self.size.map(f),
        }
    }

    #[inline]
    pub fn to_f32(&self) -> Bounds<f32> {
        Bounds {
            origin: self.origin.to_f32(),
            size: self.size.to_f32(),
        }
    }
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

/// Four values, one per side. Field order matches CSS shorthand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Scalar> Edges<T> {
    pub const fn new(top: T, right: T, bottom: T, left: T) -> Self {
        Edges {
            top,
            right,
            bottom,
            left,
        }
    }

    #[inline]
    pub fn zero() -> Self {
        Edges {
            top: T::ZERO,
            right: T::ZERO,
            bottom: T::ZERO,
            left: T::ZERO,
        }
    }

    #[inline]
    pub fn all(v: T) -> Self {
        Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// `x` applies to left and right, `y` to top and bottom.
    #[inline]
    pub fn xy(x: T, y: T) -> Self {
        Edges {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }

    #[inline]
    pub fn horizontal(&self) -> T {
        self.left + self.right
    }

    #[inline]
    pub fn vertical(&self) -> T {
        self.top + self.bottom
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.top <= T::ZERO
            && self.right <= T::ZERO
            && self.bottom <= T::ZERO
            && self.left <= T::ZERO
    }

    #[inline]
    pub fn scale(self, factor: f32) -> Self {
        Edges {
            top: self.top * factor,
            right: self.right * factor,
            bottom: self.bottom * factor,
            left: self.left * factor,
        }
    }

    #[inline]
    pub fn map<U: Scalar>(self, f: impl Fn(T) -> U) -> Edges<U> {
        Edges {
            top: f(self.top),
            right: f(self.right),
            bottom: f(self.bottom),
            left: f(self.left),
        }
    }
}

// ---------------------------------------------------------------------------
// Corners
// ---------------------------------------------------------------------------

/// Four values, one per corner. Field order matches CSS `border-radius`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Corners<T> {
    pub top_left: T,
    pub top_right: T,
    pub bottom_right: T,
    pub bottom_left: T,
}

impl<T: Scalar> Corners<T> {
    pub const fn new(top_left: T, top_right: T, bottom_right: T, bottom_left: T) -> Self {
        Corners {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    #[inline]
    pub fn zero() -> Self {
        Corners {
            top_left: T::ZERO,
            top_right: T::ZERO,
            bottom_right: T::ZERO,
            bottom_left: T::ZERO,
        }
    }

    #[inline]
    pub fn all(v: T) -> Self {
        Corners {
            top_left: v,
            top_right: v,
            bottom_right: v,
            bottom_left: v,
        }
    }

    #[inline]
    pub fn top(v: T) -> Self {
        Corners {
            top_left: v,
            top_right: v,
            bottom_right: T::ZERO,
            bottom_left: T::ZERO,
        }
    }

    #[inline]
    pub fn bottom(v: T) -> Self {
        Corners {
            top_left: T::ZERO,
            top_right: T::ZERO,
            bottom_right: v,
            bottom_left: v,
        }
    }

    #[inline]
    pub fn max(&self) -> T {
        self.top_left
            .larger(self.top_right)
            .larger(self.bottom_right)
            .larger(self.bottom_left)
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.max().to_f32() < 0.01
    }

    #[inline]
    pub fn is_uniform(&self) -> bool {
        let a = self.top_left.to_f32();
        (self.top_right.to_f32() - a).abs() < 0.01
            && (self.bottom_right.to_f32() - a).abs() < 0.01
            && (self.bottom_left.to_f32() - a).abs() < 0.01
    }

    #[inline]
    pub fn scale(self, factor: f32) -> Self {
        Corners {
            top_left: self.top_left * factor,
            top_right: self.top_right * factor,
            bottom_right: self.bottom_right * factor,
            bottom_left: self.bottom_left * factor,
        }
    }

    #[inline]
    pub fn map<U: Scalar>(self, f: impl Fn(T) -> U) -> Corners<U> {
        Corners {
            top_left: f(self.top_left),
            top_right: f(self.top_right),
            bottom_right: f(self.bottom_right),
            bottom_left: f(self.bottom_left),
        }
    }

    /// Clamp every radius so no pair on a side can overlap.
    ///
    /// A radius larger than half the shorter dimension produces a self
    /// intersecting outline, which the signed distance field in the shader
    /// renders as visible artefacts. Clamping here keeps the shader branch free.
    #[inline]
    pub fn clamp_to(self, size: Size<T>) -> Self {
        let limit = size.width.smaller(size.height).half().larger(T::ZERO);
        Corners {
            top_left: self.top_left.clamped(T::ZERO, limit),
            top_right: self.top_right.clamped(T::ZERO, limit),
            bottom_right: self.bottom_right.clamped(T::ZERO, limit),
            bottom_left: self.bottom_left.clamped(T::ZERO, limit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::px;

    #[test]
    fn bounds_hit_testing_excludes_the_far_edges() {
        let b = Bounds::from_xywh(px(10.0), px(10.0), px(100.0), px(50.0));
        assert!(b.contains(point(px(10.0), px(10.0))));
        assert!(b.contains(point(px(109.9), px(59.9))));
        assert!(!b.contains(point(px(110.0), px(30.0))));
        assert!(!b.contains(point(px(9.9), px(30.0))));
    }

    #[test]
    fn intersect_of_disjoint_rects_is_empty() {
        let a = Bounds::from_xywh(0.0f32, 0.0, 10.0, 10.0);
        let b = Bounds::from_xywh(20.0f32, 20.0, 10.0, 10.0);
        assert!(a.intersect(&b).is_empty());
        assert!(!a.intersects(&b));
    }

    #[test]
    fn insetting_never_produces_negative_extents() {
        let b = Bounds::from_xywh(px(0.0), px(0.0), px(10.0), px(10.0));
        let inset = b.inset(Edges::all(px(20.0)));
        assert_eq!(inset.size, Size::new(px(0.0), px(0.0)));
    }

    #[test]
    fn corner_radii_clamp_to_half_the_shorter_side() {
        let clamped = Corners::all(px(50.0)).clamp_to(Size::new(px(40.0), px(20.0)));
        assert_eq!(clamped.max(), px(10.0));
    }

    #[test]
    fn union_covers_both_inputs() {
        let a = Bounds::from_xywh(0.0f32, 0.0, 10.0, 10.0);
        let b = Bounds::from_xywh(20.0f32, 5.0, 10.0, 10.0);
        let u = a.union(&b);
        assert_eq!(u.left(), 0.0);
        assert_eq!(u.top(), 0.0);
        assert_eq!(u.right(), 30.0);
        assert_eq!(u.bottom(), 15.0);
    }
}
