//! Color.
//!
//! Colors are stored as non linear sRGB components in the `0.0..=1.0` range,
//! which is the space CSS authors think in and the space gradients interpolate
//! in. Conversion to linear light happens once, in the fragment shader, right
//! before the value is written to an sRGB surface.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::lerp_f32;

/// A non linear sRGB color with straight (non premultiplied) alpha.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Rgba = Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Rgba { r, g, b, a }
    }

    /// From 8 bit components.
    #[inline]
    pub fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// From 8 bit components including alpha.
    #[inline]
    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// From a packed `0xRRGGBB` literal, fully opaque.
    #[inline]
    pub fn hex(value: u32) -> Self {
        Rgba::rgb8(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        )
    }

    /// From a packed `0xRRGGBBAA` literal.
    #[inline]
    pub fn hexa(value: u32) -> Self {
        Rgba::rgba8(
            ((value >> 24) & 0xff) as u8,
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        )
    }

    /// Parse a CSS style hex literal or a named color.
    ///
    /// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` and the common CSS color
    /// keywords. Returns `None` for anything else, leaving function notation
    /// such as `rgb()` and `hsl()` to the stylesheet parser.
    pub fn parse(input: &str) -> Option<Rgba> {
        let s = input.trim();
        if let Some(hex) = s.strip_prefix('#') {
            return Rgba::parse_hex_digits(hex);
        }
        named(&s.to_ascii_lowercase())
    }

    fn parse_hex_digits(hex: &str) -> Option<Rgba> {
        fn nib(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            }
        }
        let bytes = hex.as_bytes();
        let mut d = [0u8; 8];
        for (i, b) in bytes.iter().enumerate() {
            if i >= 8 {
                return None;
            }
            d[i] = nib(*b)?;
        }
        match bytes.len() {
            3 => Some(Rgba::rgba8(
                d[0] * 17,
                d[1] * 17,
                d[2] * 17,
                255,
            )),
            4 => Some(Rgba::rgba8(d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17)),
            6 => Some(Rgba::rgba8(
                d[0] * 16 + d[1],
                d[2] * 16 + d[3],
                d[4] * 16 + d[5],
                255,
            )),
            8 => Some(Rgba::rgba8(
                d[0] * 16 + d[1],
                d[2] * 16 + d[3],
                d[4] * 16 + d[5],
                d[6] * 16 + d[7],
            )),
            _ => None,
        }
    }

    /// Pack into `0xRRGGBBAA`.
    #[inline]
    pub fn to_u32(self) -> u32 {
        let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
        (q(self.r) << 24) | (q(self.g) << 16) | (q(self.b) << 8) | q(self.a)
    }

    /// Components as an array, ready for a vertex buffer.
    #[inline]
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    #[inline]
    pub fn is_transparent(self) -> bool {
        self.a <= 0.0009
    }

    #[inline]
    pub fn is_opaque(self) -> bool {
        self.a >= 0.9991
    }

    /// Replace the alpha channel.
    #[inline]
    pub fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }

    /// Scale the existing alpha, as a group opacity would.
    #[inline]
    pub fn fade(mut self, opacity: f32) -> Self {
        self.a *= opacity;
        self
    }

    /// Interpolate componentwise. `t` of 0 yields `self`.
    #[inline]
    pub fn lerp(self, other: Rgba, t: f32) -> Rgba {
        Rgba {
            r: lerp_f32(self.r, other.r, t),
            g: lerp_f32(self.g, other.g, t),
            b: lerp_f32(self.b, other.b, t),
            a: lerp_f32(self.a, other.a, t),
        }
    }

    /// Source over compositing of `self` on top of `under`.
    pub fn over(self, under: Rgba) -> Rgba {
        let a = self.a + under.a * (1.0 - self.a);
        if a <= 0.0 {
            return Rgba::TRANSPARENT;
        }
        let f = |top: f32, bot: f32| (top * self.a + bot * under.a * (1.0 - self.a)) / a;
        Rgba {
            r: f(self.r, under.r),
            g: f(self.g, under.g),
            b: f(self.b, under.b),
            a,
        }
    }

    /// Move toward white by `amount`, expressed as a fraction of the remaining
    /// headroom in HSL lightness.
    pub fn lighten(self, amount: f32) -> Rgba {
        let mut hsl = Hsla::from(self);
        hsl.l = (hsl.l + (1.0 - hsl.l) * amount).clamp(0.0, 1.0);
        hsl.into()
    }

    /// Move toward black by `amount`.
    pub fn darken(self, amount: f32) -> Rgba {
        let mut hsl = Hsla::from(self);
        hsl.l = (hsl.l * (1.0 - amount)).clamp(0.0, 1.0);
        hsl.into()
    }

    /// Perceptual luminance, used to pick readable foreground colors.
    pub fn luminance(self) -> f32 {
        fn channel(c: f32) -> f32 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// Black or white, whichever reads better on this color.
    pub fn contrasting(self) -> Rgba {
        if self.luminance() > 0.179 {
            Rgba::BLACK
        } else {
            Rgba::WHITE
        }
    }

    /// Convert to linear light. The shader normally does this, but CPU side
    /// compositing needs it too.
    pub fn to_linear(self) -> [f32; 4] {
        fn c(v: f32) -> f32 {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        [c(self.r), c(self.g), c(self.b), self.a]
    }
}

impl Eq for Rgba {}

impl Hash for Rgba {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.r.to_bits().hash(state);
        self.g.to_bits().hash(state);
        self.b.to_bits().hash(state);
        self.a.to_bits().hash(state);
    }
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:08x}", self.to_u32())
    }
}

