//! Pictures, and the element that draws them.
//!
//! An image is the one thing an interface shows that it cannot draw for
//! itself. It has to be found, decoded, and handed to the graphics device, and
//! none of that can happen while a frame is being drawn: decoding a photograph
//! takes tens of milliseconds, which is several frames' worth of budget for
//! one picture.
//!
//! So loading is a background job and drawing is not. [`ImageStore`] holds
//! what has been asked for and what state it is in; an [`Img`] asks the store
//! for a picture every frame and draws it once it is there. A picture that is
//! still loading occupies its space and draws nothing, and the frame it
//! becomes ready is the frame the window is asked to draw again.
//!
//! The store lives on the window, so two elements showing the same file decode
//! it once and share one texture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use guirs_core::{Bounds, ElementId, GlobalElementId, Px, Rgba, SharedString, Size};
use guirs_render::{ImageId, NO_CROP};
use guirs_style::{ComputedStyle, StateFlags, Style, Styled};
use smallvec::SmallVec;

use crate::context::{content_box, Cx};
use crate::element::Element;
use crate::layout::LayoutId;

// ---------------------------------------------------------------------------
// Where a picture comes from
// ---------------------------------------------------------------------------

/// Where an image is loaded from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// A file, read and decoded off the drawing thread.
    Path(Arc<PathBuf>),
    /// Bytes already in the binary, usually from `include_bytes!`.
    ///
    /// Still decoded off the drawing thread: the bytes are compressed, and
    /// turning them into pixels is the expensive half.
    Bytes(&'static [u8]),
}

impl ImageSource {
    /// What this source is called in the store.
    ///
    /// Two elements naming the same file share one decode and one texture.
    /// Static bytes are identified by where they are rather than by their
    /// contents, because hashing a picture to find out it is the same picture
    /// costs more than it saves.
    fn key(&self) -> ImageKey {
        match self {
            ImageSource::Path(path) => ImageKey::Path(path.clone()),
            ImageSource::Bytes(bytes) => ImageKey::Bytes(bytes.as_ptr() as usize, bytes.len()),
        }
    }
}

impl From<&str> for ImageSource {
    fn from(path: &str) -> Self {
        ImageSource::Path(Arc::new(PathBuf::from(path)))
    }
}

impl From<String> for ImageSource {
    fn from(path: String) -> Self {
        ImageSource::Path(Arc::new(PathBuf::from(path)))
    }
}

impl From<&Path> for ImageSource {
    fn from(path: &Path) -> Self {
        ImageSource::Path(Arc::new(path.to_path_buf()))
    }
}

impl From<PathBuf> for ImageSource {
    fn from(path: PathBuf) -> Self {
        ImageSource::Path(Arc::new(path))
    }
}

impl From<&'static [u8]> for ImageSource {
    fn from(bytes: &'static [u8]) -> Self {
        ImageSource::Bytes(bytes)
    }
}

impl<const N: usize> From<&'static [u8; N]> for ImageSource {
    fn from(bytes: &'static [u8; N]) -> Self {
        ImageSource::Bytes(bytes.as_slice())
    }
}

/// What the store files a picture under.
///
/// Opaque on purpose: it is handed back to the store after an upload and is
/// not meant to be constructed or read anywhere else.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageKey {
    Path(Arc<PathBuf>),
    Bytes(usize, usize),
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Why a picture could not be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// The file could not be read.
    Unreadable(String),
    /// The bytes are not a picture this build can decode.
    Undecodable(String),
    /// Decoded, but too large for the texture atlas to hold.
    TooLarge { width: u32, height: u32 },
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Unreadable(why) => write!(f, "could not be read: {why}"),
            ImageError::Undecodable(why) => write!(f, "could not be decoded: {why}"),
            ImageError::TooLarge { width, height } => {
                write!(f, "{width}x{height} does not fit in the atlas")
            }
        }
    }
}

impl std::error::Error for ImageError {}

