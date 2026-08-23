//! The window: one surface, one element tree, one input pipeline.
//!
//! Input is dispatched against the hit regions the previous paint registered.
//! That is a frame behind in principle, and never noticeable in practice
//! because a frame is drawn in response to the input that preceded it.
//!
//! The routing itself lives in `InputRouter`, which needs no GPU and is
//! therefore testable.

use std::time::Instant;

use guirs_core::{GlobalElementId, Bounds, Point, Px, Rgba, ScaleFactor, Size};
use guirs_render::{FrameOutcome, Renderer};
use guirs_style::{CursorStyle, StyleEngine, Stylesheet};

use crate::context::Cx;
use crate::element::{AnyElement, Element};
use crate::event::InputEvent;
use crate::input::InputRouter;

/// The closure that builds the element tree each frame.
pub type RootFn = Box<dyn FnMut(&mut Cx) -> AnyElement>;

/// A window and everything drawn in it.
pub struct Window {
    cx: Cx,
    renderer: Renderer,
    root: RootFn,
    input: InputRouter,
    start: Instant,

    background: Rgba,
    /// Whatever part of a sequence has been typed but not yet resolved.
    keys: crate::keymap::KeymapState,
    needs_redraw: bool,
    should_close: bool,
    last_frame_stats: guirs_render::SceneStats,
}

impl Window {
    pub fn new(renderer: Renderer, cx: Cx, root: RootFn) -> Self {
        let mut window = Window {
            background: Rgba::hex(0x11111b),
            cx,
            renderer,
            root,
            input: InputRouter::new(),
            keys: crate::keymap::KeymapState::default(),
            start: Instant::now(),
            needs_redraw: true,
            should_close: false,
            last_frame_stats: Default::default(),
        };
        window.refresh_background();
        window
    }

    pub fn cx(&self) -> &Cx {
        &self.cx
    }

    pub fn cx_mut(&mut self) -> &mut Cx {
        &mut self.cx
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        &mut self.renderer
    }

    /// Whether another frame is owed, because of input, animation or a reload.
    #[inline]
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    #[inline]
    pub fn should_close(&self) -> bool {
        self.should_close
    }

    /// The pointer shape for the element currently under the pointer.
    #[inline]
    pub fn cursor(&self) -> CursorStyle {
        self.input.cursor()
    }

    pub fn scene_stats(&self) -> guirs_render::SceneStats {
        self.last_frame_stats
    }

    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Override the color the surface is cleared to.
    pub fn set_background(&mut self, color: Rgba) {
        self.background = color;
        self.needs_redraw = true;
    }

    /// Swap in a new stylesheet, as a hot reload does.
    ///
    /// Transitions are cleared because the values they were interpolating
    /// toward may no longer exist, and animating from a stale value to a new
    /// rule produces a visible lurch.
    pub fn set_stylesheet(&mut self, sheet: Stylesheet) {
        for error in &sheet.errors {
            log::warn!("stylesheet: {error}");
        }
        self.cx.styles.set_stylesheet(sheet);
        self.cx.animator.clear();
        self.refresh_background();
        self.needs_redraw = true;
    }

    fn refresh_background(&mut self) {
        if let Some(color) = self
            .cx
            .styles
            .color_token("window-background")
            .or_else(|| self.cx.styles.color_token("background"))
        {
            self.background = color;
        }
    }

    /// Reconfigure for a new size or scale factor.
    pub fn resize(&mut self, width: u32, height: u32, scale: ScaleFactor) {
        self.renderer.resize(width, height, scale);
        self.needs_redraw = true;
    }

    /// Build, lay out, paint and present one frame.
    /// How long a picture goes undrawn before it is worth reclaiming.
    ///
    /// Around a second at sixty frames a second. Long enough that scrolling
    /// something briefly off screen does not throw it away, short enough that
    /// a session working through a lot of pictures does not run out of atlas.
    const STALE_IMAGE_FRAMES: u64 = 60;