impl From<u32> for Rgba {
    fn from(v: u32) -> Rgba {
        Rgba::hex(v)
    }
}

/// Shorthand for an opaque color from a `0xRRGGBB` literal.
#[inline]
pub fn rgb(value: u32) -> Rgba {
    Rgba::hex(value)
}

/// Shorthand for a color from a `0xRRGGBBAA` literal.
#[inline]
pub fn rgba(value: u32) -> Rgba {
    Rgba::hexa(value)
}

// ---------------------------------------------------------------------------
// HSL
// ---------------------------------------------------------------------------

/// Hue, saturation, lightness, alpha. Hue is in turns, `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Hsla {
    pub h: f32,
    pub s: f32,
    pub l: f32,
    pub a: f32,
}

impl Hsla {
    #[inline]
    pub const fn new(h: f32, s: f32, l: f32, a: f32) -> Self {
        Hsla { h, s, l, a }
    }

    /// From degrees and percentages, matching CSS `hsl()` argument order.
    #[inline]
    pub fn deg(h: f32, s: f32, l: f32, a: f32) -> Self {
        Hsla {
            h: (h / 360.0).rem_euclid(1.0),
            s: s.clamp(0.0, 1.0),
            l: l.clamp(0.0, 1.0),
            a: a.clamp(0.0, 1.0),
        }
    }
}

impl From<Rgba> for Hsla {
    fn from(c: Rgba) -> Hsla {
        let max = c.r.max(c.g).max(c.b);
        let min = c.r.min(c.g).min(c.b);
        let l = (max + min) / 2.0;
        let delta = max - min;

        if delta.abs() < f32::EPSILON {
            return Hsla {
                h: 0.0,
                s: 0.0,
                l,
                a: c.a,
            };
        }

        let s = if l > 0.5 {
            delta / (2.0 - max - min)
        } else {
            delta / (max + min)
        };

        let h = if max == c.r {
            ((c.g - c.b) / delta).rem_euclid(6.0)
        } else if max == c.g {
            (c.b - c.r) / delta + 2.0
        } else {
            (c.r - c.g) / delta + 4.0
        } / 6.0;

        Hsla {
            h: h.rem_euclid(1.0),
            s,
            l,
            a: c.a,
        }
    }
}

impl From<Hsla> for Rgba {
    fn from(c: Hsla) -> Rgba {
        if c.s.abs() < f32::EPSILON {
            return Rgba {
                r: c.l,
                g: c.l,
                b: c.l,
                a: c.a,
            };
        }

        fn hue_to_channel(p: f32, q: f32, mut t: f32) -> f32 {
            t = t.rem_euclid(1.0);
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 1.0 / 2.0 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        }

        let q = if c.l < 0.5 {
            c.l * (1.0 + c.s)
        } else {
            c.l + c.s - c.l * c.s
        };
        let p = 2.0 * c.l - q;

        Rgba {
            r: hue_to_channel(p, q, c.h + 1.0 / 3.0),
            g: hue_to_channel(p, q, c.h),
            b: hue_to_channel(p, q, c.h - 1.0 / 3.0),
            a: c.a,
        }
    }
}

// ---------------------------------------------------------------------------
// Named colors
// ---------------------------------------------------------------------------

