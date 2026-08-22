//! The application runtime.
//!
//! Wraps winit, translates platform events into guirs events, and drives the
//! window. The event loop waits rather than spinning: a frame is drawn in
//! response to input, to an animation in progress, or to a stylesheet changing
//! on disk. An idle interface costs nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use guirs_core::{Bounds, Point, Px, ScaleFactor};
use guirs_render::{GraphicsBackend, Renderer};
use guirs_style::{CursorStyle, StyleEngine, Stylesheet};
use guirs_text::TextSystem;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window as PlatformWindow, WindowId};

use crate::context::Cx;
use crate::element::AnyElement;
use crate::event::{
    FileDropEvent, ImeEvent, InputEvent, Key, Modifiers, MouseButton, MouseEvent, NewWindow,
    ResizeEdge, ScrollEvent, TextInputEvent, WindowAction,
};
use crate::window::{RootFn, Window};

/// How far one wheel notch scrolls, in logical pixels.
const LINE_SCROLL: f32 = 48.0;

/// How the platform window is created.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub resizable: bool,
    pub decorations: bool,
    pub transparent: bool,
    /// Pace the loop to the display's refresh rate. Turn it off to measure what
    /// a frame really costs, or for a benchmark.
    pub vsync: bool,
    /// Prefer the discrete GPU. Interfaces are fill rate bound rather than
    /// compute bound, so on a machine with both this is usually the difference
    /// between a comfortable frame budget and a tight one.
    pub high_performance: bool,
    /// Which graphics API to draw through.
    ///
    /// The default loads one vendor's driver rather than every vendor's, which
    /// on a machine with more than one graphics card is the difference between
    /// tens of megabytes and hundreds.
    pub backend: GraphicsBackend,
    /// Ask the compositor to round the window's corners.
    ///
    /// The compositor is asked rather than the corners being drawn, so the
    /// window keeps its drop shadow, its corners are clipped rather than
    /// painted over, and the result matches every other window on the desktop.
    pub rounded_corners: bool,
    /// The picture the desktop shows for this window: in its title bar, when
    /// switching windows, and on its button in the taskbar.
    ///
    /// This is the running window's icon, which is a different thing from the
    /// icon carved into the executable. See [`App::icon`].
    pub icon: Option<crate::image::ImageSource>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            title: "guirs".into(),
            width: 1100.0,
            height: 720.0,
            min_width: Some(400.0),
            min_height: Some(300.0),
            resizable: true,
            decorations: true,
            transparent: false,
            vsync: true,
            high_performance: true,
            backend: GraphicsBackend::Native,
            rounded_corners: true,
            icon: None,
        }
    }
}

impl WindowOptions {
    /// A window with a title and otherwise ordinary settings.
    pub fn new(title: impl Into<String>) -> Self {
        WindowOptions {
            title: title.into(),
            ..WindowOptions::default()
        }
    }

    pub fn sized(mut self, width: f64, height: f64) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn min_size(mut self, width: f64, height: f64) -> Self {
        self.min_width = Some(width);
        self.min_height = Some(height);
        self
    }

    /// Draw the window's own chrome instead of the platform's.
    pub fn undecorated(mut self) -> Self {
        self.decorations = false;
        self
    }

    pub fn fixed_size(mut self) -> Self {
        self.resizable = false;
        self
    }

    pub fn transparent(mut self) -> Self {
        self.transparent = true;
        self
    }

    /// The picture the desktop shows for this window.
    ///
    /// See [`App::icon`] for what this covers and what it does not.
    pub fn icon(mut self, source: impl Into<crate::image::ImageSource>) -> Self {
        self.icon = Some(source.into());
        self
    }

    pub fn square_corners(mut self) -> Self {
        self.rounded_corners = false;
        self
    }
}

/// What can go wrong starting an application.
#[derive(Debug)]
pub enum AppError {
    EventLoop(winit::error::EventLoopError),
    Window(winit::error::OsError),
    Renderer(guirs_render::RendererError),
    NoRoot,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::EventLoop(e) => write!(f, "event loop: {e}"),
            AppError::Window(e) => write!(f, "window: {e}"),
            AppError::Renderer(e) => write!(f, "renderer: {e}"),
            AppError::NoRoot => f.write_str("no root element was provided"),
        }
    }
}

impl std::error::Error for AppError {}