    pub fn draw(&mut self) {
        let (width, height) = self.renderer.size();
        let size = Size::new(Px(width), Px(height));
        if size.is_empty() {
            return;
        }

        let frame_started = Instant::now();
        let now = self.start.elapsed().as_secs_f64();
        self.cx.begin_frame(size, self.renderer.scale_factor(), now);
        self.cx.scene.background = self.background;

        // Each phase is timed separately, because "the frame is slow" is not a
        // diagnosis and the four phases fail for completely different reasons.
        let build_started = Instant::now();
        let mut root = (self.root)(&mut self.cx);
        let node = root.request_layout(&mut self.cx);
        let build_ms = build_started.elapsed().as_secs_f32() * 1000.0;

        let layout_started = Instant::now();
        self.cx.layout.compute(node, size, &mut self.cx.text);
        let bounds = self.cx.layout.bounds(node);
        let layout_ms = layout_started.elapsed().as_secs_f32() * 1000.0;

        let paint_started = Instant::now();
        root.paint(bounds, &mut self.cx);
        self.cx.end_frame();
        let paint_ms = paint_started.elapsed().as_secs_f32() * 1000.0;

        // Both text caches are keyed by the string itself, so a session that
        // shows a lot of different text fills them with entries nothing will
        // ask for again. Bounded here rather than inside the text system,
        // which has no idea how long the session has run.

        self.cx.text.trim_caches();

        // Between painting and rendering, which is the one point in a frame
        // where both the pictures and the graphics device are reachable.
        self.upload_images();

        self.last_frame_stats = self.cx.scene.stats();
        let render_started = Instant::now();
        let outcome = self.renderer.render(&self.cx.scene, &mut self.cx.text);
        let render_ms = render_started.elapsed().as_secs_f32() * 1000.0;

        let blend = |old: f32, new: f32| if old == 0.0 { new } else { old * 0.9 + new * 0.1 };
        self.cx.build_ms = blend(self.cx.build_ms, build_ms);
        self.cx.layout_ms = blend(self.cx.layout_ms, layout_ms);
        self.cx.paint_ms = blend(self.cx.paint_ms, paint_ms);
        self.cx.render_ms = blend(self.cx.render_ms, render_ms);
        let split = self.renderer.timings();
        self.cx.prepare_ms = blend(self.cx.prepare_ms, split.prepare_ms);
        self.cx.acquire_ms = blend(self.cx.acquire_ms, split.acquire_ms);
        self.cx.submit_ms = blend(self.cx.submit_ms, split.submit_ms);
        self.cx.rasterized = split.rasterized;
        // Measured after the frame, so an application shows what it just used.
        self.cx.memory = self.memory();

        // Exponential smoothing, so the number is readable rather than jittery.
        let elapsed = frame_started.elapsed().as_secs_f32() * 1000.0;
        self.cx.frame_ms = if self.cx.frame_ms == 0.0 {
            elapsed
        } else {
            self.cx.frame_ms * 0.9 + elapsed * 0.1
        };
        self.cx.fps = if self.cx.frame_ms > 0.0 {
            1000.0 / self.cx.frame_ms
        } else {
            0.0
        };

        // Another frame is owed while anything is still transitioning, or if
        // the surface refused this one.
        self.needs_redraw = self.cx.animating || outcome == FrameOutcome::Skipped;

        // The tree may have moved under a stationary pointer, so hover has to
        // be re-evaluated against the regions this frame just registered.
        if self.input.refresh_hover(&mut self.cx) {
            self.needs_redraw = true;
        }
    }

    /// Feed one input event through the tree.
    /// Install the keymap this window resolves presses against.
    pub fn set_keymap(&mut self, keymap: crate::keymap::Keymap) {
        self.cx.keymap = keymap;
        self.keys.clear();
    }

    /// Whether a sequence is part way through, for showing in a status bar.
    pub fn pending_keys(&self) -> &[crate::keymap::Keystroke] {
        self.keys.pending()
    }

    /// Send a command to whatever answers it, as a menu would.
    pub fn dispatch_command(&mut self, command: &str) -> bool {
        let outcome = self.cx.dispatch_command(command);
        self.needs_redraw |= outcome.redraw || outcome.handled;
        self.should_close |= outcome.quit;
        outcome.handled
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        if let InputEvent::KeyDown(key) = &event {
            if self.handle_binding(key) {
                return;
            }
        }
        let outcome = self.input.handle(&mut self.cx, &event);
        if outcome.redraw {
            self.needs_redraw = true;
        }
        if outcome.quit {
            self.should_close = true;
        }
        // Anything a handler asked for while it was running, now that the
        // handler table is no longer borrowed.
        let queued = self.cx.drain_commands();
        self.needs_redraw |= queued.redraw || queued.handled;
        self.should_close |= queued.quit;
    }

