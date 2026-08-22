//! Native file and message dialogs.
//!
//! These are the platform's own dialogs rather than something drawn here, and
//! deliberately so. A file picker is where a person expects their sidebar, their
//! recent places, their network drives and their search to be, and no framework
//! reproduces those convincingly. What guirs draws is the application; this is
//! the part that belongs to the desktop.
//!
//! Every call blocks until the person answers, which is what modal means. The
//! window stops drawing while a dialog is open, exactly as a native application
//! does:
//!
//! ```no_run
//! # use guirs_ui::dialog::FileDialog;
//! # fn load(_: &std::path::Path) {}
//! if let Some(path) = FileDialog::new()
//!     .title("Open a document")
//!     .filter("Text", &["txt", "md"])
//!     .open_file()
//! {
//!     load(&path);
//! }
//! ```

use std::path::{Path, PathBuf};

/// A native file picker.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    title: Option<String>,
    directory: Option<PathBuf>,
    file_name: Option<String>,
    filters: Vec<(String, Vec<String>)>,
}

impl FileDialog {
    pub fn new() -> Self {
        FileDialog::default()
    }

    /// The dialog's own title. The platform supplies a sensible one otherwise.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Where to start. Ignored if the path does not exist.
    pub fn directory(mut self, path: impl AsRef<Path>) -> Self {
        self.directory = Some(path.as_ref().to_path_buf());
        self
    }

    /// The name to offer when saving.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// Add a named group of extensions, written without the dot.
    ///
    /// ```
    /// # use guirs_ui::dialog::FileDialog;
    /// FileDialog::new().filter("Images", &["png", "jpg", "webp"]);
    /// ```
    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(|ext| ext.to_string()).collect(),
        ));
        self
    }

    fn build(&self) -> rfd::FileDialog {
        let mut dialog = rfd::FileDialog::new();
        if let Some(title) = &self.title {
            dialog = dialog.set_title(title);
        }
        // A directory that has been deleted since it was remembered would send
        // some platforms somewhere surprising, so it is checked first.
        if let Some(directory) = &self.directory {
            if directory.is_dir() {
                dialog = dialog.set_directory(directory);
            }
        }
        if let Some(name) = &self.file_name {
            dialog = dialog.set_file_name(name);
        }
        for (name, extensions) in &self.filters {
            let refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(name, &refs);
        }
        dialog
    }

    /// Pick one existing file. `None` if the dialog was cancelled.
    pub fn open_file(self) -> Option<PathBuf> {
        self.build().pick_file()
    }

    /// Pick any number of existing files.
    pub fn open_files(self) -> Option<Vec<PathBuf>> {
        self.build().pick_files()
    }

    /// Pick a folder.
    pub fn open_directory(self) -> Option<PathBuf> {
        self.build().pick_folder()
    }

    /// Choose where to write. The platform asks about overwriting.
    pub fn save_file(self) -> Option<PathBuf> {
        self.build().save_file()
    }
}

/// What a message dialog looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageLevel {
    #[default]
    Info,
    Warning,
    Error,
}

impl MessageLevel {
    fn to_rfd(self) -> rfd::MessageLevel {
        match self {
            MessageLevel::Info => rfd::MessageLevel::Info,
            MessageLevel::Warning => rfd::MessageLevel::Warning,
            MessageLevel::Error => rfd::MessageLevel::Error,
        }
    }
}

/// A native message box.
#[derive(Clone, Debug, Default)]
pub struct MessageDialog {
    title: String,
    text: String,
    level: MessageLevel,
}

impl MessageDialog {
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        MessageDialog {
            title: title.into(),
            text: text.into(),
            level: MessageLevel::Info,
        }
    }

    pub fn level(mut self, level: MessageLevel) -> Self {
        self.level = level;
        self
    }

    pub fn warning(self) -> Self {
        self.level(MessageLevel::Warning)
    }

    pub fn error(self) -> Self {
        self.level(MessageLevel::Error)
    }

    fn build(&self) -> rfd::MessageDialog {
        rfd::MessageDialog::new()
            .set_title(&self.title)
            .set_description(&self.text)
            .set_level(self.level.to_rfd())
    }

    /// Show it with a single acknowledging button.
    pub fn show(self) {
        let _ = self.build().set_buttons(rfd::MessageButtons::Ok).show();
    }

    /// Ask a yes or no question. `true` means yes.
    ///
    /// Phrase the question so that yes is the action being asked about, because
    /// which button a platform makes the default is not this code's decision.
    pub fn confirm(self) -> bool {
        matches!(
            self.build()
                .set_buttons(rfd::MessageButtons::YesNo)
                .show(),
            rfd::MessageDialogResult::Yes
        )
    }
}

/// Show a message and wait for it to be dismissed.
pub fn message(title: impl Into<String>, text: impl Into<String>) {
    MessageDialog::new(title, text).show();
}

/// Ask a yes or no question and wait for the answer.
pub fn confirm(title: impl Into<String>, text: impl Into<String>) -> bool {
    MessageDialog::new(title, text).confirm()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A dialog cannot be opened in a test: it would block waiting for someone
    // to answer it. What is testable is that the description a caller builds is
    // the description that reaches the platform.

    #[test]
    fn a_filter_keeps_its_name_and_its_extensions() {
        let dialog = FileDialog::new()
            .filter("Images", &["png", "jpg"])
            .filter("All", &["*"]);
        assert_eq!(dialog.filters.len(), 2);
        assert_eq!(dialog.filters[0].0, "Images");
        assert_eq!(dialog.filters[0].1, vec!["png", "jpg"]);
        assert_eq!(dialog.filters[1].1, vec!["*"]);
    }

    #[test]
    fn the_builder_records_what_it_was_told() {
        let dialog = FileDialog::new()
            .title("Open a document")
            .file_name("untitled.md")
            .directory("/definitely/not/here");
        assert_eq!(dialog.title.as_deref(), Some("Open a document"));
        assert_eq!(dialog.file_name.as_deref(), Some("untitled.md"));
        assert!(dialog.directory.is_some());

        // The starting directory is only handed over if it exists, which is
        // checked at the point of showing rather than here.
        let _ = dialog.build();
    }

    #[test]
    fn a_message_defaults_to_information() {
        let dialog = MessageDialog::new("Saved", "The file was written.");
        assert_eq!(dialog.level, MessageLevel::Info);
        assert_eq!(dialog.warning().level, MessageLevel::Warning);
        assert_eq!(
            MessageDialog::new("t", "b").error().level,
            MessageLevel::Error
        );
    }

    #[test]
    fn building_a_dialog_touches_no_platform_state() {
        // Constructing is separate from showing, so a dialog can be described
        // anywhere and only opened where blocking is acceptable.
        let _ = MessageDialog::new("Title", "Body").build();
        let _ = FileDialog::new().title("Pick").build();
    }
}