/// A decoded picture, waiting to be handed to the graphics device.
pub struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Decoded {
    fn bytes(&self) -> usize {
        self.rgba.len()
    }

    /// The picture's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Four bytes a pixel, in the layout the atlas takes.
    pub fn pixels(&self) -> &[u8] {
        &self.rgba
    }
}

enum Entry {
    /// Being read and decoded off the drawing thread.
    Loading(crate::task::Task<Result<Decoded, ImageError>>),
    /// Decoded, waiting for the window to upload it.
    Decoded(Decoded),
    /// On the graphics device and ready to draw.
    Ready { id: ImageId, size: (u32, u32) },
    Failed(ImageError),
}

/// What an image is doing, from the point of view of something drawing it.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageStatus {
    /// Being read or decoded. Nothing to draw yet.
    Loading,
    /// Ready, with its size in pixels.
    Ready { id: ImageId, size: (u32, u32) },
    /// It will not arrive.
    Failed(ImageError),
}

/// Every picture a window has been asked for.
///
/// Shared by every element in the window, so the same file named twice is read
/// once, decoded once, and held once.
#[derive(Default)]
pub struct ImageStore {
    entries: HashMap<ImageKey, Entry>,
    /// Set when something finished loading, so the window knows to upload and
    /// draw again without having to ask every entry.
    changed: bool,
}

impl ImageStore {
    /// Ask for a picture, starting the work if this is the first time.
    ///
    /// Called every frame by everything drawing an image, so the common path
    /// is one hash lookup.
    pub fn request(&mut self, source: &ImageSource) -> ImageStatus {
        let key = source.key();

        // Anything already settled answers without touching the source.
        match self.entries.get(&key) {
            Some(Entry::Ready { id, size }) => {
                return ImageStatus::Ready {
                    id: *id,
                    size: *size,
                }
            }
            Some(Entry::Failed(error)) => return ImageStatus::Failed(error.clone()),
            Some(Entry::Decoded(_)) => return ImageStatus::Loading,
            Some(Entry::Loading(task)) => {
                if !task.is_ready() {
                    return ImageStatus::Loading;
                }
            }
            None => {
                let source = source.clone();
                let task = crate::task::spawn(move || decode(&source));
                self.entries.insert(key, Entry::Loading(task));
                return ImageStatus::Loading;
            }
        }

        // Finished since the last frame. Taken out of the task here rather
        // than in the upload pass, so a picture nothing is drawing any more
        // does not keep a thread's result alive.
        let Some(Entry::Loading(task)) = self.entries.get(&key) else {
            return ImageStatus::Loading;
        };
        match task.take() {
            Some(Ok(decoded)) => {
                self.entries.insert(key, Entry::Decoded(decoded));
                self.changed = true;
                ImageStatus::Loading
            }
            Some(Err(error)) => {
                self.entries.insert(key, Entry::Failed(error.clone()));
                self.changed = true;
                ImageStatus::Failed(error)
            }
            // Cancelled, or already taken. Neither can happen from here, but
            // reporting a failure is better than asking again forever.
            None => {
                let error = ImageError::Undecodable("loading did not finish".into());
                self.entries.insert(key, Entry::Failed(error.clone()));
                ImageStatus::Failed(error)
            }
        }
    }

