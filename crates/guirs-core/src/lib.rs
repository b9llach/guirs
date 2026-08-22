//! Core vocabulary types shared by every layer of guirs.
//!
//! This crate deliberately has almost no dependencies. Everything above it
//! (style, text, render, ui) speaks in the types defined here, which keeps the
//! layers decoupled and makes it possible to swap a renderer or a text stack
//! without touching the rest of the framework.
//!
//! * [`units`] and [`geometry`] are the measurements: [`Px`], [`Rems`],
//!   [`Length`], points, sizes, rectangles, edges and corners.
//! * [`color`] and [`paint`] are what things are filled with: [`Rgba`],
//!   [`Hsla`], gradients, borders and shadows.
//! * [`transform`] is the affine pair: [`Transform2D`] is what a stylesheet
//!   writes and what an animation interpolates, [`Affine`] is the matrix a
//!   vertex shader and hit testing need.
//! * [`id`] is element identity, and [`SharedString`], which is what makes
//!   rebuilding a tree every frame affordable.
//! * [`icon`] is a vector glyph, kept here because both the renderer and the
//!   element layer need to name one.

pub mod color;
pub mod geometry;
pub mod icon;
pub mod id;
pub mod paint;
pub mod transform;
pub mod units;

pub use color::{Hsla, Rgba};
pub use geometry::{Axis, Bounds, Corners, Edges, Point, Scalar, Size};
pub use icon::{Icon, IconStyle};
pub use id::{ElementId, GlobalElementId, SharedString};
pub use paint::{BorderStyle, BoxShadow, Gradient, GradientStop, Paint};
pub use transform::{Affine, Transform2D};
pub use units::{px, rems, DevicePx, Length, Px, Rems, ScaleFactor};

/// Linear interpolation between two `f32` values.
#[inline]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Clamp `t` into the inclusive `0.0..=1.0` range.
#[inline]
pub fn saturate(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}
