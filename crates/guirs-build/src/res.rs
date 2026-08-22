//! Writing a Windows resource file by hand.
//!
//! An executable's icon is not something a running program can set. It lives
//! in the resource section of the binary, which is decided when the linker
//! runs, so the only way to put one there is to hand the linker a `.res` file.
//!
//! That file is normally produced by `rc.exe` from the Windows SDK. It is also
//! a documented and rather small binary format, so it is written here instead:
//! one fewer tool that has to be installed for a build to work, and one fewer
//! dependency for anybody using this.

/// Resource types, from `winuser.h`.
pub const RT_ICON: u16 = 3;
pub const RT_GROUP_ICON: u16 = 14;

/// What the loader is told about a resource's memory.
///
/// Ignored by every version of Windows that matters, but `rc.exe` writes these
/// and some tools read them back, so the same values are written here.
const MEM_MOVEABLE_PURE: u16 = 0x1010;
const MEM_MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;

/// US English. Resources are language tagged, and an untagged one is harder
/// for tools to find than one tagged with a language nobody filters on.
const LANG_EN_US: u16 = 0x0409;

/// One resource on its way into the binary.
pub struct Resource {
    pub kind: u16,
    pub id: u16,
    pub data: Vec<u8>,
}

/// Serialize resources into the format a linker accepts.
///
/// Every entry is a header followed by its data, and both are padded to a four
/// byte boundary. The file opens with an empty entry whose header is the only
/// thing in it, which is how a reader tells a 32 bit resource file from the 16
/// bit one that came before it.
pub fn write(resources: &[Resource]) -> Vec<u8> {
    let mut out = Vec::new();
    write_entry(&mut out, &Resource { kind: 0, id: 0, data: Vec::new() }, true);
    for resource in resources {
        write_entry(&mut out, resource, false);
    }
    out
}

fn write_entry(out: &mut Vec<u8>, resource: &Resource, null_entry: bool) {
    // Type and name are each either a string or, as here, the marker 0xFFFF
    // followed by a number. Two of those is eight bytes, which is already a
    // multiple of four, so the header never needs padding of its own.
    let header_size = 8 + 8 + 16;

    out.extend_from_slice(&(resource.data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(header_size as u32).to_le_bytes());

    // Type.
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&resource.kind.to_le_bytes());
    // Name.
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&resource.id.to_le_bytes());

    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    let flags = if null_entry {
        0
    } else if resource.kind == RT_GROUP_ICON {
        MEM_MOVEABLE_PURE_DISCARDABLE
    } else {
        MEM_MOVEABLE_PURE
    };
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&if null_entry { 0 } else { LANG_EN_US }.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics

    out.extend_from_slice(&resource.data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// One icon in the directory that ties a group together.
pub struct GroupEntry {
    /// Zero means 256, which is why an icon cannot be larger than that.
    pub width: u8,
    pub height: u8,
    pub bit_count: u16,
    pub bytes: u32,
    pub id: u16,
}

/// The directory Windows reads to pick which size of an icon to draw.
///
/// The same shape as the header of an `.ico` file, except that each entry ends
/// with the number of a resource rather than an offset into a file. That one
/// difference is the whole reason an `.ico` cannot simply be pasted in.
pub fn group_icon(entries: &[GroupEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + entries.len() * 14);
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // 1 for icons, 2 for cursors
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for entry in entries {
        out.push(entry.width);
        out.push(entry.height);
        out.push(0); // colours in the palette, zero above eight bits
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&entry.bit_count.to_le_bytes());
        out.extend_from_slice(&entry.bytes.to_le_bytes());
        out.extend_from_slice(&entry.id.to_le_bytes());
    }
    out
}

