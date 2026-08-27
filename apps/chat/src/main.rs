//! A chat application built on guirs.
//!
//! Everything here is Rust. The window, the layout, the text shaping and every
//! pixel drawn go through the framework in `crates/`. The look lives in
//! `assets/theme.gss`, which is watched while the application runs, so editing
//! it changes the window without a rebuild.

// A released build is a window, not a command. Without this the linker marks
// the binary as a console application and Windows opens a console behind it,
// which is the black rectangle that appears when one is launched from the
// desktop. Kept in debug builds, where that console is somewhere for a
// `println!` to land while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod markdown;
mod state;

use guirs::prelude::*;
use markdown::MarkdownTheme;
use state::{Chat, Message, Role};

/// The state handles the whole application shares.
///
/// Named `Handles` rather than `App` because the framework already has an
/// `App`, and this holds no logic of its own.
struct Handles {
    chat: Model<Chat>,
    composer: Model<TextInputState>,
}

impl Handles {
    fn new() -> Self {
        Handles {
            chat: Model::new(Chat::new()),
            composer: Model::new(TextInputState::new("")),
        }
    }
}

fn main() -> Result<(), AppError> {
    let app = Handles::new();

    App::new()
        .title("guirs chat")
        .icon(include_bytes!("../assets/icon.png"))
        .size(1100.0, 740.0)
        .min_size(640.0, 480.0)
        // No platform title bar: the one at the top of the window is drawn by
        // this application, so it can look like the rest of it.
        .undecorated()
        .stylesheet_file(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/theme.gss"))
        .run(move |cx| chat_window(&app, cx))
}

/// Draw one chat window.
///
/// A function rather than a closure in `main`, because a second window draws
/// itself the same way. Each window has its own `Handles`, which is what makes
/// two of them two conversations rather than two views of one.
fn chat_window(app: &Handles, cx: &mut Cx) -> AnyElement {
    // Keep the reply arriving. While anything is still being revealed the
    // window is asked for another frame.
    let now = cx.now;
    if app.chat.update(|chat| chat.advance_streaming(now)) {
        cx.request_animation();
    }

    let sidebar_open = app.chat.read().sidebar_open;
    let maximized = cx.window_maximized;
    // Inline code and links are styled from the same tokens as everything
    // else, so editing the stylesheet reaches them too.
    let fallback = MarkdownTheme::default();
    let markdown = MarkdownTheme {
        code_text: cx
            .styles
            .color_token("code-text")
            .unwrap_or(fallback.code_text),
        code_background: cx
            .styles
            .color_token("code-surface")
            .unwrap_or(fallback.code_background),
        link: cx.styles.color_token("link").unwrap_or(fallback.link),
    };
    let stats = format!(
        "{:.1} MB fonts  {:.1} MB caches  {:.1} MB atlas  {} glyphs  {:.0} fps",
        cx.memory.fonts as f64 / 1_048_576.0,
        cx.memory.text_caches as f64 / 1_048_576.0,
        cx.memory.textures as f64 / 1_048_576.0,
        cx.memory.atlas_entries,
        cx.fps,
    );

    let is_empty = app.chat.read().current().messages.is_empty();
    let dropped = app.composer.clone();

    div()
        .class("root")
        .size_full()
        .col()
        .relative()
        // Files dropped anywhere in the window land in the composer. The whole
        // window is the target because a person aiming at a small one has to
        // think about where they are letting go.
        .on_file_drop(move |event, cx| {
            let names = attachment_summary(&event.paths);
            if names.is_empty() {
                return;
            }
            dropped.update(|state| state.insert(&names));
            cx.request_redraw();
        })
        .child(window_bar(app, maximized, &stats))
        .child(
            div()
                .row()
                .flex_1()
                .items_stretch()
                .child(when(sidebar_open, || sidebar(app)))
                .child(
                    div()
                        .col()
                        .flex_1()
                        .child(header(app))
                        // With nothing said yet, the greeting and the composer
                        // are one group in the middle of the window. Once
                        // there is a transcript the composer drops to the
                        // bottom and the transcript takes the room.
                        .child(when(is_empty, || {
                            div()
                                .class("stage")
                                .flex_1()
                                .col()
                                .child(
                                    text("What can I help with?")
                                        .class("greeting")
                                        .whitespace_nowrap(),
                                )
                                .child(composer(app, now))
                        }))
                        .child(when(!is_empty, || thread(app, markdown)))
                        .child(when(!is_empty, || composer(app, now)))
                        // Pinned to the bottom whether or not anything has
                        // been said, which is where the reference puts it.
                        .child(
                            text("guirs chat can make mistakes. Check important info.")
                                .class("disclaimer")
                                .whitespace_nowrap(),
                        ),
                ),
        )
        // Last, so the grab strips sit above everything else.
        .child(resize_borders())
        .into_any()
}

// ---------------------------------------------------------------------------
// Window bar
// ---------------------------------------------------------------------------

fn window_bar(app: &Handles, maximized: bool, stats: &str) -> Div {
    let _ = app;

    title_bar()
        .class("window-bar")
        .child(icon(icons::ring()).class("window-mark"))
        .child(text("guirs chat").class("window-brand").whitespace_nowrap())
        .child(flex_spacer())
        // The reference has nothing here, so neither does this. The numbers are
        // still a frame away in `cx.memory` for anyone who wants them.
        .child(when(false, || {
            text(stats.to_string()).class("window-stats").whitespace_nowrap()
        }))
        .child(window_controls(maximized))
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

fn sidebar(app: &Handles) -> Div {
    let chat = app.chat.read();
    let active = chat.active;
    let new_chat = app.chat.clone();
    let has_history = chat.conversations.iter().any(|c| !c.messages.is_empty());

    let entries: Vec<Div> = chat
        .conversations
        .iter()
        .enumerate()
        .filter(|(_, conversation)| !conversation.messages.is_empty())
        .map(|(index, conversation)| {
            let select = app.chat.clone();
            div()
                .class("conversation")
                .class_if(index == active, "active")
                .col()
                .gap(2.0)
                .cursor_pointer()
                .child(
                    text(conversation.title.clone())
                        .class("conversation-title")
                        .text_ellipsis(),
                )
                .child(
                    text(conversation.preview())
                        .class("conversation-preview")
                        .text_ellipsis(),
                )
                .on_click(move |_, _| select.update(|chat| chat.active = index))
        })
        .collect();

    let toggle = app.chat.clone();

    div()
        .class("sidebar")
        .col()
        .child(
            div()
                .class("sidebar-top")
                .row()
                .child(
                    div()
                        .class("icon-button")
                        .row()
                        .center()
                        .cursor_pointer()
                        .child(icon(icons::sidebar()))
                        .label("Toggle sidebar")
                        .on_click(move |_, _| {
                            toggle.update(|chat| chat.sidebar_open = !chat.sidebar_open)
                        }),
                )
                .child(flex_spacer())
                .child(
                    div()
                        .class("icon-button")
                        .row()
                        .center()
                        .cursor_pointer()
                        .child(icon(icons::search())),
                )
                .child(
                    div()
                        .class("icon-button")
                        .row()
                        .center()
                        .cursor_pointer()
                        .child(icon(icons::compose()))
                        .label("New chat")
                        .on_click(move |_, _| new_chat.update(Chat::new_conversation)),
                ),
        )
        .child(nav_item(icons::ring(), "guirs chat", true))
        .child(nav_item(icons::orb(), "Canvas", false))
        .child(nav_item(icons::grid(), "Explore", false))
        .child(when(has_history, || {
            text("Chats").class("sidebar-label").whitespace_nowrap()
        }))
        .child(
            scroll_view()
                .class("conversation-list")
                .flex_1()
                .children(entries),
        )
}

/// One of the fixed destinations at the top of the sidebar.
fn nav_item(glyph: Icon, label: &str, active: bool) -> Div {
    div()
        .class("nav-item")
        .class_if(active, "active")
        .row()
        .cursor_pointer()
        .child(icon(glyph))
        .child(text(label.to_string()).class("nav-label").whitespace_nowrap())
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn header(app: &Handles) -> Div {
    let _ = app;

    div()
        .class("header")
        .row()
        .child(
            div()
                .class("model-picker")
                .row()
                .cursor_pointer()
                .child(text("guirs chat").class("model-name").whitespace_nowrap())
                .child(icon(icons::chevron_down())),
        )
        .child(flex_spacer())
        .child(
            div()
                .class("pill")
                .row()
                .cursor_pointer()
                .child(icon(icons::dashed_circle()))
                .child(text("Temporary").whitespace_nowrap()),
        )
        .child(div().class("avatar"))
}

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

fn thread(app: &Handles, markdown: MarkdownTheme) -> Div {
    let chat = app.chat.read();
    let conversation = chat.current();

    if conversation.messages.is_empty() {
        return div()
            .class("thread")
            .flex_1()
            .col()
            .center()
            .child(empty_state());
    }

    // Built one screenful at a time rather than all at once. A frame costs
    // what is on screen, not what the conversation has come to: laying out a
    // message means wrapping and measuring every line of it, and doing that
    // for a hundred of them every frame is what made a long conversation
    // slow. The list measures each message as it builds it and remembers the
    // answer, so nothing here has to work out how tall a message is.
    let count = conversation.messages.len();
    let chat = app.chat.clone();
    let last = count.saturating_sub(1);

    scroll_view()
        .class("thread")
        .flex_1()
        .stick_to_bottom()
        .virtual_rows_measured(count, px(TURN_ESTIMATE), move |index| {
            let chat = chat.read();
            let Some(message) = chat.current().messages.get(index) else {
                return div().into_any();
            };
            div()
                .class("thread-row")
                .class_if(index == 0, "first")
                .class_if(index == last, "last")
                .col()
                .items_center()
                .child(
                    div()
                        .class("thread-row-inner")
                        .col()
                        .child(turn(message, &markdown)),
                )
                .into_any()
        })
}

/// What a message is assumed to be worth before anyone has looked at it.
///
/// Only ever wrong about messages nobody has scrolled to, and only until they
/// are scrolled to. A rough average of a real turn keeps the scrollbar honest
/// in the meantime.
const TURN_ESTIMATE: f32 = 150.0;

fn empty_state() -> Div {
    div()
        .class("stage")
        .flex_1()
        .col()
        .child(
            text("What can I help with?")
                .class("greeting")
                .whitespace_nowrap(),
        )
}

fn turn(message: &Message, markdown: &MarkdownTheme) -> Div {
    let is_you = message.role == Role::You;
    // What someone typed is shown exactly as typed. Only a reply is read as
    // markup, so a message about asterisks does not rewrite itself.
    let body = if is_you {
        RichText::plain(message.visible())
    } else {
        markdown::to_rich(message.visible(), markdown)
    };

    // A trailing caret while the reply is still arriving.
    let caret = message.is_streaming();

    div()
        .class("turn")
        .class_if(is_you, "from-you")
        .class_if(!is_you, "from-assistant")
        .row()
        .items_start()
        .gap(12.0)
        .when(!is_you, |turn| turn.child(div().class("avatar")))
        .child(
            div()
                .col()
                .gap(4.0)
                .flex_1()
                .items(if is_you {
                    AlignItems::End
                } else {
                    AlignItems::Start
                })
                .child(
                    text(message.role.label())
                        .class("turn-role")
                        .whitespace_nowrap(),
                )
                .child(
                    div()
                        .class("bubble")
                        .class_if(is_you, "bubble-you")
                        .col()
                        .child(
                            // Selectable, because the whole point of a reply is
                            // being able to take it somewhere else.
                            rich_text(body)
                                .class("bubble-text")
                                .selectable()
                                .on_link(|href, _| open_in_browser(href)),
                        )
                        .when(caret, |bubble| bubble.child(div().class("caret"))),
                ),
        )
        .when(is_you, |turn| turn.child(div().class("avatar avatar-you")))
}

/// Hand a link to whatever the system opens links with.
///
/// The framework reports that a link was clicked and stops there, because what
/// a link means is the application's decision. Here it means what it means in
/// a chat window.
fn open_in_browser(href: &SharedString) {
    use std::process::Command;

    let result = if cfg!(target_os = "windows") {
        // The empty argument is the window title that `start` expects first,
        // without which a quoted target would be taken as one.
        Command::new("cmd")
            .args(["/C", "start", "", href.as_str()])
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(href.as_str()).spawn()
    } else {
        Command::new("xdg-open").arg(href.as_str()).spawn()
    };

    if let Err(error) = result {
        eprintln!("could not open {href}: {error}");
    }
}

// ---------------------------------------------------------------------------
// Composer
// ---------------------------------------------------------------------------

fn composer(app: &Handles, now: f64) -> Div {
    let composer = app.composer.clone();
    let has_text = !composer.read().value.trim().is_empty();

    let on_enter = clone_sender(app, now);
    let on_send = clone_sender(app, now);
    let attach = app.composer.clone();

    div()
        .class("composer")
        .col()
        .child(
            div()
                .class("composer-box")
                .class_if(has_text, "filled")
                .col()
                .child(
                    text_input(app.composer.clone(), "Ask anything")
                        .class("composer-input")
                        .w_full()
                        .autofocus()
                        .on_key_down(move |event, cx| {
                            if event.key == Key::Enter && !event.modifiers.shift {
                                on_enter();
                                cx.request_redraw();
                            }
                        }),
                )
                .child(
                    div()
                        .class("composer-tools")
                        .row()
                        .child(
                            div()
                                .class("tool-round")
                                .row()
                                .center()
                                .cursor_pointer()
                                .child(icon(icons::plus()))
                                .label("Attach files")
                                .on_click(move |_, cx| {
                                    // The platform's own picker, because that
                                    // is where a person keeps their recent
                                    // places and their network drives.
                                    if let Some(paths) = FileDialog::new()
                                        .title("Attach files")
                                        .open_files()
                                    {
                                        let names = attachment_summary(&paths);
                                        if !names.is_empty() {
                                            attach.update(|state| state.insert(&names));
                                            cx.request_redraw();
                                        }
                                    }
                                }),
                        )
                        .child(tool(icons::globe(), "Search", false))
                        .child(tool(icons::research(), "Deep research", true))
                        .child(
                            div()
                                .class("tool")
                                .row()
                                .center()
                                .cursor_pointer()
                                .child(icon(icons::ellipsis())),
                        )
                        .child(flex_spacer())
                        .child(when(!has_text, || {
                            div()
                                .class("voice")
                                .row()
                                .center()
                                .cursor_pointer()
                                .child(icon(icons::waveform()))
                        }))
                        .child(when(has_text, || {
                            div()
                                .class("send ready")
                                .row()
                                .center()
                                .cursor_pointer()
                                .child(icon(icons::arrow_up()).class("send-icon"))
                                .label("Send")
                                .on_click(move |_, cx| {
                                    on_send();
                                    cx.request_redraw();
                                })
                        })),
                ),
        )
}

/// One of the labelled buttons under the composer.
fn tool(glyph: Icon, label: &str, marked: bool) -> Div {
    div()
        .class("tool")
        .class_if(marked, "marked")
        .row()
        .center()
        .cursor_pointer()
        .child(icon(glyph))
        .child(text(label.to_string()).whitespace_nowrap())
}

/// Name the files an attachment refers to, for putting in the composer.
fn attachment_summary(paths: &[std::path::PathBuf]) -> String {
    let names: Vec<String> = paths
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        _ => names.join(", "),
    }
}

/// A send callback that owns its own handles.
fn clone_sender(app: &Handles, now: f64) -> impl Fn() + 'static {
    let chat = app.chat.clone();
    let composer = app.composer.clone();
    move || {
        let text = composer.read().value.clone();
        if text.trim().is_empty() {
            return;
        }
        chat.update(|chat| chat.send(&text, now));
        composer.set(TextInputState::new(""));
    }
}
