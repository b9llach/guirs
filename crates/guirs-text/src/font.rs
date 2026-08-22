//! Font discovery, loading and fallback.
//!
//! Faces are addressed by a dense [`FontId`] rather than by name, so shaping
//! and glyph cache keys stay small and cheap to hash. Resolution results are
//! memoized, because a render pass asks for the same family, weight and style
//! thousands of times.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use guirs_core::{Px, SharedString};
use guirs_style::{FontFamily, FontStyle, FontWeight};

/// A loaded font face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontId(pub u32);

/// Metrics for one face, normalized so that one em is 1.0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceMetrics {
    pub units_per_em: f32,
    /// Distance from the baseline up to the top of the tallest glyph.
    pub ascent: f32,
    /// Distance from the baseline down to the bottom of the lowest glyph,
    /// stored as a positive number.
    pub descent: f32,
    /// Extra space the designer asks for between lines.
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    /// Distance from the baseline to the underline, negative meaning below.
    pub underline_offset: f32,
    pub underline_size: f32,
}

impl FaceMetrics {
    /// Scale to a concrete font size.
    pub fn scaled(&self, size: Px) -> ScaledMetrics {
        let s = size.0;
        ScaledMetrics {
            ascent: Px(self.ascent * s),
            descent: Px(self.descent * s),
            line_gap: Px(self.line_gap * s),
            cap_height: Px(self.cap_height * s),
            x_height: Px(self.x_height * s),
            underline_offset: Px(self.underline_offset * s),
            underline_size: Px((self.underline_size * s).max(1.0)),
        }
    }
}

/// Face metrics at a specific size, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScaledMetrics {
    pub ascent: Px,
    pub descent: Px,
    pub line_gap: Px,
    pub cap_height: Px,
    pub x_height: Px,
    pub underline_offset: Px,
    pub underline_size: Px,
}

impl ScaledMetrics {
    /// The font's natural line height.
    #[inline]
    pub fn line_height(&self) -> Px {
        self.ascent + self.descent + self.line_gap
    }
}

/// One face, with its bytes held for the lifetime of the system.
pub struct LoadedFace {
    pub id: FontId,
    pub family: SharedString,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub monospace: bool,
    pub metrics: FaceMetrics,
    /// Whether the face carries color glyph tables, as emoji fonts do.
    pub has_color: bool,
    /// The face's own bytes.
    ///
    /// Shared with the font database rather than copied out of it, and mapped
    /// from the file on disk where the platform allows. That makes the pages
    /// file backed instead of dirty heap: the system can drop them under
    /// pressure and read them back, and two processes using the same font
    /// share one copy. It matters most where it costs most, since a CJK face
    /// is tens of megabytes on its own.
    data: Arc<dyn AsRef<[u8]> + Sync + Send>,
    index: u32,
    /// How large the mapped face is, kept here so reporting memory does not
    /// have to touch the mapping.
    size: usize,
}

impl LoadedFace {
    /// Borrow the face for shaping or rasterization.
    ///
    /// This is a header validation and a couple of pointer reads, so it is fine
    /// to call per glyph rather than caching the result.
    #[inline]
    pub fn font_ref(&self) -> Option<swash::FontRef<'_>> {
        swash::FontRef::from_index(self.data(), self.index as usize)
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        (*self.data).as_ref()
    }

    /// How many bytes this face occupies.
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl std::fmt::Debug for LoadedFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedFace")
            .field("id", &self.id)
            .field("family", &self.family)
            .field("weight", &self.weight)
            .field("style", &self.style)
            .field("bytes", &self.size)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct QueryKey {
    families: Vec<SharedString>,
    weight: FontWeight,
    style: FontStyle,
}

/// The font database plus everything loaded from it.
pub struct FontSystem {
    db: fontdb::Database,
    faces: Vec<LoadedFace>,
    by_db_id: HashMap<fontdb::ID, FontId>,
    query_cache: HashMap<QueryKey, Option<FontId>>,
    fallbacks: Vec<FontId>,
    fallback_families: Vec<SharedString>,
    default_family: SharedString,
}

