//! The parts that only one platform has.
//!
//! Everything here is behaviour a window cannot get through winit and cannot
//! draw for itself, so it has to talk to the platform directly. Each module is
//! compiled only where it applies, and every caller goes through the shims at
//! the bottom so the runtime reads the same on every system.

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::{install as install_caption, Caption};

/// Elsewhere there is nothing to install and nothing to answer.
///
/// A stand in rather than a pile of `cfg` at the call site: the runtime asks
/// for this on every platform and simply gets a handle that does nothing.
#[cfg(not(target_os = "windows"))]
pub struct Caption;

#[cfg(not(target_os = "windows"))]
impl Caption {
    pub fn state(&self) -> &() {
        &()
    }
}

#[cfg(not(target_os = "windows"))]
pub fn install_caption(_handle: &dyn raw_window_handle::HasWindowHandle) -> Option<Caption> {
    None
}