/// Resolve a CSS color keyword. The input must already be lowercase.
pub fn named(name: &str) -> Option<Rgba> {
    let v: u32 = match name {
        "transparent" => return Some(Rgba::TRANSPARENT),
        "currentcolor" => return None,
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        "silver" => 0xc0c0c0,
        "gray" | "grey" => 0x808080,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "green" => 0x008000,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "navy" => 0x000080,
        "orange" => 0xffa500,
        "orangered" => 0xff4500,
        "gold" => 0xffd700,
        "pink" => 0xffc0cb,
        "hotpink" => 0xff69b4,
        "crimson" => 0xdc143c,
        "salmon" => 0xfa8072,
        "coral" => 0xff7f50,
        "tomato" => 0xff6347,
        "indianred" => 0xcd5c5c,
        "brown" => 0xa52a2a,
        "sienna" => 0xa0522d,
        "chocolate" => 0xd2691e,
        "peru" => 0xcd853f,
        "tan" => 0xd2b48c,
        "beige" => 0xf5f5dc,
        "wheat" => 0xf5deb3,
        "khaki" => 0xf0e68c,
        "ivory" => 0xfffff0,
        "linen" => 0xfaf0e6,
        "snow" => 0xfffafa,
        "seashell" => 0xfff5ee,
        "lavender" => 0xe6e6fa,
        "plum" => 0xdda0dd,
        "violet" => 0xee82ee,
        "orchid" => 0xda70d6,
        "indigo" => 0x4b0082,
        "slateblue" => 0x6a5acd,
        "royalblue" => 0x4169e1,
        "dodgerblue" => 0x1e90ff,
        "deepskyblue" => 0x00bfff,
        "skyblue" => 0x87ceeb,
        "steelblue" => 0x4682b4,
        "cadetblue" => 0x5f9ea0,
        "turquoise" => 0x40e0d0,
        "aquamarine" => 0x7fffd4,
        "seagreen" => 0x2e8b57,
        "forestgreen" => 0x228b22,
        "darkgreen" => 0x006400,
        "olivedrab" => 0x6b8e23,
        "yellowgreen" => 0x9acd32,
        "springgreen" => 0x00ff7f,
        "darkred" => 0x8b0000,
        "darkblue" => 0x00008b,
        "darkcyan" => 0x008b8b,
        "darkmagenta" => 0x8b008b,
        "darkorange" => 0xff8c00,
        "darkviolet" => 0x9400d3,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "dimgray" | "dimgrey" => 0x696969,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "gainsboro" => 0xdcdcdc,
        "whitesmoke" => 0xf5f5f5,
        "slategray" | "slategrey" => 0x708090,
        "midnightblue" => 0x191970,
        "rebeccapurple" => 0x663399,
        _ => return None,
    };
    Some(Rgba::hex(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_all_parse() {
        assert_eq!(Rgba::parse("#fff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse("#ffffff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse("#ffffffff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse("#000"), Some(Rgba::BLACK));
        let half = Rgba::parse("#00000080").unwrap();
        assert!((half.a - 0.502).abs() < 0.01);
        assert_eq!(Rgba::parse("#xyz"), None);
        assert_eq!(Rgba::parse("#12345"), None);
    }

    #[test]
    fn named_colors_resolve_case_insensitively() {
        assert_eq!(Rgba::parse("RebeccaPurple"), Some(Rgba::hex(0x663399)));
        assert_eq!(Rgba::parse("transparent"), Some(Rgba::TRANSPARENT));
        assert_eq!(Rgba::parse("notacolor"), None);
    }

    #[test]
    fn hsl_round_trips() {
        for raw in [0x6c5ce7u32, 0xff0000, 0x00ff00, 0x123456, 0x808080] {
            let c = Rgba::hex(raw);
            let back: Rgba = Hsla::from(c).into();
            assert!((c.r - back.r).abs() < 0.002, "r for {raw:06x}");
            assert!((c.g - back.g).abs() < 0.002, "g for {raw:06x}");
            assert!((c.b - back.b).abs() < 0.002, "b for {raw:06x}");
        }
    }

    #[test]
    fn lighten_and_darken_move_in_the_right_direction() {
        let base = Rgba::hex(0x6c5ce7);
        assert!(base.lighten(0.2).luminance() > base.luminance());
        assert!(base.darken(0.2).luminance() < base.luminance());
    }

    #[test]
    fn contrasting_picks_a_readable_foreground() {
        assert_eq!(Rgba::WHITE.contrasting(), Rgba::BLACK);
        assert_eq!(Rgba::BLACK.contrasting(), Rgba::WHITE);
    }

    #[test]
    fn opaque_source_over_replaces_the_backdrop() {
        let out = Rgba::hex(0xff0000).over(Rgba::hex(0x0000ff));
        assert_eq!(out, Rgba::hex(0xff0000));
    }
}