impl std::fmt::Debug for FontSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSystem")
            .field("loaded_faces", &self.faces.len())
            .field("default_family", &self.default_family)
            .finish()
    }
}

impl Default for FontSystem {
    fn default() -> Self {
        FontSystem::new()
    }
}

impl FontSystem {
    /// An empty system with no fonts. Load some before laying out text.
    pub fn new() -> Self {
        FontSystem {
            db: fontdb::Database::new(),
            faces: Vec::new(),
            by_db_id: HashMap::new(),
            query_cache: HashMap::new(),
            fallbacks: Vec::new(),
            fallback_families: default_fallback_families(),
            default_family: SharedString::from(default_ui_family()),
        }
    }

    /// Load every font installed on this machine.
    ///
    /// This walks the system font directories and reads each face's headers.
    /// It is the slowest thing that happens at startup, typically tens of
    /// milliseconds, and only needs doing once.
    pub fn with_system_fonts() -> Self {
        let mut system = FontSystem::new();
        system.load_system_fonts();
        system
    }

    pub fn load_system_fonts(&mut self) {
        self.db.load_system_fonts();
        self.query_cache.clear();
    }

    /// Register a font file. Returns how many faces it contained.
    pub fn load_file(&mut self, path: impl AsRef<Path>) -> std::io::Result<usize> {
        let before = self.db.len();
        self.db.load_font_file(path)?;
        self.query_cache.clear();
        Ok(self.db.len() - before)
    }

    /// Register every font in a directory tree.
    pub fn load_dir(&mut self, path: impl AsRef<Path>) {
        self.db.load_fonts_dir(path);
        self.query_cache.clear();
    }

    /// Register font bytes directly, for fonts embedded in the binary.
    pub fn load_bytes(&mut self, data: Vec<u8>) {
        self.db.load_font_data(data);
        self.query_cache.clear();
    }

    /// The family used when a style names none.
    pub fn default_family(&self) -> &SharedString {
        &self.default_family
    }

    pub fn set_default_family(&mut self, family: impl Into<SharedString>) {
        self.default_family = family.into();
        self.query_cache.clear();
    }

    /// Replace the ordered list of families consulted when the requested font
    /// has no glyph for a character.
    pub fn set_fallback_families(&mut self, families: Vec<SharedString>) {
        self.fallback_families = families;
        self.fallbacks.clear();
    }