    /// Everything decoded and waiting for the graphics device.
    ///
    /// The window calls this between painting and rendering, which is the one
    /// point in a frame where both the pictures and the device are reachable.
    pub(crate) fn take_decoded(&mut self) -> Vec<(ImageKey, Decoded)> {
        if !self.changed {
            return Vec::new();
        }
        let ready: Vec<ImageKey> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry, Entry::Decoded(_)))
            .map(|(key, _)| key.clone())
            .collect();

        let mut out = Vec::with_capacity(ready.len());
        for key in ready {
            if let Some(Entry::Decoded(decoded)) = self.entries.remove(&key) {
                out.push((key, decoded));
            }
        }
        self.changed = false;
        out
    }

    pub(crate) fn uploaded(&mut self, key: ImageKey, id: ImageId, size: (u32, u32)) {
        self.entries.insert(key, Entry::Ready { id, size });
    }

    pub(crate) fn upload_failed(&mut self, key: ImageKey, error: ImageError) {
        self.entries.insert(key, Entry::Failed(error));
    }

    /// Whether anything finished loading and is waiting to be drawn.
    pub fn has_pending(&self) -> bool {
        self.changed
    }

    /// Decoded pixels held on the heap waiting to be uploaded.
    ///
    /// Normally zero: pixels live here for the part of one frame between
    /// decoding and upload, and belong to the graphics device after that.
    pub fn pending_bytes(&self) -> usize {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                Entry::Decoded(decoded) => Some(decoded.bytes()),
                _ => None,
            })
            .sum()
    }

    /// How many pictures the window is holding, in any state.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Read and decode one picture. Runs off the drawing thread.