    /// Give the keymap first refusal on a press.
    ///
    /// Returns whether the press was taken. A binding that resolves to a
    /// command nothing answers is *not* taken: a keymap is allowed to name
    /// commands a particular window does not do, and swallowing the key would
    /// stop somebody typing a letter that happens to be bound elsewhere.
    fn handle_binding(&mut self, key: &crate::event::KeyEvent) -> bool {
        use crate::keymap::Match;

        // Both live on the context, so they are taken out for the duration of
        // the call and put straight back.
        let keymap = std::mem::take(&mut self.cx.keymap);
        let contexts = std::mem::take(&mut self.cx.focused_contexts);
        let outcome = self.keys.press(&keymap, &contexts, key);
        self.cx.focused_contexts = contexts;
        self.cx.keymap = keymap;

        match outcome {
            Match::None => false,
            // Half of a sequence. Taken, so the key does not also type a
            // letter, and the next press decides what it meant.
            Match::Pending => {
                self.needs_redraw = true;
                true
            }
            Match::Command(command) => self.dispatch_command(&command),
        }
    }

    /// What this window is currently holding.
    pub fn memory(&self) -> MemoryReport {
        let render = self.renderer.memory();
        MemoryReport {
            fonts: self.cx.text.fonts.memory_bytes(),
            text_caches: self.cx.text.shaper.memory_bytes()
                + self.cx.text.layout_cache_bytes(),
            textures: render.textures,
            buffers: render.buffers,
            element_state: self.cx.state.len() * 64
                + self.cx.animator.tracked_elements() * 1536
                + self.cx.images.pending_bytes(),
            atlas_entries: render.atlas_entries,
            cached_layouts: self.cx.text.cached_layouts(),
            cached_runs: self.cx.text.shaper.cached_runs(),
            faces: self.cx.text.fonts.face_count(),
        }
    }

    /// When something asked to be drawn again, if anything did.
    ///
    /// In the same clock the runtime uses to sleep, so it can wait exactly
    /// that long rather than spinning or sleeping through it.
    pub fn redraw_at(&self) -> Option<Instant> {
        let seconds = self.cx.redraw_at?;
        Some(self.start + std::time::Duration::from_secs_f64(seconds.max(0.0)))
    }

    /// Hand anything that finished decoding to the graphics device.
    ///
    /// A picture that arrives has to be drawn again to be seen, because the
    /// frame that learned about it had already been painted without it.
    fn upload_images(&mut self) {
        let decoded = self.cx.images.take_decoded();
        if decoded.is_empty() {
            return;
        }
        for (key, picture) in decoded {
            let (width, height) = picture.size();
            let mut id = self.renderer.add_image(width, height, picture.pixels());

            if id.is_none() && self.renderer.images_are_full() {
                // Out of room. Anything nothing has drawn for a while is
                // worth giving up to make space; anything still on screen is
                // read and decoded again over the next frame or two.
                let stale_before = self.renderer.frame_index().saturating_sub(Self::STALE_IMAGE_FRAMES);
                let flushed = self.renderer.flush_images(stale_before);
                if !flushed.is_empty() {
                    self.cx.images.forget(&flushed);
                    id = self.renderer.add_image(width, height, picture.pixels());
                }
            }

            match id {
                Some(id) => self.cx.images.uploaded(key, id, (width, height)),
                None => {
                    // Either larger than a page, or the atlas is full of
                    // pictures all still being looked at. Recorded as a
                    // failure rather than retried every frame forever.
                    log::warn!("image of {width}x{height} could not be held");
                    self.cx
                        .images
                        .upload_failed(key, crate::image::ImageError::TooLarge { width, height });
                }
            }
        }
        self.needs_redraw = true;
    }

    /// The interface as a screen reader sees it, and where the keyboard is.
    pub fn access(&self) -> (&crate::access::AccessTree, Option<GlobalElementId>) {
        (&self.cx.access, self.cx.input.focused)
    }

    /// Start or stop describing the interface.
    ///
    /// Off until something attaches, because building the description is real
    /// work on every frame and almost no run of an application needs it.
    pub fn set_accessible(&mut self, accessible: bool) {
        if self.cx.access.active != accessible {
            self.cx.access.active = accessible;
            self.cx.access.clear();
            self.needs_redraw = true;
        }
    }

