//! Build time helpers for guirs applications.
//!
//! One thing lives here, because one thing about a desktop application cannot
//! be done from inside it: the icon the desktop shows for the program itself,
//! in Explorer, on a pinned shortcut and in the task manager. That icon is a
//! resource inside the executable, put there when the linker runs, so nothing
//! a running program does can change it.
//!
//! Call it from the application's own `build.rs`:
//!
//! ```no_run
//! fn main() {
//!     guirs_build::icon("assets/icon.png").expect("icon");
//! }
//! ```
//!
//! and add it as a build dependency:
//!
//! ```toml
//! [build-dependencies]
//! guirs-build = "0.1"
//! ```
//!
//! There is a second icon, and it is a different one: the picture shown in a
//! window's own title bar and on its taskbar button while it runs. That one is
//! set from inside the program with `App::icon`, and an application usually
//! wants both, pointing at the same file.
//!
//! On anything other than Windows this does nothing at all, so it can be
//! called unconditionally.

// The example above is a build script, and a build script is a `fn main`.
// Showing it without one would be showing something that does not work.
#![allow(clippy::needless_doctest_main)]

use std::path::{Path, PathBuf};

use image::ImageEncoder;

mod res;

use res::{GroupEntry, Resource, RT_GROUP_ICON, RT_ICON};

/// The sizes written into the executable.
///
/// Windows picks the closest one and scales it, so what matters is covering
/// the sizes it actually asks for: small icons in Explorer's list views, the
/// standard desktop icon, the taskbar at each display scale, and the large
/// tile. Sizes it has to invent by scaling are the ones that look soft.
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Anything that can stop an icon reaching the executable.
#[derive(Debug)]
pub enum Error {
    /// The file could not be read.
    Unreadable(PathBuf, std::io::Error),
    /// The file is not a picture this can decode.
    Undecodable(PathBuf, String),
    /// The generated resource could not be written into `OUT_DIR`.
    NotWritten(PathBuf, std::io::Error),
    /// A build script asked for this outside of a build.
    NoOutDir,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreadable(path, why) => write!(f, "{} could not be read: {why}", path.display()),
            Error::Undecodable(path, why) => {
                write!(f, "{} could not be decoded: {why}", path.display())
            }
            Error::NotWritten(path, why) => {
                write!(f, "{} could not be written: {why}", path.display())
            }
            Error::NoOutDir => f.write_str("OUT_DIR is not set, so this is not a build script"),
        }
    }
}

impl std::error::Error for Error {}

/// Give the executable an icon.
///
/// Takes any picture that can be decoded, usually a square PNG. It is resized
/// to every size Windows asks for and written into the binary, so one file is
/// enough and there is no need to prepare an `.ico`.
///
/// Does nothing when building for anything other than Windows, and nothing
/// when building for Windows with the GNU toolchain, whose linker does not
/// accept a resource file. Both cases are reported as a build warning rather
/// than an error, so a cross build does not fail over an icon.
pub fn icon(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    // Cargo watches nothing by default once a build script names one path, so
    // this has to be said whether or not the icon is used on this target.
    println!("cargo:rerun-if-changed={}", path.display());

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        println!(
            "cargo:warning=the executable icon needs the MSVC toolchain, whose linker takes a \
             resource file. The window's own icon, set with App::icon, is unaffected."
        );
        return Ok(());
    }

    let bytes = std::fs::read(path).map_err(|e| Error::Unreadable(path.to_path_buf(), e))?;
    let file = build(&bytes).map_err(|why| Error::Undecodable(path.to_path_buf(), why))?;

    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|_| Error::NoOutDir)?)
        .join("guirs-icon.res");
    std::fs::write(&out, file).map_err(|e| Error::NotWritten(out.clone(), e))?;

    // Only the binaries of the package this script belongs to. A resource
    // handed to the linker for a test or a build script is at best ignored.
    println!("cargo:rustc-link-arg-bins={}", out.display());
    Ok(())
}