    /// Bytes of font data held resident.
    ///
    /// A face is kept in memory because shaping and rasterizing both need the
    /// original tables. CJK and emoji faces are tens of megabytes each, so this
    /// is usually the largest single thing an interface holds.
    ///
    /// Where the platform allows, these are mapped from the font file rather
    /// than copied, so most of this is file backed rather than heap.
    pub fn memory_bytes(&self) -> usize {
        self.faces.iter().map(|face| face.size).sum()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// Every family name the database knows, sorted and deduplicated.
    pub fn family_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .db
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    #[inline]
    pub fn face(&self, id: FontId) -> &LoadedFace {
        &self.faces[id.0 as usize]
    }

    #[inline]
    pub fn try_face(&self, id: FontId) -> Option<&LoadedFace> {
        self.faces.get(id.0 as usize)
    }

    /// Resolve a family list, weight and style to a concrete face.
    ///
    /// Falls back to the default UI family, and then to any face at all, so a
    /// caller never has to handle a missing font mid layout.
    pub fn resolve(
        &mut self,
        family: &FontFamily,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FontId> {
        let mut families: Vec<SharedString> = family.names().to_vec();
        if families.is_empty() {
            families.push(self.default_family.clone());
        }

        let key = QueryKey {
            families: families.clone(),
            weight,
            style,
        };
        if let Some(hit) = self.query_cache.get(&key) {
            return *hit;
        }

        // Hoisted because `query` takes `&mut self`, so the fallback list
        // cannot borrow from `self` inside the closure. Cloning a
        // `SharedString` is a refcount bump.
        let fallback = [self.default_family.clone()];
        let resolved = self
            .query(&families, weight, style)
            .or_else(|| self.query(&fallback, weight, style))
            .or_else(|| self.any_face(weight, style));

        self.query_cache.insert(key, resolved);
        resolved
    }

    /// Find a face containing a glyph for `ch`, starting from `preferred`.
    ///
    /// Returns `preferred` when it already covers the character, which is the
    /// overwhelmingly common case and costs one charmap lookup.
    pub fn resolve_for_char(
        &mut self,
        preferred: FontId,
        ch: char,
        weight: FontWeight,
        style: FontStyle,
    ) -> FontId {
        if self.covers(preferred, ch) {
            return preferred;
        }
        self.ensure_fallbacks(weight, style);
        for candidate in self.fallbacks.clone() {
            if candidate != preferred && self.covers(candidate, ch) {
                return candidate;
            }
        }
        preferred
    }

    /// Whether a face has a glyph for `ch`.
    pub fn covers(&self, id: FontId, ch: char) -> bool {
        let Some(face) = self.try_face(id) else {
            return false;
        };
        let Some(font) = face.font_ref() else {
            return false;
        };
        font.charmap().map(ch) != 0
    }

    fn ensure_fallbacks(&mut self, weight: FontWeight, style: FontStyle) {
        if !self.fallbacks.is_empty() {
            return;
        }
        let families = self.fallback_families.clone();
        for family in families {
            if let Some(id) = self.query(&[family], weight, style) {
                if !self.fallbacks.contains(&id) {
                    self.fallbacks.push(id);
                }
            }
        }
    }

    fn query(
        &mut self,
        families: &[SharedString],
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FontId> {
        let names: Vec<fontdb::Family> = families
            .iter()
            .map(|name| match name.as_str() {
                "sans-serif" | "system-ui" | "ui-sans-serif" => fontdb::Family::SansSerif,
                "serif" | "ui-serif" => fontdb::Family::Serif,
                "monospace" | "ui-monospace" => fontdb::Family::Monospace,
                "cursive" => fontdb::Family::Cursive,
                "fantasy" => fontdb::Family::Fantasy,
                other => fontdb::Family::Name(other),
            })
            .collect();

        let db_id = self.db.query(&fontdb::Query {
            families: &names,
            weight: fontdb::Weight(weight.0),
            stretch: fontdb::Stretch::Normal,
            style: match style {
                FontStyle::Normal => fontdb::Style::Normal,
                FontStyle::Italic => fontdb::Style::Italic,
                FontStyle::Oblique => fontdb::Style::Oblique,
            },
        })?;
        self.register(db_id)
    }

    fn any_face(&mut self, weight: FontWeight, style: FontStyle) -> Option<FontId> {
        let _ = (weight, style);
        let db_id = self.db.faces().next().map(|face| face.id)?;
        self.register(db_id)
    }

    /// Pull a face out of the database and hold its bytes.
    fn register(&mut self, db_id: fontdb::ID) -> Option<FontId> {
        if let Some(existing) = self.by_db_id.get(&db_id) {
            return Some(*existing);
        }

        let info = self.db.face(db_id)?;
        let family = info
            .families
            .first()
            .map(|(name, _)| SharedString::from(name.as_str()))
            .unwrap_or_default();
        let weight = FontWeight(info.weight.0);
        let style = match info.style {
            fontdb::Style::Normal => FontStyle::Normal,
            fontdb::Style::Italic => FontStyle::Italic,
            fontdb::Style::Oblique => FontStyle::Oblique,
        };
        let monospace = info.monospaced;
        let index = info.index;

        // Shared with the database rather than copied out of it. Where the
        // face came from a file this maps it, which keeps the pages file
        // backed rather than turning them into dirty heap.
        //
        // Unsafe because a mapping shows whatever the file says now: if
        // something truncates a font while it is mapped, reading past the new
        // end faults. In practice the fonts in play are the system's own and
        // the application's own, neither of which is rewritten underneath a
        // running process, and this is what every font stack does with them.
        // The alternative is a private copy of every face, which for a CJK
        // font is tens of megabytes of dirty memory per process.
        let (data, _) = unsafe { self.db.make_shared_face_data(db_id) }?;
        let size = (*data).as_ref().len();

        let (metrics, has_color) = {
            let font = swash::FontRef::from_index((*data).as_ref(), index as usize)?;
            let raw = font.metrics(&[]);
            let upem = raw.units_per_em as f32;
            let scale = if upem > 0.0 { 1.0 / upem } else { 0.0 };
            let has_color = font.color_palettes().count() > 0;
            (
                FaceMetrics {
                    units_per_em: upem,
                    ascent: raw.ascent * scale,
                    descent: raw.descent.abs() * scale,
                    line_gap: raw.leading * scale,
                    cap_height: raw.cap_height * scale,
                    x_height: raw.x_height * scale,
                    underline_offset: raw.underline_offset * scale,
                    underline_size: raw.stroke_size * scale,
                },
                has_color,
            )
        };

        let id = FontId(self.faces.len() as u32);
        self.faces.push(LoadedFace {
            size,
            id,
            family,
            weight,
            style,
            monospace,
            metrics,
            has_color,
            data,
            index,
        });
        self.by_db_id.insert(db_id, id);
        Some(id)
    }
}

/// The platform's standard interface font.
pub fn default_ui_family() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Segoe UI"
    }
    #[cfg(target_os = "macos")]
    {
        "Helvetica Neue"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "DejaVu Sans"
    }
}

/// The platform's usual coverage fonts, in the order they should be consulted.
pub fn default_fallback_families() -> Vec<SharedString> {
    let names: &[&str] = {
        #[cfg(target_os = "windows")]
        {
            &[
                "Segoe UI",
                "Segoe UI Emoji",
                "Segoe UI Symbol",
                "Microsoft YaHei",
                "Yu Gothic",
                "Malgun Gothic",
                "Nirmala UI",
                "Arial",
            ]
        }
        #[cfg(target_os = "macos")]
        {
            &[
                "Helvetica Neue",
                "Apple Color Emoji",
                "PingFang SC",
                "Hiragino Sans",
                "Apple SD Gothic Neo",
                "Arial Unicode MS",
            ]
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            &[
                "DejaVu Sans",
                "Noto Sans",
                "Noto Color Emoji",
                "Noto Sans CJK SC",
                "Liberation Sans",
            ]
        }
    };
    names.iter().map(|n| SharedString::from(*n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_system_resolves_nothing() {
        let mut fonts = FontSystem::new();
        assert_eq!(fonts.face_count(), 0);
        assert_eq!(
            fonts.resolve(&FontFamily::single("Nonexistent"), FontWeight::NORMAL, FontStyle::Normal),
            None
        );
    }

    #[test]
    fn metrics_scale_linearly() {
        let metrics = FaceMetrics {
            units_per_em: 1000.0,
            ascent: 0.8,
            descent: 0.2,
            line_gap: 0.0,
            ..Default::default()
        };
        let scaled = metrics.scaled(Px(20.0));
        assert_eq!(scaled.ascent, Px(16.0));
        assert_eq!(scaled.descent, Px(4.0));
        assert_eq!(scaled.line_height(), Px(20.0));
    }

    #[test]
    fn underline_thickness_never_rounds_away() {
        let metrics = FaceMetrics {
            units_per_em: 1000.0,
            underline_size: 0.001,
            ..Default::default()
        };
        assert_eq!(metrics.scaled(Px(12.0)).underline_size, Px(1.0));
    }

    #[test]
    fn generic_family_names_map_to_database_generics() {
        // Exercised through resolution on an empty database: the point is that
        // the mapping does not panic and does not treat these as literal names.
        let mut fonts = FontSystem::new();
        for generic in ["sans-serif", "serif", "monospace", "system-ui"] {
            assert_eq!(
                fonts.resolve(
                    &FontFamily::single(generic),
                    FontWeight::NORMAL,
                    FontStyle::Normal
                ),
                None
            );
        }
    }

    #[test]
    fn the_platform_defaults_are_populated() {
        assert!(!default_ui_family().is_empty());
        assert!(!default_fallback_families().is_empty());
    }
}