/// Pack pixels into the bitmap layout an icon resource holds.
///
/// Three things about this are not obvious and all three will produce an icon
/// that looks wrong rather than one that fails to build: the declared height is
/// twice the real height, because the header covers the colours and the mask
/// together; rows run bottom to top; and the channel order is blue, green,
/// red, alpha.
pub fn icon_bitmap(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mask_stride = (width as usize).div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + (width * height * 4) as usize + mask_stride * height as usize);

    out.extend_from_slice(&40u32.to_le_bytes()); // header size
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&((height * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&0u32.to_le_bytes()); // uncompressed
    out.extend_from_slice(&0u32.to_le_bytes()); // image size, may be zero
    out.extend_from_slice(&0i32.to_le_bytes()); // pixels per metre across
    out.extend_from_slice(&0i32.to_le_bytes()); // pixels per metre down
    out.extend_from_slice(&0u32.to_le_bytes()); // palette entries used
    out.extend_from_slice(&0u32.to_le_bytes()); // palette entries needed

    for y in (0..height).rev() {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            out.push(rgba[i + 2]);
            out.push(rgba[i + 1]);
            out.push(rgba[i]);
            out.push(rgba[i + 3]);
        }
    }

    // The mask predates transparency and is one bit a pixel, set where the
    // icon should not be drawn. Windows composites from the alpha channel
    // instead, but the mask still has to be here and still has to be the right
    // size, and a version of Windows old enough to read it should find
    // something sensible rather than a solid block.
    for y in (0..height).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in 0..width {
            let alpha = rgba[((y * width + x) * 4 + 3) as usize];
            if alpha < 128 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resource_file_opens_with_an_empty_entry() {
        let out = write(&[]);
        // Thirty two bytes of header describing nothing. A reader that does
        // not find this treats the file as the older sixteen bit format.
        assert_eq!(out.len(), 32);
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 0, "data size");
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()), 32, "header size");
    }

    #[test]
    fn every_entry_starts_on_a_four_byte_boundary() {
        // A linker walks this file by adding sizes together, so an entry whose
        // data is not padded puts every later entry at the wrong offset.
        let odd = Resource { kind: RT_ICON, id: 1, data: vec![7; 13] };
        let next = Resource { kind: RT_ICON, id: 2, data: vec![9; 3] };
        let out = write(&[odd, next]);
        assert_eq!(out.len() % 4, 0);

        let second = 32 + 32 + 16; // null entry, first header, padded 13 bytes
        assert_eq!(second % 4, 0);
        assert_eq!(
            u32::from_le_bytes(out[second..second + 4].try_into().unwrap()),
            3,
            "the second entry is not where its header says it is"
        );
    }

    #[test]
    fn a_group_entry_is_fourteen_bytes() {
        // Sixteen in an .ico file and fourteen here, because the last field is
        // a resource number rather than an offset. Getting this wrong shifts
        // every entry after the first and Windows draws nothing.
        let group = group_icon(&[
            GroupEntry { width: 16, height: 16, bit_count: 32, bytes: 100, id: 1 },
            GroupEntry { width: 32, height: 32, bit_count: 32, bytes: 400, id: 2 },
        ]);
        assert_eq!(group.len(), 6 + 2 * 14);
        assert_eq!(u16::from_le_bytes(group[4..6].try_into().unwrap()), 2, "count");
        assert_eq!(u16::from_le_bytes(group[18..20].try_into().unwrap()), 1, "first id");
        assert_eq!(group[20], 32, "second entry does not start where it should");
    }

    #[test]
    fn a_full_size_icon_is_recorded_as_zero() {
        // The field is one byte, so 256 does not fit in it and is written as
        // zero. An icon recorded as 256 would be read as being one pixel wide.
        let group = group_icon(&[GroupEntry {
            width: 0,
            height: 0,
            bit_count: 32,
            bytes: 10,
            id: 1,
        }]);
        assert_eq!(group[6], 0);
        assert_eq!(group[7], 0);
    }

    #[test]
    fn a_bitmap_declares_twice_the_height_it_has() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let bitmap = icon_bitmap(4, 4, &rgba);
        let height = i32::from_le_bytes(bitmap[8..12].try_into().unwrap());
        assert_eq!(height, 8, "the mask is part of the declared height");
        let width = i32::from_le_bytes(bitmap[4..8].try_into().unwrap());
        assert_eq!(width, 4);
    }

    #[test]
    fn a_bitmap_is_bottom_up_and_blue_first() {
        // One red pixel on the top row, one blue on the bottom.
        let mut rgba = vec![0u8; 2 * 2 * 4];
        rgba[0..4].copy_from_slice(&[255, 0, 0, 255]); // top left, red
        rgba[8..12].copy_from_slice(&[0, 0, 255, 255]); // bottom left, blue
        let bitmap = icon_bitmap(2, 2, &rgba);

        // The first pixel written is the bottom left one, in BGRA.
        assert_eq!(&bitmap[40..44], &[255, 0, 0, 255], "expected blue, stored BGRA");
        // The top left one comes last, on the second row.
        assert_eq!(&bitmap[48..52], &[0, 0, 255, 255], "expected red, stored BGRA");
    }

    #[test]
    fn the_mask_is_padded_to_four_bytes_a_row() {
        // A row of a 16 pixel icon is two bytes of mask and has to be padded
        // to four, or the mask ends short and Windows reads past it.
        let rgba = vec![255u8; 16 * 16 * 4];
        let bitmap = icon_bitmap(16, 16, &rgba);
        assert_eq!(bitmap.len(), 40 + 16 * 16 * 4 + 4 * 16);
    }

    #[test]
    fn the_mask_marks_what_is_transparent() {
        let mut rgba = vec![255u8; 8 * 8 * 4];
        for x in 0..4 {
            rgba[x * 4 + 3] = 0; // left half of the top row, fully clear
        }
        let bitmap = icon_bitmap(8, 8, &rgba);
        let mask = &bitmap[40 + 8 * 8 * 4..];
        // Rows are bottom up, so the top row is the last one in the mask.
        assert_eq!(mask[mask.len() - 4], 0b1111_0000);
    }
}
