//! Texture atlas allocation.
//!
//! Glyphs and images are packed into shared textures so that drawing a screen
//! full of text is one draw call rather than thousands. Packing uses the shelf
//! algorithm: rectangles are placed in horizontal bands, each band as tall as
//! the first rectangle that opened it.
//!
//! Shelf packing is not the tightest algorithm available, but glyphs at a given
//! size are all roughly the same height, which is exactly the case it handles
//! well. It is also O(shelves) per insert with no bookkeeping between frames.

use std::collections::HashMap;
use std::hash::Hash;

/// Padding kept around every entry.
///
/// Without it, bilinear sampling at the edge of one glyph can pick up a
/// neighbour's texels and leave a faint fringe.
const GUTTER: u32 = 1;

/// A rectangle reserved inside one atlas page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AtlasSlot {
    pub page: u16,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasSlot {
    /// Normalized texture coordinates for this slot: u0, v0, u1, v1.
    pub fn uv(&self, atlas_size: u32) -> [f32; 4] {
        let s = atlas_size as f32;
        [
            self.x as f32 / s,
            self.y as f32 / s,
            (self.x + self.width) as f32 / s,
            (self.y + self.height) as f32 / s,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
struct Shelf {
    y: u32,
    height: u32,
    cursor: u32,
}

/// Shelf packer for a single square page.
#[derive(Debug)]
pub struct PageAllocator {
    size: u32,
    shelves: Vec<Shelf>,
    used: u32,
}

impl PageAllocator {
    pub fn new(size: u32) -> Self {
        PageAllocator {
            size,
            shelves: Vec::new(),
            used: 0,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// Fraction of the page's area handed out so far.
    pub fn occupancy(&self) -> f32 {
        self.used as f32 / (self.size * self.size) as f32
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
        self.used = 0;
    }

    /// Reserve space, or `None` when the page is full.
    pub fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let padded_width = width + GUTTER * 2;
        let padded_height = height + GUTTER * 2;
        if padded_width > self.size || padded_height > self.size {
            return None;
        }

        // Reuse the shortest shelf that fits. Refusing shelves much taller than
        // needed stops one tall glyph from wasting a whole band.
        let mut best: Option<usize> = None;
        for (index, shelf) in self.shelves.iter().enumerate() {
            if shelf.height < padded_height {
                continue;
            }
            if shelf.cursor + padded_width > self.size {
                continue;
            }
            if shelf.height > padded_height * 2 && shelf.height > padded_height + 8 {
                continue;
            }
            match best {
                Some(current) if self.shelves[current].height <= shelf.height => {}
                _ => best = Some(index),
            }
        }

        let shelf_index = match best {
            Some(index) => index,
            None => {
                let next_y = self
                    .shelves
                    .last()
                    .map(|s| s.y + s.height)
                    .unwrap_or(0);
                if next_y + padded_height > self.size {
                    return None;
                }
                self.shelves.push(Shelf {
                    y: next_y,
                    height: padded_height,
                    cursor: 0,
                });
                self.shelves.len() - 1
            }
        };

        let shelf = &mut self.shelves[shelf_index];
        let x = shelf.cursor;
        let y = shelf.y;
        shelf.cursor += padded_width;
        self.used += padded_width * padded_height;
        Some((x + GUTTER, y + GUTTER))
    }
}

/// A multi page atlas keyed by whatever identifies an entry.
#[derive(Debug)]
pub struct Atlas<K: Eq + Hash + Clone> {
    pages: Vec<PageAllocator>,
    entries: HashMap<K, AtlasSlot>,
    page_size: u32,
    max_pages: usize,
    /// Pages added since the last time the renderer synced its textures.
    pending_pages: usize,
}

impl<K: Eq + Hash + Clone> Atlas<K> {
    pub fn new(page_size: u32, max_pages: usize) -> Self {
        Atlas {
            pages: vec![PageAllocator::new(page_size)],
            entries: HashMap::new(),
            page_size,
            max_pages: max_pages.max(1),
            pending_pages: 1,
        }
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn get(&self, key: &K) -> Option<AtlasSlot> {
        self.entries.get(key).copied()
    }

    /// Take the count of pages created since the last call, so the caller can
    /// create matching GPU textures.
    pub fn take_pending_pages(&mut self) -> usize {
        std::mem::take(&mut self.pending_pages)
    }

    /// Reserve a slot for `key`, returning the existing one if present.
    ///
    /// Returns `None` only when every page is full and no more may be added.
    pub fn insert(&mut self, key: K, width: u32, height: u32) -> Option<AtlasSlot> {
        if let Some(existing) = self.entries.get(&key) {
            return Some(*existing);
        }

        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = page.allocate(width, height) {
                let slot = AtlasSlot {
                    page: index as u16,
                    x,
                    y,
                    width,
                    height,
                };
                self.entries.insert(key, slot);
                return Some(slot);
            }
        }

        if self.pages.len() >= self.max_pages {
            return None;
        }

        let mut page = PageAllocator::new(self.page_size);
        let (x, y) = page.allocate(width, height)?;
        let slot = AtlasSlot {
            page: self.pages.len() as u16,
            x,
            y,
            width,
            height,
        };
        self.pages.push(page);
        self.pending_pages += 1;
        self.entries.insert(key, slot);
        Some(slot)
    }

    /// Forget every entry and reset packing.
    ///
    /// The GPU textures keep their contents until overwritten, which is
    /// harmless because nothing refers to the stale texels any more.
    pub fn reset(&mut self) {
        self.entries.clear();
        for page in &mut self.pages {
            page.clear();
        }
    }

    /// Whether the atlas is full enough that a reset is worth considering.
    pub fn is_under_pressure(&self) -> bool {
        self.pages.len() >= self.max_pages
            && self.pages.iter().all(|page| page.occupancy() > 0.9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_stay_inside_the_page() {
        let mut page = PageAllocator::new(64);
        for _ in 0..40 {
            if let Some((x, y)) = page.allocate(10, 10) {
                assert!(x + 10 <= 64, "x overflow at {x}");
                assert!(y + 10 <= 64, "y overflow at {y}");
            }
        }
    }

    #[test]
    fn allocations_never_overlap() {
        let mut page = PageAllocator::new(128);
        let mut placed: Vec<(u32, u32, u32, u32)> = Vec::new();
        let sizes = [(8, 12), (16, 12), (4, 12), (20, 24), (10, 24), (6, 8)];
        for i in 0..60 {
            let (w, h) = sizes[i % sizes.len()];
            if let Some((x, y)) = page.allocate(w, h) {
                for (px, py, pw, ph) in &placed {
                    let disjoint =
                        x + w <= *px || *px + *pw <= x || y + h <= *py || *py + *ph <= y;
                    assert!(
                        disjoint,
                        "({x},{y},{w},{h}) overlaps ({px},{py},{pw},{ph})"
                    );
                }
                placed.push((x, y, w, h));
            }
        }
        assert!(placed.len() > 20, "packed too few: {}", placed.len());
    }

    #[test]
    fn oversized_entries_are_refused_rather_than_clipped() {
        let mut page = PageAllocator::new(64);
        assert_eq!(page.allocate(64, 64), None);
        assert_eq!(page.allocate(100, 10), None);
        assert!(page.allocate(60, 60).is_some());
    }

    #[test]
    fn a_full_page_stops_allocating() {
        let mut page = PageAllocator::new(32);
        let mut count = 0;
        while page.allocate(10, 10).is_some() {
            count += 1;
            assert!(count < 100, "allocator never reported full");
        }
        assert!(count > 0);
    }

    #[test]
    fn repeated_keys_return_the_same_slot() {
        let mut atlas: Atlas<u32> = Atlas::new(256, 2);
        let first = atlas.insert(7, 10, 10).unwrap();
        let second = atlas.insert(7, 10, 10).unwrap();
        assert_eq!(first, second);
        assert_eq!(atlas.entry_count(), 1);
    }

    #[test]
    fn the_atlas_grows_onto_new_pages() {
        let mut atlas: Atlas<u32> = Atlas::new(64, 4);
        let mut pages_seen = std::collections::HashSet::new();
        for i in 0..200u32 {
            if let Some(slot) = atlas.insert(i, 20, 20) {
                pages_seen.insert(slot.page);
            }
        }
        assert!(pages_seen.len() > 1, "never spilled onto a second page");
        assert!(atlas.page_count() <= 4);
    }

    #[test]
    fn the_atlas_refuses_to_exceed_its_page_budget() {
        let mut atlas: Atlas<u32> = Atlas::new(64, 1);
        let mut refused = false;
        for i in 0..500u32 {
            if atlas.insert(i, 30, 30).is_none() {
                refused = true;
                break;
            }
        }
        assert!(refused);
        assert_eq!(atlas.page_count(), 1);
    }

    #[test]
    fn uv_coordinates_are_normalized() {
        let slot = AtlasSlot {
            page: 0,
            x: 64,
            y: 128,
            width: 32,
            height: 16,
        };
        let uv = slot.uv(256);
        assert_eq!(uv, [0.25, 0.5, 0.375, 0.5625]);
    }

    #[test]
    fn resetting_frees_everything() {
        let mut atlas: Atlas<u32> = Atlas::new(64, 1);
        for i in 0..5u32 {
            atlas.insert(i, 10, 10);
        }
        assert_eq!(atlas.entry_count(), 5);
        atlas.reset();
        assert_eq!(atlas.entry_count(), 0);
        assert!(atlas.insert(99, 10, 10).is_some());
    }

    #[test]
    fn new_pages_are_reported_once() {
        let mut atlas: Atlas<u32> = Atlas::new(64, 4);
        assert_eq!(atlas.take_pending_pages(), 1);
        assert_eq!(atlas.take_pending_pages(), 0);
        for i in 0..200u32 {
            atlas.insert(i, 20, 20);
        }
        assert!(atlas.take_pending_pages() >= 1);
        assert_eq!(atlas.take_pending_pages(), 0);
    }
}