fn decode(source: &ImageSource) -> Result<Decoded, ImageError> {
    let bytes = match source {
        ImageSource::Path(path) => std::fs::read(path.as_ref())
            .map_err(|error| ImageError::Unreadable(error.to_string()))?
            .into(),
        ImageSource::Bytes(bytes) => std::borrow::Cow::Borrowed(*bytes),
    };

    let mut decoded = image::load_from_memory(&bytes)
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;

    // A picture larger than a page cannot be held at all, so it is shrunk to
    // fit rather than refused. A photograph straight from a camera is several
    // times the size of any box it will be drawn into, so this usually costs
    // nothing anybody can see and saves a great deal of texture memory.
    let limit = guirs_render::IMAGE_ATLAS_SIZE;
    if decoded.width() > limit || decoded.height() > limit {
        let (width, height) = (decoded.width(), decoded.height());
        decoded = decoded.resize(limit, limit, image::imageops::FilterType::Lanczos3);
        log::debug!(
            "image of {width}x{height} shrunk to {}x{} to fit the atlas",
            decoded.width(),
            decoded.height()
        );
    }

    // Straight to the layout the atlas takes. Anything else would mean a
    // second pass over every pixel at upload time.
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Decoded {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

/// Decode a picture into the form the desktop wants for a window's icon.
///
/// Synchronous, unlike everything else here. An icon is small, it is wanted
/// once before the window is on screen, and a window that appears without one
/// and gains it a moment later flickers in the taskbar.
pub fn load_icon(source: &ImageSource) -> Result<winit::window::Icon, ImageError> {
    let decoded = decode(source)?;
    winit::window::Icon::from_rgba(decoded.rgba, decoded.width, decoded.height).map_err(|error| {
        ImageError::Undecodable(format!("not usable as an icon: {error}"))
    })
}

// ---------------------------------------------------------------------------
// Fitting a picture into a box
// ---------------------------------------------------------------------------

/// What to do when a picture and the box it is given are different shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectFit {
    /// Fill the box, changing the picture's shape to match. The only option
    /// that distorts, which is why it is not the default.
    Fill,
    /// Fit the whole picture inside the box, leaving space along one axis.
    #[default]
    Contain,
    /// Fill the box with the picture's shape kept, cropping the overflow.
    /// What a thumbnail or an avatar wants.
    Cover,
    /// Draw at the picture's own size, centered, cropped if it does not fit.
    None,
}

/// Where to draw, and what part of the picture to draw there.
///
/// Split out from painting so the arithmetic can be tested without a window,
/// a graphics device or a file.
pub fn fit(fit: ObjectFit, box_size: Size<Px>, image: (u32, u32)) -> (Bounds<Px>, [f32; 4]) {
    let full = Bounds::from_xywh(Px::ZERO, Px::ZERO, box_size.width, box_size.height);
    let (width, height) = (image.0 as f32, image.1 as f32);
    if width <= 0.0 || height <= 0.0 || box_size.width <= Px::ZERO || box_size.height <= Px::ZERO {
        return (full, NO_CROP);
    }

    let box_width = box_size.width.0;
    let box_height = box_size.height.0;

    match fit {
        ObjectFit::Fill => (full, NO_CROP),

        ObjectFit::Contain => {
            let scale = (box_width / width).min(box_height / height);
            (centered(box_size, width * scale, height * scale), NO_CROP)
        }

        ObjectFit::Cover => {
            // The box is filled, so the picture is cropped instead. The
            // fraction kept along each axis is how much of the picture the box
            // can show once it has been scaled to cover.
            let scale = (box_width / width).max(box_height / height);
            let kept_u = (box_width / (width * scale)).min(1.0);
            let kept_v = (box_height / (height * scale)).min(1.0);
            let u0 = (1.0 - kept_u) * 0.5;
            let v0 = (1.0 - kept_v) * 0.5;
            (full, [u0, v0, u0 + kept_u, v0 + kept_v])
        }

        ObjectFit::None => {
            let kept_u = (box_width / width).min(1.0);
            let kept_v = (box_height / height).min(1.0);
            let u0 = (1.0 - kept_u) * 0.5;
            let v0 = (1.0 - kept_v) * 0.5;
            (
                centered(box_size, width.min(box_width), height.min(box_height)),
                [u0, v0, u0 + kept_u, v0 + kept_v],
            )
        }
    }
}

/// The shape a picture should impose on its box, if any.
///
/// Only when exactly one side was left open, so the other can be worked out
/// from it. Given both, the author decided the shape and a picture must not
/// argue with it; given neither, the picture's own size already is the answer.
fn aspect_for(width_open: bool, height_open: bool, natural: Option<(u32, u32)>) -> Option<f32> {
    if width_open == height_open {
        return None;
    }
    let (width, height) = natural?;
    if height == 0 {
        return None;
    }
    Some(width as f32 / height as f32)
}

fn centered(box_size: Size<Px>, width: f32, height: f32) -> Bounds<Px> {
    Bounds::from_xywh(
        Px((box_size.width.0 - width) * 0.5),
        Px((box_size.height.0 - height) * 0.5),
        Px(width),
        Px(height),
    )
}

// ---------------------------------------------------------------------------
// The element
// ---------------------------------------------------------------------------

/// A picture.
///
/// ```no_run
/// # use guirs_style::Styled;
/// # use guirs_core::Px;
/// # use guirs_ui::image::{img, ObjectFit};
/// img("assets/photo.jpg").w(Px(320.0)).h(Px(180.0)).fit(ObjectFit::Cover);
/// ```
pub struct Img {
    source: ImageSource,
    object_fit: ObjectFit,
    alt: Option<SharedString>,
    element_type: SharedString,
    id: Option<ElementId>,
    classes: SmallVec<[SharedString; 4]>,
    style: Style,

    global_id: GlobalElementId,
    computed: ComputedStyle,
    node: Option<LayoutId>,
    /// The size of the picture once it is known, so layout can use its shape.
    natural: Option<(u32, u32)>,
}

/// Draw a picture, from a path or from bytes already in the binary.
///
/// ```no_run
/// # use guirs_ui::image::img;
/// img("assets/logo.png");
/// img(include_bytes!("../../../LICENSE-MIT"));
/// ```
pub fn img(source: impl Into<ImageSource>) -> Img {
    Img::new(source)
}

impl Img {
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Img {
            source: source.into(),
            object_fit: ObjectFit::default(),
            alt: None,
            element_type: SharedString::from("img"),
            id: None,
            classes: SmallVec::new(),
            style: Style::new(),
            global_id: GlobalElementId::ROOT,
            computed: ComputedStyle::default(),
            node: None,
            natural: None,
        }
    }

    /// What to do when the picture and its box are different shapes.
    pub fn fit(mut self, fit: ObjectFit) -> Self {
        self.object_fit = fit;
        self
    }

    /// What this picture shows, for anyone who cannot see it.
    ///
    /// Without this a picture is described as an image and nothing more. Leave
    /// it off only for decoration, which is better marked with
    /// [`Img::decorative`].
    pub fn alt(mut self, text: impl Into<SharedString>) -> Self {
        self.alt = Some(text.into());
        self
    }

    /// Leave this picture out of the interface's description.
    ///
    /// For pictures that carry nothing: a background texture, a flourish, a
    /// spacer. Announcing them interrupts without informing.
    pub fn decorative(mut self) -> Self {
        self.alt = None;
        self.element_type = SharedString::from("img-decorative");
        self
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one or more classes, separated by whitespace.
    pub fn class(mut self, class: impl Into<SharedString>) -> Self {
        crate::div::push_classes(&mut self.classes, class.into());
        self
    }

    pub fn class_if(self, condition: bool, class: impl Into<SharedString>) -> Self {
        if condition {
            self.class(class)
        } else {
            self
        }
    }

    /// The size a picture takes when nothing sizes it: its own.
    ///
    /// A picture whose size is not known yet takes no space, so a layout does
    /// not jump when one arrives at a different shape than a guess would have
    /// been.
    fn intrinsic(&self) -> Size<Px> {
        match self.natural {
            Some((width, height)) => Size::new(Px(width as f32), Px(height as f32)),
            None => Size::new(Px::ZERO, Px::ZERO),
        }
    }

    fn is_decorative(&self) -> bool {
        self.element_type.as_ref() == "img-decorative"
    }
}

impl Styled for Img {
    #[inline]
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl Element for Img {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        self.global_id = cx.begin_element(
            self.id.as_ref(),
            &self.element_type,
            &self.classes,
            false,
            StateFlags::EMPTY,
        );
        self.computed = cx.compute_style(&self.style);
        cx.end_element();

        // Asked for here rather than at paint, so the frame that learns a
        // picture's shape is the frame that lays it out.
        if let ImageStatus::Ready { size, .. } = cx.images.request(&self.source) {
            self.natural = Some(size);
        }

        if self.computed.aspect_ratio.is_none() {
            self.computed.aspect_ratio = aspect_for(
                matches!(self.computed.size.width, guirs_style::Length::Auto),
                matches!(self.computed.size.height, guirs_style::Length::Auto),
                self.natural,
            );
        }

        let node = cx.layout.add_fixed_node(&self.computed, self.intrinsic());
        self.node = Some(node);
        node
    }

    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        if !crate::context::is_visible(&self.computed) {
            return;
        }

        cx.push_opacity(self.computed.opacity);
        let opacity = cx.opacity();
        let content = content_box(bounds, &self.computed, cx.root_font_size);

        if let ImageStatus::Ready { id, size } = cx.images.request(&self.source) {
            let (placed, crop) = fit(self.object_fit, content.size, size);
            let drawn = Bounds::from_xywh(
                content.origin.x + placed.origin.x,
                content.origin.y + placed.origin.y,
                placed.size.width,
                placed.size.height,
            );
            if drawn.size.width > Px::ZERO && drawn.size.height > Px::ZERO {
                cx.scene.push_image(guirs_render::Image {
                    id,
                    bounds: drawn,
                    corner_radii: self.computed.border_radius.clamp_to(drawn.size),
                    // White, so the picture comes through as it is. Opacity is
                    // the only thing multiplied in.
                    tint: Rgba::WHITE.fade(opacity),
                    crop,
                });
            }
        }

        // Described whatever state it is in, so a picture that has not arrived
        // is still something a reader can find rather than a hole.
        if self.is_decorative() {
            cx.access.suppress();
        }
        if cx
            .access
            .push(self.global_id, crate::access::AccessRole::Image, bounds)
            .is_some()
        {
            if let Some(node) = cx.access.current_mut() {
                node.label = self.alt.clone();
            }
            cx.access.pop();
        }
        if self.is_decorative() {
            cx.access.resume();
        }

        cx.register_hit(self.global_id, bounds, self.computed.cursor, false, false);
        cx.pop_opacity();
    }
}