    /// Carry out something a screen reader asked for.
    ///
    /// Returns whether anything came of it. A reader offers a person the
    /// actions an element advertised, so this is the other half of that
    /// promise: the same code path a pointer would have taken.
    pub fn perform_access_action(
        &mut self,
        action: accesskit::Action,
        element: GlobalElementId,
    ) -> bool {
        match action {
            accesskit::Action::Focus => {
                // Only somewhere the keyboard could have reached on its own.
                // Putting focus on something outside the focus order strands
                // it: the next Tab has nowhere to go from.
                let focusable = self
                    .cx
                    .access
                    .nodes
                    .iter()
                    .any(|node| node.id == element && node.focusable);
                if !focusable {
                    return false;
                }
                self.cx.input.focused = Some(element);
                // Focus arrived without the keyboard, but a reader's user is
                // navigating, so the ring belongs on screen.
                self.cx.input.focus_visible = true;
                self.needs_redraw = true;
                true
            }
            accesskit::Action::Click => {
                let handler = self
                    .cx
                    .handlers
                    .get(&element)
                    .and_then(|handlers| handlers.on_click.clone());
                let Some(handler) = handler else {
                    return false;
                };

                // Through the element's own handler, so an activation from a
                // reader and a click from a pointer are the same thing as far
                // as the application is concerned.
                let bounds = self
                    .cx
                    .hits
                    .iter()
                    .find(|region| region.id == element)
                    .map(|region| region.bounds)
                    .unwrap_or_default();
                let event = crate::event::MouseEvent {
                    position: bounds.center(),
                    local: guirs_core::Point::new(
                        bounds.size.width * 0.5,
                        bounds.size.height * 0.5,
                    ),
                    bounds,
                    button: Some(crate::event::MouseButton::Left),
                    modifiers: self.cx.input.modifiers,
                    click_count: 1,
                };

                let mut redraw = false;
                let mut quit = false;
                let mut context = crate::context::EventContext::new(
                    &mut self.cx.input,
                    &mut self.cx.state,
                    &mut redraw,
                    &mut quit,
                );
                handler(&event, &mut context);
                self.needs_redraw = true;
                self.should_close |= quit;
                // A handler may have asked for a command, which is what every
                // menu item does. Without this a reader could open a menu and
                // choose from it and nothing would happen.
                let queued = self.cx.drain_commands();
                self.needs_redraw |= queued.redraw || queued.handled;
                self.should_close |= queued.quit;
                true
            }
            _ => false,
        }
    }

    /// Where the window's maximize control was drawn, if it has one.
    pub fn snap_target(&self) -> Option<guirs_core::Bounds<Px>> {
        self.cx.snap_target
    }

    /// Where the focused field's caret is, for an input method's candidate
    /// window, or `None` when nothing that composes has focus.
    pub fn ime_area(&self) -> Option<guirs_core::Bounds<Px>> {
        self.cx.ime_area
    }

    /// Take the window operations handlers asked for.
    pub fn take_window_actions(&mut self) -> Vec<crate::event::WindowAction> {
        std::mem::take(&mut self.cx.input.window_actions)
    }

    /// Tell the tree whether the platform window is maximized.
    pub fn set_maximized(&mut self, maximized: bool) {
        if self.cx.window_maximized != maximized {
            self.cx.window_maximized = maximized;
            self.needs_redraw = true;
        }
    }

    /// Record a press for multi click detection, returning the click count.
    pub fn track_click(&mut self, position: Point<Px>) -> u32 {
        let now = self.start.elapsed().as_secs_f64();
        self.input.track_click(position, now)
    }
}

/// What an open window is holding, in bytes.
///
/// Separating these matters, because the answer to "why is this using memory"
/// is almost always one line of it rather than the total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryReport {
    /// Font files held resident for shaping and rasterizing.
    pub fonts: usize,
    /// Shaped runs and laid out blocks.
    pub text_caches: usize,
    /// Atlas and ramp textures, in graphics memory.
    pub textures: usize,
    /// Instance buffers.
    pub buffers: usize,
    /// Retained per element state and transition tracks.
    pub element_state: usize,
    pub atlas_entries: usize,
    pub cached_layouts: usize,
    pub cached_runs: usize,
    pub faces: usize,
}

impl MemoryReport {
    /// Everything except graphics memory, which is not on the process heap.
    pub fn heap(&self) -> usize {
        self.fonts + self.text_caches + self.buffers + self.element_state
    }

    pub fn total(&self) -> usize {
        self.heap() + self.textures
    }
}

/// Build a style engine from stylesheet source, logging anything wrong with it.
pub fn engine_from_source(source: &str) -> StyleEngine {
    let sheet = Stylesheet::parse(source);
    for error in &sheet.errors {
        log::warn!("stylesheet: {error}");
    }
    StyleEngine::new(sheet)
}

/// Paint the root at the window origin.
pub fn window_bounds(size: Size<Px>) -> Bounds<Px> {
    Bounds::new(Point::zero(), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_stylesheet_still_produces_an_engine() {
        let engine = engine_from_source("button { color: ###; } div { background: #123456; }");
        assert!(engine.rule_count() >= 1);
    }
}