/// Builds and runs an application.
///
/// # A window rather than a command
///
/// On Windows a program is linked as either a console application or a windowed
/// one, and the default is a console application. A windowed program built that
/// way opens a console behind itself, which is the black rectangle that appears
/// when one is started from the desktop. There is no way for a library to
/// settle this on an application's behalf, because it is decided by an
/// attribute the compiler reads at the top of the crate root. One line, above
/// the first item in `main.rs`:
///
/// ```
/// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
/// ```
///
/// Written that way, a released build is a window and a debug build keeps its
/// console, which is somewhere for a `println!` to land while developing. It
/// does nothing anywhere else, so it is safe to leave in.
pub struct App {
    options: WindowOptions,
    stylesheet_source: Option<String>,
    stylesheet_path: Option<PathBuf>,
    hot_reload: bool,
    font_paths: Vec<PathBuf>,
    font_dirs: Vec<PathBuf>,
    font_bytes: Vec<Vec<u8>>,
    system_fonts: bool,
    default_family: Option<String>,
    root_font_size: Px,
}

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            options: WindowOptions::default(),
            stylesheet_source: None,
            stylesheet_path: None,
            hot_reload: true,
            font_paths: Vec::new(),
            font_dirs: Vec::new(),
            font_bytes: Vec::new(),
            system_fonts: true,
            default_family: None,
            root_font_size: Px(16.0),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.options.title = title.into();
        self
    }

    /// The picture the desktop shows for this window.
    ///
    /// Shown in the title bar, when switching between windows, and on the
    /// window's taskbar button while it is running. A square PNG is the usual
    /// thing to hand it, either a path or bytes from `include_bytes!`:
    ///
    /// ```no_run
    /// # use guirs_ui::App;
    /// App::new().icon(include_bytes!("../../../examples/kitchen-sink/assets/sample.png"));
    /// ```
    ///
    /// This is the *window's* icon. The one the desktop shows for the program
    /// itself, in Explorer, on a pinned shortcut and in Task Manager, is a
    /// resource carved into the executable at link time, which no library can
    /// do from inside a running process. The README explains how to set that
    /// one.
    pub fn icon(mut self, source: impl Into<crate::image::ImageSource>) -> Self {
        self.options.icon = Some(source.into());
        self
    }

    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.options.width = width;
        self.options.height = height;
        self
    }

    pub fn min_size(mut self, width: f64, height: f64) -> Self {
        self.options.min_width = Some(width);
        self.options.min_height = Some(height);
        self
    }

    /// Apply a transformation only when `condition` holds.
    pub fn when(self, condition: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            f(self)
        } else {
            self
        }
    }

    /// Draw through a specific graphics API.
    pub fn backend(mut self, backend: GraphicsBackend) -> Self {
        self.options.backend = backend;
        self
    }

    /// Leave the window's corners square.
    pub fn square_corners(mut self) -> Self {
        self.options.rounded_corners = false;
        self
    }

    /// Prefer the integrated GPU, for battery life.
    pub fn low_power(mut self) -> Self {
        self.options.high_performance = false;
        self
    }

    /// Stop pacing the loop to the display's refresh rate.
    pub fn without_vsync(mut self) -> Self {
        self.options.vsync = false;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.options.resizable = resizable;
        self
    }

    /// Draw without the platform's title bar, for a custom window chrome.
    pub fn undecorated(mut self) -> Self {
        self.options.decorations = false;
        self
    }

    pub fn window_options(mut self, options: WindowOptions) -> Self {
        self.options = options;
        self
    }

    /// Use a stylesheet compiled into the binary.
    pub fn stylesheet(mut self, source: impl Into<String>) -> Self {
        self.stylesheet_source = Some(source.into());
        self
    }

    /// Load a stylesheet from disk and watch it for changes.
    pub fn stylesheet_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stylesheet_path = Some(path.into());
        self
    }

    /// Turn off reloading the stylesheet when the file changes.
    pub fn no_hot_reload(mut self) -> Self {
        self.hot_reload = false;
        self
    }

    /// Register a font file.
    pub fn font_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.font_paths.push(path.into());
        self
    }

    /// Register every font in a directory.
    pub fn font_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.font_dirs.push(path.into());
        self
    }

    /// Register a font embedded in the binary.
    pub fn font_bytes(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.font_bytes.push(bytes.into());
        self
    }

    /// Skip loading the machine's installed fonts. Startup gets faster, and
    /// only the fonts registered here are available.
    pub fn without_system_fonts(mut self) -> Self {
        self.system_fonts = false;
        self
    }

    /// The family used when a style names none.
    pub fn default_font_family(mut self, family: impl Into<String>) -> Self {
        self.default_family = Some(family.into());
        self
    }

    /// The size one `rem` refers to.
    pub fn root_font_size(mut self, size: Px) -> Self {
        self.root_font_size = size;
        self
    }

    /// Run until the window closes.
    pub fn run(
        self,
        root: impl FnMut(&mut Cx) -> AnyElement + 'static,
    ) -> Result<(), AppError> {
        // A user event type, so background work can wake the loop. Without one
        // the loop sleeps until the platform has something to say, and a task
        // finishing is not something the platform knows about.
        let event_loop = EventLoop::<Wake>::with_user_event()
            .build()
            .map_err(AppError::EventLoop)?;
        event_loop.set_control_flow(ControlFlow::Wait);

        // Sending can fail once the loop has gone, which is what happens while
        // the application is shutting down and a task finishes on the way out.
        // There is nothing left to draw at that point, so it is not an error.
        let proxy = event_loop.create_proxy();
        crate::task::set_waker(move || {
            let _ = proxy.send_event(Wake::Task);
        });

        let mut runner = Runner::new(self, Box::new(root), event_loop.create_proxy())?;
        event_loop.run_app(&mut runner).map_err(AppError::EventLoop)
    }
}

