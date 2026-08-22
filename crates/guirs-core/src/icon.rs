//! Vector icons.
//!
//! An icon is SVG path data plus the square it was drawn in. That is all of SVG
//! that an icon set actually uses, and stopping there means icons rasterize
//! through exactly the same path as glyphs: fill a coverage mask at device
//! resolution, put it in the atlas, tint it with the element's color.
//!
//! The consequence is that an icon costs no more to draw than a letter, scales
//! to any size without blurring, and picks up `color` from the cascade like
//! text does.

use std::hash::{Hash, Hasher};

use crate::id::SharedString;

/// How an icon's path is turned into coverage.
///
/// Most modern icon sets are drawn as strokes rather than filled shapes, so
/// both have to work. Filling a stroke authored path produces a blob, and
/// stroking a filled one produces an outline of the silhouette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconStyle {
    /// Fill the enclosed area.
    Fill,
    /// Trace the path with a line of this width, in view box units.
    Stroke(f32),
}

impl Eq for IconStyle {}

impl std::hash::Hash for IconStyle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            IconStyle::Fill => 0u8.hash(state),
            IconStyle::Stroke(width) => {
                1u8.hash(state);
                width.to_bits().hash(state);
            }
        }
    }
}

/// A single path icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    /// SVG path data: the `d` attribute of a `<path>`.
    path: SharedString,
    /// The width and height of the square the path was authored in. Almost
    /// every icon set uses 16 or 24.
    view_box: u16,
    style: IconStyle,
    /// Hash of everything that affects the picture, precomputed because it is
    /// the atlas cache key and is looked up on every icon on every frame.
    hash: u64,
}

impl Icon {
    /// A filled icon, from the `d` attribute of an SVG `<path>`.
    ///
    /// ```
    /// # use guirs_core::Icon;
    /// Icon::new("M6 6 L18 18 M18 6 L6 18", 24);
    /// ```
    pub fn new(path: impl Into<SharedString>, view_box: u16) -> Self {
        Icon::styled(path, view_box, IconStyle::Fill)
    }

    /// A stroked icon, as most icon sets are drawn.
    ///
    /// `width` is in view box units, so an icon authored on a 24 unit grid with
    /// a 2 unit stroke keeps its weight at any rendered size.
    pub fn stroked(path: impl Into<SharedString>, view_box: u16, width: f32) -> Self {
        Icon::styled(path, view_box, IconStyle::Stroke(width))
    }

    pub fn styled(path: impl Into<SharedString>, view_box: u16, style: IconStyle) -> Self {
        let path = path.into();
        let view_box = view_box.max(1);
        let mut hasher = crate::id::stable_hasher();
        path.as_str().hash(&mut hasher);
        view_box.hash(&mut hasher);
        style.hash(&mut hasher);
        let hash = Hasher::finish(&hasher);
        Icon {
            path,
            view_box,
            style,
            hash,
        }
    }

    #[inline]
    pub fn style(&self) -> IconStyle {
        self.style
    }

    /// The same icon drawn a different way.
    pub fn with_style(&self, style: IconStyle) -> Icon {
        Icon::styled(self.path.clone(), self.view_box, style)
    }

    #[inline]
    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    #[inline]
    pub fn view_box(&self) -> u16 {
        self.view_box
    }

    /// A stable identifier for this path, for cache keys.
    #[inline]
    pub fn key(&self) -> u64 {
        self.hash
    }

    /// The factor that maps the path's coordinates onto a box of `size`.
    #[inline]
    pub fn scale_for(&self, size: f32) -> f32 {
        size / self.view_box as f32
    }
}

impl Hash for Icon {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // The precomputed hash already folds in the path, the view box and the
        // style, which is everything that decides what gets drawn.
        self.hash.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_follows_everything_that_changes_the_picture() {
        let a = Icon::new("M0 0 L10 10", 24);
        let b = Icon::new("M0 0 L10 10", 24);

        assert_eq!(a, b);
        assert_eq!(a.key(), b.key());
        assert_ne!(a.key(), Icon::new("M0 0 L10 0", 24).key());
        assert_ne!(a.key(), Icon::new("M0 0 L10 10", 16).key());
        assert_ne!(a.key(), Icon::stroked("M0 0 L10 10", 24, 2.0).key());
        assert_ne!(
            Icon::stroked("M0 0", 24, 1.0).key(),
            Icon::stroked("M0 0", 24, 2.0).key()
        );
    }

    #[test]
    fn a_stroked_icon_remembers_its_width() {
        let icon = Icon::stroked("M0 0 L1 1", 24, 1.5);
        assert_eq!(icon.style(), IconStyle::Stroke(1.5));
        assert_eq!(icon.with_style(IconStyle::Fill).style(), IconStyle::Fill);
    }

    #[test]
    fn scaling_maps_the_view_box_onto_the_target() {
        let icon = Icon::new("M0 0", 24);
        assert_eq!(icon.scale_for(24.0), 1.0);
        assert_eq!(icon.scale_for(12.0), 0.5);
        assert_eq!(icon.scale_for(48.0), 2.0);
    }

    #[test]
    fn a_zero_view_box_cannot_divide_by_zero() {
        let icon = Icon::new("M0 0", 0);
        assert!(icon.scale_for(16.0).is_finite());
    }

    #[test]
    fn cloning_is_cheap_and_preserves_identity() {
        let icon = Icon::new("M0 0 L1 1", 24);
        let copy = icon.clone();
        assert_eq!(icon, copy);
        assert_eq!(icon.key(), copy.key());
    }
}
