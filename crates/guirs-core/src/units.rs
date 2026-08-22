//! Units of measure.
//!
//! guirs distinguishes three kinds of pixel:
//!
//! * [`Px`] - logical pixels. This is what layout, style and widget code use.
//! * [`DevicePx`] - physical pixels on the actual display surface.
//! * [`Rems`] - multiples of the root font size, for type-relative sizing.
//!
//! Converting between logical and physical goes through a [`ScaleFactor`],
//! which is the window's DPI scale. Keeping these as distinct types stops the
//! single most common class of HiDPI bug: multiplying by the scale factor twice,
//! or forgetting to multiply at all.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A logical pixel. Layout, style and widgets are all expressed in these.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Px(pub f32);

/// Shorthand constructor for [`Px`].
#[inline]
pub const fn px(value: f32) -> Px {
    Px(value)
}

/// A multiple of the root font size.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Rems(pub f32);

/// Shorthand constructor for [`Rems`].
#[inline]
pub const fn rems(value: f32) -> Rems {
    Rems(value)
}

/// A physical pixel on the display surface.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct DevicePx(pub i32);

/// The ratio of physical pixels to logical pixels for a given display.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
#[repr(transparent)]
pub struct ScaleFactor(pub f32);

impl Default for ScaleFactor {
    fn default() -> Self {
        ScaleFactor(1.0)
    }
}

impl Px {
    pub const ZERO: Px = Px(0.0);
    pub const ONE: Px = Px(1.0);
    /// Sentinel used where a dimension is unconstrained.
    pub const MAX: Px = Px(f32::MAX);

    #[inline]
    pub fn abs(self) -> Px {
        Px(self.0.abs())
    }

    #[inline]
    pub fn round(self) -> Px {
        Px(self.0.round())
    }

    #[inline]
    pub fn floor(self) -> Px {
        Px(self.0.floor())
    }

    #[inline]
    pub fn ceil(self) -> Px {
        Px(self.0.ceil())
    }

    #[inline]
    pub fn min(self, other: Px) -> Px {
        Px(self.0.min(other.0))
    }

    #[inline]
    pub fn max(self, other: Px) -> Px {
        Px(self.0.max(other.0))
    }

    #[inline]
    pub fn clamp(self, lo: Px, hi: Px) -> Px {
        Px(self.0.clamp(lo.0, hi.0))
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    /// Snap to the physical pixel grid for `scale`, so edges stay crisp.
    #[inline]
    pub fn snap(self, scale: ScaleFactor) -> Px {
        Px((self.0 * scale.0).round() / scale.0)
    }

    #[inline]
    pub fn to_device(self, scale: ScaleFactor) -> DevicePx {
        DevicePx((self.0 * scale.0).round() as i32)
    }
}

impl Rems {
    pub const ZERO: Rems = Rems(0.0);

    /// Resolve against a root font size expressed in logical pixels.
    #[inline]
    pub fn to_px(self, root_font_size: Px) -> Px {
        Px(self.0 * root_font_size.0)
    }
}

impl DevicePx {
    pub const ZERO: DevicePx = DevicePx(0);

    #[inline]
    pub fn to_px(self, scale: ScaleFactor) -> Px {
        Px(self.0 as f32 / scale.0)
    }
}

impl ScaleFactor {
    #[inline]
    pub fn get(self) -> f32 {
        self.0
    }
}

/// A length that may be absolute, relative to the parent, or automatic.
///
/// This mirrors the set of lengths the stylesheet language accepts, and is
/// lowered into the layout engine's own length type by `guirs-ui`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Length {
    /// Determined by the layout algorithm.
    #[default]
    Auto,
    /// An absolute number of logical pixels.
    Px(Px),
    /// A multiple of the root font size.
    Rems(Rems),
    /// A fraction of the parent's corresponding dimension, where 1.0 is 100%.
    Percent(f32),
    /// A share of the free space along the parent's main axis, as in `flex: n`.
    Fraction(f32),
}

impl Length {
    /// Resolve to concrete pixels where possible.
    ///
    /// `parent` is the parent's extent along the same axis. Returns `None` for
    /// lengths the layout engine must resolve itself.
    pub fn resolve(self, parent: Px, root_font_size: Px) -> Option<Px> {
        match self {
            Length::Auto | Length::Fraction(_) => None,
            Length::Px(v) => Some(v),
            Length::Rems(v) => Some(v.to_px(root_font_size)),
            Length::Percent(f) => Some(Px(parent.0 * f)),
        }
    }

    #[inline]
    pub fn is_auto(self) -> bool {
        matches!(self, Length::Auto)
    }
}