/// Turn a picture into the bytes of a resource file.
///
/// Split from [`icon`] so the whole thing can be exercised without a build,
/// a linker, or a file on disk.
fn build(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let source = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let source = source.to_rgba8();

    let mut resources = Vec::with_capacity(SIZES.len() + 1);
    let mut group = Vec::with_capacity(SIZES.len());

    for (index, size) in SIZES.iter().copied().enumerate() {
        // Ids start at one. Zero is legal but conventionally avoided, and the
        // group is given the id an application's main icon is expected to
        // have so that anything looking for it by number finds it.
        let id = index as u16 + 1;
        let scaled = image::imageops::resize(
            &source,
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );

        // Above 128 pixels an icon is stored as a PNG rather than as raw
        // pixels. A 256 square bitmap is a quarter of a megabyte on its own,
        // and every Windows that can display one can decode a PNG.
        let data = if size >= 256 {
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(
                    scaled.as_raw(),
                    size,
                    size,
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| e.to_string())?;
            png
        } else {
            res::icon_bitmap(size, size, scaled.as_raw())
        };

        group.push(GroupEntry {
            // The field is one byte, so the largest size is recorded as zero.
            width: if size >= 256 { 0 } else { size as u8 },
            height: if size >= 256 { 0 } else { size as u8 },
            bit_count: 32,
            bytes: data.len() as u32,
            id,
        });
        resources.push(Resource { kind: RT_ICON, id, data });
    }

    resources.push(Resource {
        kind: RT_GROUP_ICON,
        // The lowest numbered group icon is the one the desktop shows for the
        // program, so this is the one that matters.
        id: 1,
        data: res::group_icon(&group),
    });

    Ok(res::write(&resources))
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A small square PNG, built rather than stored so the test does not
    /// depend on a file that could go missing.
    fn sample() -> Vec<u8> {
        let mut image = image::RgbaImage::new(32, 32);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let edge = x < 2 || y < 2 || x >= 30 || y >= 30;
            *pixel = image::Rgba(if edge {
                [0, 0, 0, 0]
            } else {
                [(x * 8) as u8, (y * 8) as u8, 200, 255]
            });
        }
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(image.as_raw(), 32, 32, image::ExtendedColorType::Rgba8)
            .unwrap();
        png
    }

    #[test]
    fn a_picture_becomes_a_resource_file() {
        let file = build(&sample()).expect("build failed");
        assert!(file.len() > 32, "nothing but the opening entry");
        assert_eq!(file.len() % 4, 0);
    }

    #[test]
    fn every_size_windows_asks_for_is_present() {
        let file = build(&sample()).expect("build failed");
        let mut kinds = Vec::new();
        let mut at = 32; // past the opening entry
        while at + 32 <= file.len() {
            let data_size = u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            let header_size = u32::from_le_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
            kinds.push(u16::from_le_bytes(file[at + 10..at + 12].try_into().unwrap()));
            at += header_size + data_size.next_multiple_of(4);
        }
        assert_eq!(
            kinds.iter().filter(|k| **k == RT_ICON).count(),
            SIZES.len(),
            "one icon per size"
        );
        assert_eq!(
            kinds.iter().filter(|k| **k == RT_GROUP_ICON).count(),
            1,
            "exactly one group ties them together"
        );
        assert_eq!(at, file.len(), "the walk did not land exactly at the end");
    }

    #[test]
    fn the_group_names_every_icon_that_was_written() {
        let file = build(&sample()).expect("build failed");
        // The group is last, and its data follows its header.
        let mut at = 32;
        let mut group = None;
        while at + 32 <= file.len() {
            let data_size = u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            let header_size = u32::from_le_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
            let kind = u16::from_le_bytes(file[at + 10..at + 12].try_into().unwrap());
            if kind == RT_GROUP_ICON {
                group = Some(&file[at + header_size..at + header_size + data_size]);
            }
            at += header_size + data_size.next_multiple_of(4);
        }
        let group = group.expect("no group icon");
        let count = u16::from_le_bytes(group[4..6].try_into().unwrap()) as usize;
        assert_eq!(count, SIZES.len());

        let ids: Vec<u16> = (0..count)
            .map(|i| {
                let entry = 6 + i * 14;
                u16::from_le_bytes(group[entry + 12..entry + 14].try_into().unwrap())
            })
            .collect();
        assert_eq!(ids, (1..=SIZES.len() as u16).collect::<Vec<_>>());
    }

    #[test]
    fn the_largest_icon_is_stored_as_a_png() {
        let file = build(&sample()).expect("build failed");
        // A 256 square bitmap would be a quarter of a megabyte. Finding the
        // whole file smaller than that is enough to show it was compressed.
        assert!(
            file.len() < 256 * 256 * 4,
            "the largest icon was stored uncompressed: {} bytes",
            file.len()
        );
    }

    #[test]
    fn something_that_is_not_a_picture_is_refused() {
        assert!(build(b"this is not a png and never was").is_err());
    }
}