/// Something that happened away from the platform's own event stream.
#[derive(Debug)]
enum Wake {
    /// Work finished off the interface's thread. What finished is the task's
    /// business; the interface finds out by asking the tasks it holds.
    Task,
    /// A screen reader attached, detached, or asked for something.
    Access(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for Wake {
    fn from(event: accesskit_winit::Event) -> Self {
        Wake::Access(event)
    }
}

/// One open window: the platform's half and ours.
struct WindowHost {
    platform: Arc<PlatformWindow>,
    window: Window,
    /// Per window, because two windows have the pointer in different places
    /// and want different cursors over what is under it.
    cursor_position: Point<Px>,
    applied_cursor: CursorStyle,
    /// Files collected from the desktop, flushed as one event once the burst
    /// of per file messages the platform sends has finished arriving.
    pending_drop: Vec<PathBuf>,
    pending_hover: Vec<PathBuf>,
    hover_cancelled: bool,
    /// Whether the input method has been turned on, and where it was last told
    /// the caret is. Tracked so the platform is only told when it changes,
    /// which for the caret area is otherwise once a frame forever.
    ime_allowed: bool,
    ime_area: Option<Bounds<Px>>,
    /// Answers the platform's questions about this window's own caption
    /// controls, which is what keeps Windows layout picker working over a
    /// maximize button the application drew itself.
    ///
    /// Held everywhere so the runtime reads the same on every system, but only
    /// ever consulted on Windows, which is the only platform that asks.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    caption: Option<crate::platform::Caption>,
    /// Bridges this window to whatever is reading it aloud. Present always;
    /// it does nothing until a screen reader actually attaches.
    access: accesskit_winit::Adapter,
    /// Whether one is attached. Building the tree costs something, so it is
    /// only built while somebody is listening.
    access_active: bool,
    closing: bool,
}

struct Runner {
    config: App,
    /// Kept so a window opened later can be given its own adapter.
    proxy: winit::event_loop::EventLoopProxy<Wake>,
    /// Windows asked for but not created yet. The first one is the root; the
    /// rest arrive while running.
    pending: Vec<NewWindow>,
    windows: HashMap<WindowId, WindowHost>,
    /// Modifier state belongs to the keyboard rather than to a window.
    modifiers: Modifiers,
    reload: Option<Receiver<()>>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Runner {
    fn new(
        config: App,
        root: RootFn,
        proxy: winit::event_loop::EventLoopProxy<Wake>,
    ) -> Result<Self, AppError> {
        let first = NewWindow {
            options: config.options.clone(),
            root,
        };
        Ok(Runner {
            config,
            proxy,
            pending: vec![first],
            windows: HashMap::new(),
            modifiers: Modifiers::default(),
            reload: None,
            _watcher: None,
        })
    }

    /// Bring up every window that has been asked for and not created yet.
    fn open_pending(&mut self, event_loop: &ActiveEventLoop) {
        for spec in std::mem::take(&mut self.pending) {
            match self.open_window(event_loop, spec) {
                Ok(id) => {
                    log::debug!("opened window {id:?}");
                }
                Err(error) => log::error!("could not open a window: {error}"),
            }
        }
    }

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        spec: NewWindow,
    ) -> Result<WindowId, String> {
        let options = spec.options;
        let mut attributes = PlatformWindow::default_attributes()
            .with_title(options.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(options.width, options.height))
            .with_resizable(options.resizable)
            .with_decorations(options.decorations)
            .with_transparent(options.transparent)
            // The accessibility adapter has to be attached before anything is
            // on screen, so the window is made hidden and shown below.
            .with_visible(false);

        // Decoded here rather than on the task pool: it is one small picture,
        // it is wanted before the window is on screen, and a window that
        // appears without its icon and gains one a frame later flickers in the
        // taskbar.
        if let Some(source) = &options.icon {
            match crate::image::load_icon(source) {
                Ok(icon) => {
                    // Windows keeps two icons per window and uses them in
                    // different places: the small one in the title bar, the
                    // large one on the taskbar button and in the window
                    // switcher. Setting only the first is why an application
                    // can look right in its title bar and still show a
                    // stranger's icon in the taskbar, so both are set here.
                    #[cfg(target_os = "windows")]
                    {
                        use winit::platform::windows::WindowAttributesExtWindows;
                        attributes = attributes.with_taskbar_icon(Some(icon.clone()));
                    }
                    attributes = attributes.with_window_icon(Some(icon));
                }
                Err(error) => log::warn!("window icon could not be used: {error}"),
            }
        }

        if let (Some(width), Some(height)) = (options.min_width, options.min_height) {
            attributes =
                attributes.with_min_inner_size(winit::dpi::LogicalSize::new(width, height));
        }

        let platform = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|error| error.to_string())?,
        );
        apply_corner_preference(&platform, options.rounded_corners);

        let physical = platform.inner_size();
        let scale = ScaleFactor(platform.scale_factor() as f32);
        let renderer = Renderer::new(
            platform.clone(),
            physical.width.max(1),
            physical.height.max(1),
            scale,
            options.vsync,
            options.high_performance,
            options.backend,
        )
        .map_err(|error| error.to_string())?;

        let mut cx = Cx::new(self.build_text_system(), self.build_style_engine());
        cx.root_font_size = self.config.root_font_size;

        let id = platform.id();
        let access = accesskit_winit::Adapter::with_event_loop_proxy(
            event_loop,
            &platform,
            self.proxy.clone(),
        );
        platform.set_visible(true);

        let caption = crate::platform::install_caption(platform.as_ref());
        self.windows.insert(
            id,
            WindowHost {
                platform,
                window: Window::new(renderer, cx, spec.root),
                cursor_position: Point::zero(),
                applied_cursor: CursorStyle::Default,
                pending_drop: Vec::new(),
                pending_hover: Vec::new(),
                hover_cancelled: false,
                ime_allowed: false,
                ime_area: None,
                caption,
                access,
                access_active: false,
                closing: false,
            },
        );
        Ok(id)
    }