impl From<Px> for Length {
    fn from(v: Px) -> Self {
        Length::Px(v)
    }
}

impl From<Rems> for Length {
    fn from(v: Rems) -> Self {
        Length::Rems(v)
    }
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(Px(v))
    }
}

// ---------------------------------------------------------------------------
// Operator plumbing
// ---------------------------------------------------------------------------

macro_rules! float_newtype_ops {
    ($t:ident, $suffix:literal) => {
        impl Add for $t {
            type Output = $t;
            #[inline]
            fn add(self, rhs: $t) -> $t {
                $t(self.0 + rhs.0)
            }
        }
        impl Sub for $t {
            type Output = $t;
            #[inline]
            fn sub(self, rhs: $t) -> $t {
                $t(self.0 - rhs.0)
            }
        }
        impl Mul<f32> for $t {
            type Output = $t;
            #[inline]
            fn mul(self, rhs: f32) -> $t {
                $t(self.0 * rhs)
            }
        }
        impl Mul<$t> for f32 {
            type Output = $t;
            #[inline]
            fn mul(self, rhs: $t) -> $t {
                $t(self * rhs.0)
            }
        }
        impl Div<f32> for $t {
            type Output = $t;
            #[inline]
            fn div(self, rhs: f32) -> $t {
                $t(self.0 / rhs)
            }
        }
        impl Div<$t> for $t {
            type Output = f32;
            #[inline]
            fn div(self, rhs: $t) -> f32 {
                self.0 / rhs.0
            }
        }
        impl Neg for $t {
            type Output = $t;
            #[inline]
            fn neg(self) -> $t {
                $t(-self.0)
            }
        }
        impl AddAssign for $t {
            #[inline]
            fn add_assign(&mut self, rhs: $t) {
                self.0 += rhs.0;
            }
        }
        impl SubAssign for $t {
            #[inline]
            fn sub_assign(&mut self, rhs: $t) {
                self.0 -= rhs.0;
            }
        }
        impl MulAssign<f32> for $t {
            #[inline]
            fn mul_assign(&mut self, rhs: f32) {
                self.0 *= rhs;
            }
        }
        impl DivAssign<f32> for $t {
            #[inline]
            fn div_assign(&mut self, rhs: f32) {
                self.0 /= rhs;
            }
        }
        impl From<f32> for $t {
            #[inline]
            fn from(v: f32) -> $t {
                $t(v)
            }
        }
        impl From<$t> for f32 {
            #[inline]
            fn from(v: $t) -> f32 {
                v.0
            }
        }
        // Total equality and hashing over the bit pattern. NaN is never a valid
        // value for these units, so treating bit equality as Eq is sound here
        // and lets these types be used as cache keys.
        impl Eq for $t {}
        impl Hash for $t {
            #[inline]
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.0.to_bits().hash(state);
            }
        }
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", self.0, $suffix)
            }
        }
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", self.0, $suffix)
            }
        }
    };
}

float_newtype_ops!(Px, "px");
float_newtype_ops!(Rems, "rem");

impl Add for DevicePx {
    type Output = DevicePx;
    #[inline]
    fn add(self, rhs: DevicePx) -> DevicePx {
        DevicePx(self.0 + rhs.0)
    }
}

impl Sub for DevicePx {
    type Output = DevicePx;
    #[inline]
    fn sub(self, rhs: DevicePx) -> DevicePx {
        DevicePx(self.0 - rhs.0)
    }
}

impl Mul<ScaleFactor> for Px {
    type Output = DevicePx;
    #[inline]
    fn mul(self, rhs: ScaleFactor) -> DevicePx {
        DevicePx((self.0 * rhs.0).round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_conversions_round_trip() {
        let scale = ScaleFactor(2.0);
        assert_eq!(px(10.0).to_device(scale), DevicePx(20));
        assert_eq!(DevicePx(20).to_px(scale), px(10.0));
    }

    #[test]
    fn snapping_lands_on_the_physical_grid() {
        let scale = ScaleFactor(2.0);
        assert_eq!(px(10.3).snap(scale), px(10.5));
        assert_eq!(px(10.1).snap(scale), px(10.0));
    }

    #[test]
    fn percent_lengths_resolve_against_the_parent() {
        assert_eq!(
            Length::Percent(0.5).resolve(px(200.0), px(16.0)),
            Some(px(100.0))
        );
        assert_eq!(Length::Auto.resolve(px(200.0), px(16.0)), None);
        assert_eq!(
            Length::Rems(rems(1.5)).resolve(px(200.0), px(16.0)),
            Some(px(24.0))
        );
    }
}