crate::impl_into_element!(Img);

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: f32, height: f32) -> Size<Px> {
        Size::new(Px(width), Px(height))
    }

    #[test]
    fn fill_uses_the_whole_box_and_the_whole_picture() {
        let (bounds, crop) = fit(ObjectFit::Fill, size(100.0, 50.0), (200, 200));
        assert_eq!(bounds.size, size(100.0, 50.0));
        assert_eq!(crop, NO_CROP);
    }

    #[test]
    fn contain_keeps_the_shape_and_leaves_space() {
        // A square picture in a wide box touches the top and bottom, and is
        // centered in the space left over.
        let (bounds, crop) = fit(ObjectFit::Contain, size(100.0, 50.0), (200, 200));
        assert_eq!(bounds.size, size(50.0, 50.0));
        assert_eq!(bounds.origin.x, Px(25.0));
        assert_eq!(bounds.origin.y, Px::ZERO);
        assert_eq!(crop, NO_CROP, "contain must not crop");
    }

    #[test]
    fn cover_fills_the_box_by_cropping() {
        // A square picture in a wide box fills it, and loses the top and
        // bottom rather than being squashed.
        let (bounds, crop) = fit(ObjectFit::Cover, size(100.0, 50.0), (200, 200));
        assert_eq!(bounds.size, size(100.0, 50.0), "cover must fill the box");
        assert_eq!(crop[0], 0.0, "nothing is lost across");
        assert_eq!(crop[2], 1.0);
        // Half the height is kept, taken from the middle.
        assert!((crop[1] - 0.25).abs() < 0.001, "{crop:?}");
        assert!((crop[3] - 0.75).abs() < 0.001, "{crop:?}");
    }

    #[test]
    fn cover_of_a_matching_shape_crops_nothing() {
        let (bounds, crop) = fit(ObjectFit::Cover, size(100.0, 50.0), (200, 100));
        assert_eq!(bounds.size, size(100.0, 50.0));
        assert_eq!(crop, NO_CROP);
    }

    #[test]
    fn none_draws_at_its_own_size() {
        let (bounds, crop) = fit(ObjectFit::None, size(100.0, 100.0), (40, 20));
        assert_eq!(bounds.size, size(40.0, 20.0));
        assert_eq!(bounds.origin.x, Px(30.0), "not centered across");
        assert_eq!(bounds.origin.y, Px(40.0), "not centered down");
        assert_eq!(crop, NO_CROP);
    }

    #[test]
    fn none_crops_a_picture_larger_than_its_box() {
        let (bounds, crop) = fit(ObjectFit::None, size(50.0, 50.0), (100, 100));
        assert_eq!(bounds.size, size(50.0, 50.0));
        // Half of each axis, from the middle.
        assert!((crop[0] - 0.25).abs() < 0.001, "{crop:?}");
        assert!((crop[2] - 0.75).abs() < 0.001, "{crop:?}");
    }

    #[test]
    fn nothing_is_divided_by_zero() {
        // A picture with no area, or a box with none, has to come out as
        // something rather than as a NaN that reaches the shader.
        for (box_size, image) in [
            (size(100.0, 50.0), (0, 0)),
            (size(0.0, 0.0), (10, 10)),
            (size(100.0, 0.0), (10, 10)),
        ] {
            for mode in [
                ObjectFit::Fill,
                ObjectFit::Contain,
                ObjectFit::Cover,
                ObjectFit::None,
            ] {
                let (bounds, crop) = fit(mode, box_size, image);
                assert!(bounds.size.width.0.is_finite(), "{mode:?} {image:?}");
                assert!(bounds.size.height.0.is_finite(), "{mode:?} {image:?}");
                assert!(crop.iter().all(|value| value.is_finite()));
            }
        }
    }

    #[test]
    fn a_picture_does_not_argue_with_a_size_it_was_given() {
        let natural = Some((320, 180));
        // Both sides given: the box is the box, whatever shape the picture is.
        // Getting this wrong resizes every explicitly sized picture on screen.
        assert_eq!(aspect_for(false, false, natural), None);
        // Neither given: the picture's own size already answers it.
        assert_eq!(aspect_for(true, true, natural), None);
        // One given: the other follows from the picture's shape.
        assert_eq!(aspect_for(true, false, natural), Some(320.0 / 180.0));
        assert_eq!(aspect_for(false, true, natural), Some(320.0 / 180.0));
    }

    #[test]
    fn a_picture_that_has_not_arrived_imposes_no_shape() {
        assert_eq!(aspect_for(false, true, None), None);
        // And one with no height cannot produce a ratio at all.
        assert_eq!(aspect_for(false, true, Some((320, 0))), None);
    }

    #[test]
    fn the_same_file_named_twice_is_one_entry() {
        let mut store = ImageStore::default();
        let one = ImageSource::from("a/b.png");
        let two = ImageSource::from("a/b.png");
        store.request(&one);
        store.request(&two);
        assert_eq!(store.len(), 1, "the same picture was loaded twice");
    }

    #[test]
    fn the_same_bytes_are_one_entry() {
        static PIXELS: &[u8] = b"not really a picture";
        let mut store = ImageStore::default();
        store.request(&ImageSource::Bytes(PIXELS));
        store.request(&ImageSource::Bytes(PIXELS));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_missing_file_settles_on_a_failure() {
        let mut store = ImageStore::default();
        let source = ImageSource::from("does/not/exist.png");
        assert_eq!(store.request(&source), ImageStatus::Loading);

        // Asked for until it settles, the way a frame would.
        let mut status = ImageStatus::Loading;
        for _ in 0..200 {
            status = store.request(&source);
            if status != ImageStatus::Loading {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            matches!(status, ImageStatus::Failed(ImageError::Unreadable(_))),
            "a missing file never settled: {status:?}"
        );

        // And it stays settled rather than being asked for again every frame.
        assert!(matches!(store.request(&source), ImageStatus::Failed(_)));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn bytes_that_are_not_a_picture_settle_on_a_failure() {
        static NOISE: &[u8] = b"this is not a png and never was";
        let mut store = ImageStore::default();
        let source = ImageSource::Bytes(NOISE);

        let mut status = ImageStatus::Loading;
        for _ in 0..200 {
            status = store.request(&source);
            if status != ImageStatus::Loading {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            matches!(status, ImageStatus::Failed(ImageError::Undecodable(_))),
            "undecodable bytes never settled: {status:?}"
        );
    }

    #[test]
    fn a_real_picture_decodes_and_waits_to_be_uploaded() {
        let mut store = ImageStore::default();
        let source = ImageSource::Bytes(PNG_2X1);

        let mut settled = false;
        for _ in 0..200 {
            store.request(&source);
            if store.has_pending() {
                settled = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(settled, "a valid picture never finished decoding");

        let decoded = store.take_decoded();
        assert_eq!(decoded.len(), 1);
        let (_, picture) = &decoded[0];
        assert_eq!((picture.width, picture.height), (2, 1));
        // Four bytes a pixel, which is what the atlas takes.
        // Two pixels, four bytes each, which is what the atlas takes.
        assert_eq!(picture.rgba.len(), 8);
    }

    /// A two by one PNG: one red pixel and one blue one.
    static PNG_2X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x7B,
        0x40, 0xE8, 0xDD, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0x00, 0x04, 0xFF, 0x01, 0x07, 0x00, 0x01, 0xFF, 0xE2, 0x23, 0x9E, 0x59, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
}
