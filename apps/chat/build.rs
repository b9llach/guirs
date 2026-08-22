//! The icon the desktop shows for this program.
//!
//! Separate from the one in `main.rs`: that one is the running window's, shown
//! in its taskbar button, and this one is carved into the executable, shown in
//! Explorer, on a pinned shortcut and in the task manager. Both point at the
//! same file.

fn main() {
    if let Err(error) = guirs_build::icon("assets/icon.png") {
        // Not fatal. A build that produced a working application with the
        // wrong icon is better than no application at all.
        println!("cargo:warning=icon: {error}");
    }
}