    /// Drop every window that asked to go, and stop when none are left.
    fn reap_closed(&mut self, event_loop: &ActiveEventLoop) {
        self.windows
            .retain(|_, host| !host.closing && !host.window.should_close());
        if self.windows.is_empty() && self.pending.is_empty() {
            event_loop.exit();
        }
    }

    fn build_style_engine(&self) -> StyleEngine {
        let source = match (&self.config.stylesheet_path, &self.config.stylesheet_source) {
            (Some(path), _) => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    log::error!("could not read {}: {error}", path.display());
                    self.config.stylesheet_source.clone().unwrap_or_default()
                }
            },
            (None, Some(inline)) => inline.clone(),
            (None, None) => String::new(),
        };

        let sheet = Stylesheet::parse(&source);
        for error in &sheet.errors {
            log::warn!("stylesheet: {error}");
        }
        StyleEngine::new(sheet)
    }

    fn build_text_system(&self) -> TextSystem {
        let mut text = if self.config.system_fonts {
            TextSystem::with_system_fonts()
        } else {
            TextSystem::new()
        };
        for path in &self.config.font_paths {
            if let Err(error) = text.fonts.load_file(path) {
                log::warn!("could not load font {}: {error}", path.display());
            }
        }
        for dir in &self.config.font_dirs {
            text.fonts.load_dir(dir);
        }
        for bytes in &self.config.font_bytes {
            text.fonts.load_bytes(bytes.clone());
        }
        if let Some(family) = &self.config.default_family {
            text.fonts.set_default_family(family.as_str());
        }
        text
    }

    fn start_watching(&mut self) {
        let (Some(path), true) = (self.config.stylesheet_path.clone(), self.config.hot_reload)
        else {
            return;
        };
        let (sender, receiver) = channel();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                if event.kind.is_modify() || event.kind.is_create() {
                    let _ = sender.send(());
                }
            }
        });

        match watcher {
            Ok(mut watcher) => {
                use notify::Watcher;
                // Watch the containing directory rather than the file itself.
                // Editors routinely save by writing a temporary file and
                // renaming it over the original, which detaches a watch bound
                // to the inode.
                let target = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                if let Err(error) = watcher.watch(&target, notify::RecursiveMode::NonRecursive) {
                    log::warn!("could not watch {}: {error}", target.display());
                    return;
                }
                self._watcher = Some(watcher);
                self.reload = Some(receiver);
                log::info!("watching {} for changes", path.display());
            }
            Err(error) => log::warn!("could not start a file watcher: {error}"),
        }
    }

    /// Carry out whatever a window's tree asked the platform to do.
    fn apply_window_actions(&mut self, id: WindowId) {
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        let actions = host.window.take_window_actions();
        if actions.is_empty() {
            return;
        }
        let platform = host.platform.clone();
        let mut opening = Vec::new();

        for action in actions {
            match action {
                WindowAction::Minimize => platform.set_minimized(true),
                WindowAction::ToggleMaximize => {
                    platform.set_maximized(!platform.is_maximized())
                }
                WindowAction::Close => host.closing = true,
                WindowAction::SetTitle(title) => platform.set_title(&title),
                // Deferred: creating a window needs the event loop, which is
                // not in hand here, and the borrow of this one is.
                WindowAction::Open(spec) => opening.push(*spec),
                // Both hand the press in progress to the platform, which then
                // owns the gesture until the button comes back up.
                WindowAction::StartDrag => {
                    if let Err(error) = platform.drag_window() {
                        log::debug!("window drag refused: {error}");
                    }
                }
                WindowAction::StartResize(edge) => {
                    if let Err(error) = platform.drag_resize_window(to_platform_edge(edge)) {
                        log::debug!("window resize refused: {error}");
                    }
                }
            }
        }

        self.pending.extend(opening);
    }

    fn apply_cursor(&mut self, id: WindowId) {
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        let cursor = host.window.cursor();
        if cursor == host.applied_cursor {
            return;
        }
        host.applied_cursor = cursor;
        host.platform.set_cursor(to_platform_cursor(cursor));
    }

    /// Turn the input method on where a field has focus, and tell it where.
    ///
    /// Composition is off until something asks for it, because a window with
    /// no text field should not be opening candidate windows. The caret area
    /// is what stops the candidate list appearing in a corner of the screen.
    fn apply_ime(&mut self, id: WindowId) {
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        let area = host.window.ime_area();
        let allowed = area.is_some();

        if allowed != host.ime_allowed {
            host.ime_allowed = allowed;
            host.platform.set_ime_allowed(allowed);
            // A field that just lost focus should not leave a candidate window
            // hanging over the place it used to be.
            if !allowed {
                host.ime_area = None;
            }
        }

        if let Some(area) = area {
            if host.ime_area != Some(area) {
                host.ime_area = Some(area);
                host.platform.set_ime_cursor_area(
                    winit::dpi::LogicalPosition::new(area.origin.x.0, area.origin.y.0),
                    winit::dpi::LogicalSize::new(area.size.width.0, area.size.height.0),
                );
            }
        }
    }

    /// Keep the platform told where the maximize control is, and act on what
    /// it saw there.
    ///
    /// Claiming that rectangle means the ordinary mouse messages stop arriving
    /// over it, so the hover and the click are fed back in here as though they
    /// had come the usual way. Without that the button would go dead and stop
    /// lighting up the moment the layout picker started working.
    #[cfg(target_os = "windows")]
    fn apply_caption(&mut self, id: WindowId) {
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        let Some(caption) = &host.caption else {
            return;
        };

        let target = host.window.snap_target();
        let scale = ScaleFactor(host.platform.scale_factor() as f32);

        let (hover, left, clicked) = {
            let mut state = caption.state().borrow_mut();
            state.set_target(target, scale);
            state.take()
        };

        if let Some((x, y)) = hover {
            let (x, y) = caption.state().borrow().to_logical(x, y);
            host.cursor_position = Point::new(x, y);
            host.window.handle_event(InputEvent::MouseMove(MouseEvent {
                position: Point::new(x, y),
                button: None,
                modifiers: self.modifiers,
                click_count: 0,
                ..Default::default()
            }));
        }
        if left {
            host.window.handle_event(InputEvent::MouseLeave);
        }
        if clicked {
            host.platform.set_maximized(!host.platform.is_maximized());
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn apply_caption(&mut self, _id: WindowId) {}

    /// Tell whatever is listening what the last frame looked like.
    ///
    /// Only while something is: an application with no screen reader attached
    /// builds no tree and sends no update, which is why this can run after
    /// every frame without anyone noticing.
    fn publish_access(&mut self, id: WindowId) {
        let title = self.config.options.title.clone();
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        if !host.access_active {
            return;
        }
        let scale = ScaleFactor(host.platform.scale_factor() as f32);
        let (tree, focused) = host.window.access();
        let update = crate::access_bridge::build(tree, focused, scale, &title);
        host.access.update_if_active(|| update);
    }

    /// Something attached, detached, or asked for an action.
    fn handle_access(&mut self, event: accesskit_winit::Event) {
        use accesskit_winit::WindowEvent as AccessEvent;

        let id = event.window_id;
        match event.window_event {
            AccessEvent::InitialTreeRequested => {
                if let Some(host) = self.windows.get_mut(&id) {
                    // Turning this on is what makes the next frame describe
                    // itself. The frame after that is the one with anything in
                    // it, so a redraw is asked for as well.
                    host.access_active = true;
                    host.window.set_accessible(true);
                    host.window.request_redraw();
                    host.platform.request_redraw();
                }
            }
            AccessEvent::AccessibilityDeactivated => {
                if let Some(host) = self.windows.get_mut(&id) {
                    host.access_active = false;
                    host.window.set_accessible(false);
                }
            }
            AccessEvent::ActionRequested(request) => {
                let Some(element) = crate::access_bridge::element_for(request.target_node) else {
                    return;
                };
                if let Some(host) = self.windows.get_mut(&id) {
                    if host.window.perform_access_action(request.action, element) {
                        host.platform.request_redraw();
                    }
                }
            }
        }
    }

    /// Hand over any files the desktop finished delivering.
    ///
    /// The platform reports a multiple file drop one path at a time, so they
    /// are collected and delivered together once the burst is over. Otherwise
    /// dropping four files would look like four separate drops.
    fn flush_file_events(&mut self) {
        for host in self.windows.values_mut() {
            let position = host.cursor_position;
            if !host.pending_hover.is_empty() {
                let paths = std::mem::take(&mut host.pending_hover);
                host.window.handle_event(InputEvent::FileHover(FileDropEvent {
                    paths,
                    position,
                    modifiers: self.modifiers,
                }));
            }
            if !host.pending_drop.is_empty() {
                let paths = std::mem::take(&mut host.pending_drop);
                host.window.handle_event(InputEvent::FileDrop(FileDropEvent {
                    paths,
                    position,
                    modifiers: self.modifiers,
                }));
            }
            if std::mem::take(&mut host.hover_cancelled) {
                host.window.handle_event(InputEvent::FileHoverCancelled);
            }
        }
    }
}

impl ApplicationHandler<Wake> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.open_pending(event_loop);
        if self.windows.is_empty() {
            event_loop.exit();
            return;
        }
        self.start_watching();
    }

    /// Something finished off the interface's thread.
    ///
    /// Which window wanted it is not recorded, so every window is asked to
    /// draw. A window with nothing to show for it draws one frame and settles
    /// again, which is cheaper than tracking who is waiting on what.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Task => {
                for host in self.windows.values_mut() {
                    host.window.request_redraw();
                    host.platform.request_redraw();
                }
            }
            Wake::Access(event) => self.handle_access(event),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // An event for a window that has already gone is not an error; the
        // platform can have queued it before the close took effect.
        let Some(host) = self.windows.get_mut(&id) else {
            return;
        };
        // The adapter has to see the platform's events too, or a screen
        // reader loses track of where the window is and whether it is active.
        host.access.process_event(&host.platform, &event);

        let scale = host.platform.scale_factor() as f32;
        let maximized = host.platform.is_maximized();

        if matches!(event, WindowEvent::CloseRequested) {
            host.closing = true;
            self.reap_closed(event_loop);
            return;
        }

        let modifiers = self.modifiers;
        let mut new_modifiers = None;
        let mut published = false;
        let window = &mut host.window;

        match event {
            WindowEvent::Resized(size) => {
                window.resize(size.width.max(1), size.height.max(1), ScaleFactor(scale));
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = host.platform.inner_size();
                window.resize(
                    size.width.max(1),
                    size.height.max(1),
                    ScaleFactor(scale_factor as f32),
                );
            }

            WindowEvent::RedrawRequested => {
                window.set_maximized(maximized);
                window.draw();
                published = true;
            }

            WindowEvent::Focused(focused) => {
                window.handle_event(InputEvent::WindowFocus(focused));
            }

            WindowEvent::ModifiersChanged(state) => {
                let state = state.state();
                new_modifiers = Some(Modifiers {
                    shift: state.shift_key(),
                    control: state.control_key(),
                    alt: state.alt_key(),
                    meta: state.super_key(),
                });
            }

            WindowEvent::CursorMoved { position, .. } => {
                let logical = Point::new(
                    Px(position.x as f32 / scale),
                    Px(position.y as f32 / scale),
                );
                host.cursor_position = logical;
                window.handle_event(InputEvent::MouseMove(MouseEvent {
                    position: logical,
                    button: None,
                    modifiers,
                    click_count: 0,
                    // Filled in per recipient during dispatch.
                    ..Default::default()
                }));
            }

            WindowEvent::CursorLeft { .. } => {
                window.handle_event(InputEvent::MouseLeave);
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let button = to_button(button);
                let position = host.cursor_position;
                let click_count = match state {
                    ElementState::Pressed => window.track_click(position),
                    ElementState::Released => 1,
                };
                let mouse = MouseEvent {
                    position,
                    button: Some(button),
                    modifiers,
                    click_count,
                    ..Default::default()
                };
                window.handle_event(match state {
                    ElementState::Pressed => InputEvent::MouseDown(mouse),
                    ElementState::Released => InputEvent::MouseUp(mouse),
                });
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // Positive delta moves the view toward the end of the content,
                // which is the opposite sign from what the platform reports.
                let (delta, precise) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (
                        Point::new(Px(-x * LINE_SCROLL), Px(-y * LINE_SCROLL)),
                        false,
                    ),
                    MouseScrollDelta::PixelDelta(p) => (
                        Point::new(Px(-(p.x as f32) / scale), Px(-(p.y as f32) / scale)),
                        true,
                    ),
                };
                window.handle_event(InputEvent::Scroll(ScrollEvent {
                    position: host.cursor_position,
                    delta,
                    modifiers,
                    precise,
                }));
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let key = to_key(&event.logical_key);
                let key_event = crate::event::KeyEvent {
                    key,
                    modifiers,
                    repeat: event.repeat,
                };
                match event.state {
                    ElementState::Pressed => {
                        window.handle_event(InputEvent::KeyDown(key_event));
                        // The platform decides what a keystroke produces, so
                        // text arrives separately from the key itself.
                        if let Some(text) = &event.text {
                            let printable: String =
                                text.chars().filter(|c| !c.is_control()).collect();
                            if !printable.is_empty() && !modifiers.accelerator() {
                                window.handle_event(InputEvent::TextInput(TextInputEvent {
                                    text: printable,
                                }));
                            }
                        }
                    }
                    ElementState::Released => {
                        window.handle_event(InputEvent::KeyUp(key_event));
                    }
                }
            }

            WindowEvent::Ime(ime) => {
                window.handle_event(InputEvent::Ime(match ime {
                    winit::event::Ime::Enabled => ImeEvent::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => {
                        ImeEvent::Preedit { text, cursor }
                    }
                    winit::event::Ime::Commit(text) => ImeEvent::Commit(text),
                    winit::event::Ime::Disabled => ImeEvent::Disabled,
                }));
            }

            // The platform reports a drag one path at a time. They are
            // gathered here and handed over together once the burst ends.
            WindowEvent::HoveredFile(path) => {
                host.pending_hover.push(path);
                host.hover_cancelled = false;
            }
            WindowEvent::DroppedFile(path) => {
                host.pending_drop.push(path);
                host.pending_hover.clear();
                host.hover_cancelled = false;
            }
            WindowEvent::HoveredFileCancelled => {
                host.pending_hover.clear();
                host.hover_cancelled = true;
            }

            _ => {}
        }

        if let Some(modifiers) = new_modifiers {
            self.modifiers = modifiers;
        }
        if published {
            self.publish_access(id);
        }

        self.apply_window_actions(id);
        self.apply_cursor(id);
        self.apply_ime(id);
        self.apply_caption(id);
        self.open_pending(event_loop);
        self.reap_closed(event_loop);

        if let Some(host) = self.windows.get(&id) {
            if host.window.needs_redraw() {
                host.platform.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.flush_file_events();

        // Pick up a stylesheet edit.
        let reloaded = match &self.reload {
            Some(receiver) => {
                let mut any = false;
                while receiver.try_recv().is_ok() {
                    any = true;
                }
                any
            }
            None => false,
        };

        if reloaded {
            if let Some(path) = self.config.stylesheet_path.clone() {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        // Every window shares the stylesheet, so every window
                        // has to be told it changed.
                        let sheet = Stylesheet::parse(&source);
                        for host in self.windows.values_mut() {
                            host.window.set_stylesheet(sheet.clone());
                        }
                        log::info!("reloaded {}", path.display());
                    }
                    Err(error) => log::warn!("could not reread {}: {error}", path.display()),
                }
            }
        }

        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.apply_window_actions(id);
        }
        self.open_pending(event_loop);
        self.reap_closed(event_loop);

        let now = std::time::Instant::now();
        let mut soonest: Option<std::time::Instant> = None;
        for host in self.windows.values() {
            let deadline = host.window.redraw_at();
            // The moment something asked for has arrived, so give it its frame.
            let due = deadline.is_some_and(|at| at <= now);

            if host.window.needs_redraw() || due {
                host.platform.request_redraw();
            }
            if let Some(at) = deadline.filter(|at| *at > now) {
                soonest = Some(match soonest {
                    Some(existing) => existing.min(at),
                    None => at,
                });
            }
        }

        // Sleep until whatever asked to be drawn again wants its frame, rather
        // than until the next thing the platform has to say. A blinking caret
        // needs two frames a second, not sixty, and waiting is what keeps the
        // difference off the battery.
        event_loop.set_control_flow(match soonest {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

/// Ask the desktop compositor to round or square the window's corners.
///
/// Windows 11 rounds decorated windows on its own but leaves undecorated ones
/// square, so a custom title bar has to ask. Elsewhere this is a no op: macOS
/// rounds its windows already, and on Linux it is the compositor's business.
#[cfg(target_os = "windows")]
fn apply_corner_preference(window: &PlatformWindow, rounded: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Declared here rather than pulling in a Windows binding crate for two
    // constants and one call.
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;
    const DWMWCP_DONOTROUND: u32 = 1;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };

    let preference: u32 = if rounded {
        DWMWCP_ROUND
    } else {
        DWMWCP_DONOTROUND
    };
    // Fails harmlessly on Windows 10, which has no such attribute.
    let result = unsafe {
        DwmSetWindowAttribute(
            win32.hwnd.get(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const u32 as *const core::ffi::c_void,
            core::mem::size_of::<u32>() as u32,
        )
    };
    if result != 0 {
        log::debug!("the compositor declined to set a corner preference");
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_corner_preference(_window: &PlatformWindow, _rounded: bool) {}

fn to_platform_edge(edge: ResizeEdge) -> winit::window::ResizeDirection {
    use winit::window::ResizeDirection;
    match edge {
        ResizeEdge::North => ResizeDirection::North,
        ResizeEdge::South => ResizeDirection::South,
        ResizeEdge::East => ResizeDirection::East,
        ResizeEdge::West => ResizeDirection::West,
        ResizeEdge::NorthEast => ResizeDirection::NorthEast,
        ResizeEdge::NorthWest => ResizeDirection::NorthWest,
        ResizeEdge::SouthEast => ResizeDirection::SouthEast,
        ResizeEdge::SouthWest => ResizeDirection::SouthWest,
    }
}

fn to_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Back => MouseButton::Other(3),
        winit::event::MouseButton::Forward => MouseButton::Other(4),
        winit::event::MouseButton::Other(n) => MouseButton::Other(n),
    }
}

fn to_key(key: &WinitKey) -> Key {
    match key {
        WinitKey::Named(named) => match named {
            NamedKey::Escape => Key::Escape,
            NamedKey::Enter => Key::Enter,
            NamedKey::Tab => Key::Tab,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Insert => Key::Insert,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Space => Key::Space,
            NamedKey::F1 => Key::Function(1),
            NamedKey::F2 => Key::Function(2),
            NamedKey::F3 => Key::Function(3),
            NamedKey::F4 => Key::Function(4),
            NamedKey::F5 => Key::Function(5),
            NamedKey::F6 => Key::Function(6),
            NamedKey::F7 => Key::Function(7),
            NamedKey::F8 => Key::Function(8),
            NamedKey::F9 => Key::Function(9),
            NamedKey::F10 => Key::Function(10),
            NamedKey::F11 => Key::Function(11),
            NamedKey::F12 => Key::Function(12),
            _ => Key::Unknown,
        },
        WinitKey::Character(text) => match text.chars().next() {
            Some(c) => Key::Character(c.to_ascii_lowercase()),
            None => Key::Unknown,
        },
        _ => Key::Unknown,
    }
}

fn to_platform_cursor(cursor: CursorStyle) -> winit::window::CursorIcon {
    use winit::window::CursorIcon;
    match cursor {
        CursorStyle::Default => CursorIcon::Default,
        CursorStyle::Pointer => CursorIcon::Pointer,
        CursorStyle::Text => CursorIcon::Text,
        CursorStyle::Crosshair => CursorIcon::Crosshair,
        CursorStyle::Move => CursorIcon::Move,
        CursorStyle::Grab => CursorIcon::Grab,
        CursorStyle::Grabbing => CursorIcon::Grabbing,
        CursorStyle::NotAllowed => CursorIcon::NotAllowed,
        CursorStyle::Wait => CursorIcon::Wait,
        CursorStyle::ResizeHorizontal => CursorIcon::EwResize,
        CursorStyle::ResizeVertical => CursorIcon::NsResize,
        CursorStyle::ResizeNwSe => CursorIcon::NwseResize,
        CursorStyle::ResizeNeSw => CursorIcon::NeswResize,
        CursorStyle::ColResize => CursorIcon::ColResize,
        CursorStyle::RowResize => CursorIcon::RowResize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_options_have_sensible_defaults() {
        let options = WindowOptions::default();
        assert!(options.width > 0.0 && options.height > 0.0);
        assert!(options.resizable);
        assert!(options.decorations);
    }

    #[test]
    fn the_builder_records_what_it_is_given() {
        let app = App::new()
            .title("Editor")
            .size(640.0, 480.0)
            .min_size(320.0, 240.0)
            .without_system_fonts()
            .no_hot_reload()
            .default_font_family("Fira Code")
            .root_font_size(Px(18.0));

        assert_eq!(app.options.title, "Editor");
        assert_eq!(app.options.width, 640.0);
        assert_eq!(app.options.min_height, Some(240.0));
        assert!(!app.system_fonts);
        assert!(!app.hot_reload);
        assert_eq!(app.default_family.as_deref(), Some("Fira Code"));
        assert_eq!(app.root_font_size, Px(18.0));
    }

    #[test]
    fn named_keys_translate() {
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Enter)), Key::Enter);
        assert_eq!(to_key(&WinitKey::Named(NamedKey::ArrowLeft)), Key::Left);
        assert_eq!(to_key(&WinitKey::Named(NamedKey::F5)), Key::Function(5));
    }

    #[test]
    fn character_keys_are_case_folded() {
        let upper = WinitKey::Character("A".into());
        assert_eq!(to_key(&upper), Key::Character('a'));
    }

    #[test]
    fn mouse_buttons_translate() {
        assert_eq!(
            to_button(winit::event::MouseButton::Left),
            MouseButton::Left
        );
        assert_eq!(
            to_button(winit::event::MouseButton::Other(7)),
            MouseButton::Other(7)
        );
    }

    #[test]
    fn every_cursor_has_a_platform_equivalent() {
        for cursor in [
            CursorStyle::Default,
            CursorStyle::Pointer,
            CursorStyle::Text,
            CursorStyle::Crosshair,
            CursorStyle::Move,
            CursorStyle::Grab,
            CursorStyle::Grabbing,
            CursorStyle::NotAllowed,
            CursorStyle::Wait,
            CursorStyle::ResizeHorizontal,
            CursorStyle::ResizeVertical,
            CursorStyle::ResizeNwSe,
            CursorStyle::ResizeNeSw,
            CursorStyle::ColResize,
            CursorStyle::RowResize,
        ] {
            let _ = to_platform_cursor(cursor);
        }
    }
}
