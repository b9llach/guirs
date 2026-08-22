//! A gallery of every built in widget, styled entirely from `assets/theme.gss`.
//!
//! Run it, then edit the stylesheet while it is open. The file is watched and
//! reloaded, so colors, radii, spacing and transitions all change live.

// A released build is a window, not a command. Without this the linker marks
// the binary as a console application and Windows opens a console behind it,
// which is the black rectangle that appears when one is launched from the
// desktop. Kept in debug builds, where that console is somewhere for a
// `println!` to land while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use guirs::prelude::*;

/// Everything the interface remembers between frames.
struct State {
    nav: Model<usize>,
    tab: Model<usize>,
    notifications: Model<bool>,
    telemetry: Model<bool>,
    dark_mode: Model<bool>,
    density: Model<usize>,
    volume: Model<f32>,
    theme: Model<usize>,
    theme_open: Model<bool>,
    search: Model<TextInputState>,
    notes: Model<TextInputState>,
    clicks: Model<u32>,
    menus: Model<MenuState>,
    /// What the last command asked for, so choosing one visibly does
    /// something rather than being taken on faith.
    last_command: Model<String>,
    split_outer: Model<SplitState>,
    split_inner: Model<SplitState>,
}

impl State {
    fn new() -> Self {
        State {
            nav: Model::new(0),
            tab: Model::new(0),
            notifications: Model::new(true),
            telemetry: Model::new(false),
            dark_mode: Model::new(true),
            density: Model::new(1),
            volume: Model::new(0.62),
            theme: Model::new(0),
            theme_open: Model::new(false),
            search: Model::new(TextInputState::new("")),
            notes: Model::new(TextInputState::new(
                "An area wraps at its own width and grows as lines are added.\n\nEnter starts a new one. Up and down move between them, and keep the column they set out from.",
            )),
            clicks: Model::new(0),
            menus: Model::new(MenuState::default()),
            last_command: Model::new(String::from("nothing yet")),
            split_outer: Model::new(SplitState::new(200.0).range(120.0, 420.0)),
            split_inner: Model::new(SplitState::new(150.0).range(60.0, 300.0)),
        }
    }
}

const NAV: [&str; 6] = [
    "Overview",
    "Components",
    "Typography",
    "Layout",
    "Motion",
    "Settings",
];
const TABS: [&str; 3] = ["Controls", "Surfaces", "Data"];
const THEMES: [&str; 4] = ["Midnight", "Slate", "Nord", "Solarized"];
const DENSITY: [&str; 3] = ["Compact", "Comfortable", "Spacious"];

fn main() -> Result<(), AppError> {
    let state = State::new();

    App::new()
        .title("guirs kitchen sink")
        .size(1180.0, 780.0)
        .min_size(720.0, 520.0)
        .stylesheet_file(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/theme.gss"))
        // One keymap for the whole application. The menu reads it to show
        // each entry's shortcut, so neither has to repeat the other.
        .keymap(
            Keymap::new()
                .bind("cmd-n", "file:new")
                .bind("cmd-o", "file:open")
                .bind("cmd-s", "file:save")
                .bind("cmd-shift-s", "file:save-as")
                .bind("cmd-z", "edit:undo")
                .bind("cmd-shift-z", "edit:redo")
                .bind("cmd-k cmd-d", "edit:duplicate")
                .bind("f11", "view:full-screen"),
        )
        // Run `GUIRS_NO_VSYNC=1 kitchen-sink` to see the uncapped frame cost.
        .when(std::env::var_os("GUIRS_NO_VSYNC").is_some(), App::without_vsync)
        .run(move |cx| {
            let fps = cx.fps;
            let frame_ms = cx.frame_ms;
            let phases = format!(
                "build {:.1} layout {:.1} paint {:.1} | prep {:.1} wait {:.1} submit {:.1} | rast {}",
                cx.build_ms,
                cx.layout_ms,
                cx.paint_ms,
                cx.prepare_ms,
                cx.acquire_ms,
                cx.submit_ms,
                cx.rasterized
            );
            div()
                .class("root")
                .size_full()
                .col()
                .child(titlebar(&state, fps, frame_ms, &phases, &cx.keymap))
                .child(
                    div()
                        .row()
                        .flex_1()
                        .items_stretch()
                        .child(sidebar(&state, fps))
                        .child(content(&state)),
                )
                .into_any()
        })
}

fn titlebar(state: &State, fps: f32, frame_ms: f32, phases: &str, keymap: &Keymap) -> Div {
    let clicks = state.clicks.clone();

    div()
        .class("titlebar")
        .row()
        .items_center()
        .gap(10.0)
        .child(text("guirs").class("title").whitespace_nowrap())
        .child(text("kitchen sink").class("muted small").whitespace_nowrap())
        .child(text(format!("v{}", guirs::VERSION)).class("badge").whitespace_nowrap())
        .child(menus(state, keymap))
        .child(flex_spacer())
        .child(
            text(format!("last command: {}", state.last_command.read()))
                .class("mono small muted")
                .whitespace_nowrap(),
        )
        .child(text(phases).class("mono small muted").whitespace_nowrap())
        .child(
            text(format!("{fps:.0} fps  {frame_ms:.1} ms"))
                .class("mono small muted")
                .whitespace_nowrap(),
        )
        .child(text(format!("{} clicks", clicks.get())).class("muted small").whitespace_nowrap())
}

/// The menu bar, and the handlers for everything it can ask for.
///
/// Each entry names a command and nothing else. The shortcuts beside them are
/// read from the keymap, so rebinding a key changes the menu with it.
fn menus(state: &State, keymap: &Keymap) -> Div {
    let items = [
        submenu(
            "File",
            [
                menu_item("New").command("file:new"),
                menu_item("Open").command("file:open"),
                submenu(
                    "Open Recent",
                    [
                        menu_item("kitchen-sink.rs").command("file:open-recent"),
                        menu_item("theme.gss").command("file:open-recent"),
                    ],
                ),
                menu_separator(),
                menu_item("Save").command("file:save"),
                menu_item("Save As").command("file:save-as"),
                menu_separator(),
                menu_item("Quit").command("app:quit"),
            ],
        ),
        submenu(
            "Edit",
            [
                menu_item("Undo").command("edit:undo"),
                menu_item("Redo").command("edit:redo").enabled(false),
                menu_separator(),
                menu_item("Duplicate Line").command("edit:duplicate"),
            ],
        ),
        submenu(
            "View",
            [
                menu_item("Dark Mode").command("view:dark").checked(state.dark_mode.get()),
                menu_item("Full Screen").command("view:full-screen"),
            ],
        ),
    ];

    let heard = state.last_command.clone();
    let dark = state.dark_mode.clone();

    menu_bar(&items, state.menus.clone(), keymap)
        // Every command lands here. An application would answer each one
        // somewhere meaningful; this shows what arrived, which is the part
        // worth seeing.
        .on_command("file:new", command_echo(&heard, "file:new"))
        .on_command("file:open", command_echo(&heard, "file:open"))
        .on_command("file:open-recent", command_echo(&heard, "file:open-recent"))
        .on_command("file:save", command_echo(&heard, "file:save"))
        .on_command("file:save-as", command_echo(&heard, "file:save-as"))
        .on_command("edit:undo", command_echo(&heard, "edit:undo"))
        .on_command("edit:duplicate", command_echo(&heard, "edit:duplicate"))
        .on_command("view:full-screen", command_echo(&heard, "view:full-screen"))
        .on_command("app:quit", |cx| cx.quit())
        .on_command("view:dark", move |_| dark.update(|on| *on = !*on))
}

fn command_echo(heard: &Model<String>, name: &'static str) -> impl Fn(&mut EventContext) {
    let heard = heard.clone();
    move |_| heard.set(name.to_string())
}

fn sidebar(state: &State, fps: f32) -> Div {
    let selected = state.nav.get();

    panel()
        .class("sidebar")
        .child(text("Navigation").class("section-title"))
        .children(NAV.iter().enumerate().map(|(index, name)| {
            let nav = state.nav.clone();
            list_item(index == selected)
                .child(text(*name).whitespace_nowrap())
                .on_click(move |_, _| nav.set(index))
        }))
        .child(flex_spacer())
        .child(div().class("divider"))
        .child(
            div()
                .class("stat")
                .col()
                .child(text(format!("{:.0}", fps)).class("stat-value").whitespace_nowrap())
                .child(text("frames per second").class("stat-label").whitespace_nowrap()),
        )
}

fn content(state: &State) -> Div {
    // The sidebar picks the section. Tabs only appear inside Components,
    // because nesting one navigation inside another that leads to the same
    // place is how a demo ends up with controls that do nothing.
    match state.nav.get() {
        0 => overview_section(state),
        1 => components_section(state),
        2 => typography_section(),
        3 => layout_section(state),
        4 => motion_section(),
        _ => settings_section(state),
    }
}

fn page() -> Div {
    scroll_view().class("content").flex_1()
}

fn section(title: &str, subtitle: &str) -> Div {
    column()
        .gap(3.0)
        .child(text(title.to_string()).class("title").whitespace_nowrap())
        .child(text(subtitle.to_string()).class("muted small"))
}

fn overview_section(state: &State) -> Div {
    let stats: [(&str, &str); 4] = [
        ("6", "crates"),
        ("438", "tests"),
        ("2", "shaders"),
        ("21", "widgets"),
    ];

    page()
        .child(section(
            "Overview",
            "A GPU accelerated interface framework written entirely in Rust.",
        ))
        .child(
            row()
                .gap(12.0)
                .wrap()
                .children(stats.iter().map(|(value, label)| {
                    div()
                        .class("stat")
                        .col()
                        .min_w(px(132.0))
                        .child(text(*value).class("stat-value").whitespace_nowrap())
                        .child(text(*label).class("stat-label").whitespace_nowrap())
                })),
        )
        .child(
            card()
                .child(text("What is drawing this").class("section-title"))
                .child(text(
                    "Two pipelines. One draws rounded boxes, the other draws sprites. \
                     Borders, per corner radii, gradients and Gaussian shadows are all \
                     evaluated analytically in a fragment shader, so nothing is \
                     tessellated and antialiasing stays exact at any scale factor.",
                ))
                .child(progress(state.volume.get())),
        )
}

fn components_section(state: &State) -> Div {
    let tab = state.tab.get();
    let set_tab = state.tab.clone();

    page()
        .child(section(
            "Components",
            "Every widget the framework ships with.",
        ))
        .child(tab_bar(&TABS, tab, move |index| set_tab.set(index)))
        .child(match tab {
            0 => controls_tab(state),
            1 => surfaces_tab(state),
            _ => data_tab(state),
        })
}

fn typography_section() -> Div {
    let sizes: [(&str, f32, FontWeight); 5] = [
        ("Display", 30.0, FontWeight::SEMIBOLD),
        ("Title", 21.0, FontWeight::SEMIBOLD),
        ("Body", 14.0, FontWeight::NORMAL),
        ("Small", 12.0, FontWeight::NORMAL),
        ("Caption", 11.0, FontWeight::MEDIUM),
    ];

    page()
        .child(section(
            "Typography",
            "Shaped with the font's own tables, so ligatures and complex scripts work.",
        ))
        .child(
            card()
                .gap(14.0)
                .child(text("Scale").class("section-title"))
                .children(sizes.iter().map(|(name, size, weight)| {
                    row()
                        .gap(18.0)
                        .items_baseline()
                        .child(
                            text(*name)
                                .class("muted small")
                                .w(px(74.0))
                                .whitespace_nowrap(),
                        )
                        .child(
                            text("The quick brown fox")
                                .text_size(*size)
                                .font_weight(*weight)
                                .whitespace_nowrap(),
                        )
                })),
        )
        .child(
            card()
                .gap(10.0)
                .child(text("Scripts and shaping").class("section-title"))
                .child(text("fi ffl  x != y  ->").class("mono").selectable())
                .child(
                    text("\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}")
                        .selectable(),
                )
                .child(
                    text("\u{05e9}\u{05dc}\u{05d5}\u{05dd}  \u{0645}\u{0631}\u{062d}\u{0628}\u{0627}")
                        .selectable(),
                )
                .child(
                    text("Select any of these lines and copy them.").class("muted small"),
                ),
        )
        .child(
            card()
                .gap(10.0)
                .child(text("One paragraph, several styles").class("section-title"))
                .child(
                    rich_text(
                        rich("A span describes only what differs, so ")
                            .bold("bold")
                            .push(", ")
                            .italic("italic")
                            .push(", a piece of code like ")
                            .code("Corners::all(px(10.0))")
                            .push(" and a ")
                            .link("link", "https://doc.rust-lang.org/book/")
                            .push(
                                " all sit on one baseline and wrap together. They are \
                                 spans of a single laid out paragraph rather than \
                                 separate elements placed in a row, which is why the \
                                 line breaker can measure each piece with its own font.",
                            ),
                    )
                    .selectable()
                    .on_link(|href, _| println!("link: {href}")),
                )
                .child(
                    text("Resize the window: the whole paragraph reflows as one block.")
                        .class("muted small"),
                ),
        )
}

fn layout_section(state: &State) -> Div {
    fn block(color: u32, width: f32) -> Div {
        div().w(px(width)).h(px(34.0)).rounded(6.0).bg(rgb(color))
    }

    page()
        .child(section("Layout", "Flexbox, the way a browser does it."))
        .child(
            card()
                .gap(12.0)
                .child(text("Split panes").class("section-title"))
                .child(
                    text("Drag either divider. Nested, so the right hand side is itself split.")
                        .class("muted"),
                )
                .child(
                    div().class("split-demo").child(split_x(
                        state.split_outer.clone(),
                        column()
                            .gap(6.0)
                            .child(text("Sidebar").class("section-title"))
                            .child(text("Fixed width, kept when the window resizes.").class("muted small")),
                        split_y(
                            state.split_inner.clone(),
                            column()
                                .gap(6.0)
                                .child(text("Editor").class("section-title"))
                                .child(text("Takes whatever is left over.").class("muted small")),
                            column()
                                .gap(6.0)
                                .child(text("Panel").class("section-title"))
                                .child(text("Dragged up and down.").class("muted small")),
                        ),
                    )),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("Row with gap").class("section-title"))
                .child(
                    row()
                        .gap(8.0)
                        .child(block(0x6c5ce7, 70.0))
                        .child(block(0x00cec9, 70.0))
                        .child(block(0xe05561, 70.0)),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("Grow and shrink").class("section-title"))
                .child(
                    row()
                        .gap(8.0)
                        .child(block(0x6c5ce7, 60.0))
                        .child(div().h(px(34.0)).rounded(6.0).bg(rgb(0x00cec9)).flex_1())
                        .child(block(0xe05561, 60.0)),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("Wrapping").class("section-title"))
                .child(row().gap(8.0).wrap().children((0..9).map(|i| {
                    block(if i % 2 == 0 { 0x6c5ce7 } else { 0x2c2c40 }, 108.0)
                }))),
        )
}

fn motion_section() -> Div {
    page()
        .child(section(
            "Motion",
            "Transitions come from the stylesheet, not from code.",
        ))
        .child(
            card()
                .gap(12.0)
                .child(text("Hover these").class("section-title"))
                .child(
                    row().gap(12.0).wrap().children(
                        [0x6c5ce7u32, 0x00cec9, 0x3fb950, 0xe05561]
                            .iter()
                            .map(|color| div().class("swatch").bg(rgb(*color))),
                    ),
                )
                .child(
                    text("border-radius eases from a square to a circle over 220ms.")
                        .class("muted small"),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("Interrupting a transition").class("section-title"))
                .child(
                    row()
                        .gap(10.0)
                        .wrap()
                        .child(button("Hover and leave quickly").class("primary"))
                        .child(button("Then do it again")),
                )
                .child(
                    text(
                        "Reversing partway resumes from what is on screen rather than \
                         snapping back to where it started.",
                    )
                    .class("muted small"),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("Transforms").class("section-title"))
                .child(
                    row()
                        .gap(18.0)
                        .items_center()
                        .wrap()
                        .child(div().class("tile").rotate(12.0).child(text("rotate")))
                        .child(div().class("tile").scale(1.15).child(text("scale")))
                        .child(
                            div()
                                .class("tile")
                                .translate(px(0.0), px(-6.0))
                                .child(text("translate")),
                        )
                        .child(
                            div()
                                .class("tile")
                                .rotate(-8.0)
                                .scale(0.9)
                                .transform_origin(0.0, 1.0)
                                .child(text("origin")),
                        ),
                )
                .child(
                    text(
                        "Painting only. Layout has already run, so a transform moves \
                         nothing around it and costs no measuring. Press a button \
                         anywhere in this window to see one animate.",
                    )
                    .class("muted small"),
                ),
        )
}

fn settings_section(state: &State) -> Div {
    let notifications = state.notifications.get();
    let set_notifications = state.notifications.clone();
    let telemetry = state.telemetry.get();
    let set_telemetry = state.telemetry.clone();
    let dark = state.dark_mode.get();
    let set_dark = state.dark_mode.clone();
    let theme = state.theme.get();
    let set_theme = state.theme.clone();

    page()
        .child(section("Settings", "Controls wired to real state."))
        .child(
            card()
                .gap(12.0)
                .child(text("Preferences").class("section-title"))
                .child(checkbox("Notifications", notifications, move |on| {
                    set_notifications.set(on)
                }))
                .child(checkbox("Send telemetry", telemetry, move |on| {
                    set_telemetry.set(on)
                }))
                .child(
                    row()
                        .gap(12.0)
                        .child(text("Dark mode").w(px(110.0)).whitespace_nowrap())
                        .child(toggle(dark, move |on| set_dark.set(on))),
                )
                .child(
                    row()
                        .gap(12.0)
                        .child(text("Theme").w(px(110.0)).whitespace_nowrap())
                        .child(select(
                            &THEMES,
                            theme,
                            state.theme_open.clone(),
                            move |index| set_theme.set(index),
                        )),
                ),
        )
        .child(
            card()
                .gap(10.0)
                .child(text("More than one line").class("section-title"))
                .child(
                    text_area(state.notes.clone(), "Notes\u{2026}")
                        .class("notes")
                        .w_full()
                        .max_h(px(150.0)),
                )
                .child(
                    text("Select across the lines, and paste something with newlines in it.")
                        .class("muted small"),
                ),
        )
        .child(
            card()
                .gap(12.0)
                .child(text("The desktop").class("section-title"))
                .child(
                    row()
                        .gap(10.0)
                        .wrap()
                        .child(button("Open a file\u{2026}").on_click(|_, _| {
                            // The platform's own picker rather than one drawn
                            // here, because that is where a person keeps their
                            // recent places and their network drives.
                            match FileDialog::new()
                                .title("Pick any file")
                                .open_file()
                            {
                                Some(path) => println!("picked {}", path.display()),
                                None => println!("cancelled"),
                            }
                        }))
                        .child(button("Ask something").on_click(|_, _| {
                            let yes = confirm("Kitchen sink", "Is this dialog native?");
                            println!("answered {yes}");
                        }))
                        .child(button("Second window").class("primary").on_click(
                            |_, cx| {
                                // The state belongs to the window, so it is made
                                // here and moved into the closure. Making it
                                // inside the closure would reset it every frame.
                                let clicks = Model::new(0i32);
                                cx.open_window(
                                    WindowOptions::new("kitchen sink: a peer")
                                        .sized(520.0, 380.0),
                                    move |_| second_window(&clicks).into_any(),
                                )
                            },
                        )),
                )
                .child(
                    text(
                        "The second window is a peer rather than a child: its own \
                         state and focus, the same stylesheet, and closing this one \
                         leaves it open.",
                    )
                    .class("muted small"),
                ),
        )
        .child(
            card()
                .gap(10.0)
                .child(text("Files from the desktop").class("section-title"))
                .child(
                    div()
                        .class("drop-target")
                        .h(px(84.0))
                        .col()
                        .center()
                        .child(text("Drag files here").class("muted"))
                        .on_file_drop(|event, _| {
                            for path in &event.paths {
                                println!("dropped {}", path.display());
                            }
                        }),
                )
                .child(
                    text(
                        "One event carries every path, because a drop of four files \
                         is one gesture rather than four. Paths are printed to the \
                         terminal.",
                    )
                    .class("muted small"),
                ),
        )
}

/// What the second window draws.
///
/// Deliberately not the gallery again: two windows showing the same thing
/// would not make it obvious that they keep their own state.
fn second_window(count: &Model<i32>) -> Div {
    let bump = count.clone();

    div()
        .class("root")
        .size_full()
        .col()
        .center()
        .gap(14.0)
        .child(text("A peer window").class("section-title"))
        .child(
            text(
                "Its own element state, scroll positions and focus. It shares the \
                 stylesheet, so editing the theme file changes both.",
            )
            .class("muted small")
            .w(px(360.0)),
        )
        .child(text(format!("Clicked {} times", count.get())).class("mono"))
        .child(
            button("Click me")
                .class("primary")
                .on_click(move |_, cx| {
                    bump.update(|n| *n += 1);
                    cx.request_redraw();
                }),
        )
}

fn controls_tab(state: &State) -> Div {
    let bump = state.clicks.clone();
    let bump_primary = state.clicks.clone();
    let notifications = state.notifications.get();
    let set_notifications = state.notifications.clone();
    let telemetry = state.telemetry.get();
    let set_telemetry = state.telemetry.clone();
    let dark = state.dark_mode.get();
    let set_dark = state.dark_mode.clone();
    let density = state.density.get();
    let volume = state.volume.get();
    let set_volume = state.volume.clone();
    let theme = state.theme.get();
    let set_theme = state.theme.clone();

    column()
        .gap(18.0)
        .child(
            card()
                .child(text("Buttons").class("section-title"))
                .child(
                    row()
                        .gap(10.0)
                        .wrap()
                        .child(
                            button("Primary")
                                .class("primary")
                                .on_click(move |_, _| bump_primary.update(|n| *n += 1)),
                        )
                        .child(button("Secondary").on_click(move |_, _| bump.update(|n| *n += 1)))
                        .child(button("Danger").class("danger"))
                        .child(text_button("Ghost"))
                        .child(button("Pill").class("pill")),
                )
                .child(
                    text("Hover and press them. Every transition comes from the stylesheet.")
                        .class("muted small"),
                ),
        )
        .child(
            card()
                .child(text("Toggles").class("section-title"))
                .child(
                    row()
                        .gap(28.0)
                        .wrap()
                        .child(
                            column()
                                .gap(6.0)
                                .child(checkbox("Notifications", notifications, move |on| {
                                    set_notifications.set(on)
                                }))
                                .child(checkbox("Telemetry", telemetry, move |on| {
                                    set_telemetry.set(on)
                                }))
                                .child(checkbox("Disabled option", false, |_| {})),
                        )
                        .child(
                            column().gap(6.0).children(
                                DENSITY.iter().enumerate().map(|(index, name)| {
                                    let set_density = state.density.clone();
                                    radio(*name, index == density, move || set_density.set(index))
                                }),
                            ),
                        )
                        .child(
                            row()
                                .gap(10.0)
                                .child(text("Dark mode"))
                                .child(toggle(dark, move |on| set_dark.set(on))),
                        ),
                ),
        )
        .child(
            card()
                .child(text("Ranges").class("section-title"))
                .child(
                    row()
                        .gap(14.0)
                        .child(text("Volume").class("muted small").w(px(56.0)))
                        .child(slider(volume, move |value| set_volume.set(value)).flex_1())
                        .child(
                            text(format!("{:>3}%", (volume * 100.0).round() as i32))
                                .class("mono small")
                                .w(px(44.0)),
                        ),
                )
                .child(progress(volume)),
        )
        .child(
            card()
                .child(text("Fields").class("section-title"))
                .child(
                    row()
                        .gap(14.0)
                        .wrap()
                        .child(text_input(state.search.clone(), "Search components"))
                        .child(select(
                            &THEMES,
                            theme,
                            state.theme_open.clone(),
                            move |index| set_theme.set(index),
                        )),
                )
                .child(
                    text("Click the field and type. Arrow keys, shift selection, Home, End and Ctrl-A all work.")
                        .class("muted small"),
                ),
        )
}

/// A picture, drawn four ways.
///
/// The sample is wider than the boxes it is put in, so every mode looks
/// different: the difference between them is the whole point of having them.
static SAMPLE: &[u8] = include_bytes!("../assets/sample.png");

fn images_card() -> Div {
    let modes: [(&str, ObjectFit); 4] = [
        ("contain", ObjectFit::Contain),
        ("cover", ObjectFit::Cover),
        ("fill", ObjectFit::Fill),
        ("none", ObjectFit::None),
    ];

    let mut gallery = row().gap(14.0).wrap();
    for (name, mode) in modes {
        gallery = gallery.child(
            column()
                .gap(6.0)
                .child(
                    img(SAMPLE)
                        .fit(mode)
                        .alt(format!("Sample picture, {name}"))
                        .w(px(140.0))
                        .h(px(96.0))
                        .rounded(px(8.0)),
                )
                .child(text(name).class("caption")),
        );
    }

    card()
        .child(text("Images").class("section-title"))
        .child(
            text("Decoded off the drawing thread and packed into an atlas of their own. The same file drawn four times is read once.")
                .class("muted"),
        )
        .child(gallery)
        .child(
            row()
                .gap(14.0)
                .items_center()
                .child(
                    img(SAMPLE)
                        .fit(ObjectFit::Cover)
                        .alt("Sample picture as a round avatar")
                        .size(px(64.0))
                        .rounded(px(32.0)),
                )
                .child(
                    text("Rounded in the shader, so an avatar costs one sprite and no mask.")
                        .class("muted"),
                ),
        )
}

fn surfaces_tab(_state: &State) -> Div {
    let swatches: [(&str, u32); 6] = [
        ("primary", 0x6c5ce7),
        ("accent", 0x00cec9),
        ("success", 0x3fb950),
        ("danger", 0xe05561),
        ("surface", 0x1e1e2e),
        ("border", 0x2c2c40),
    ];

    column()
        .gap(18.0)
        .child(images_card())
        .child(
            card()
                .child(text("Gradients and shadows").class("section-title"))
                .child(
                    row()
                        .gap(14.0)
                        .wrap()
                        .child(
                            div()
                                .w(px(180.0))
                                .h(px(90.0))
                                .rounded(12.0)
                                .bg(Paint::linear(
                                    135.0,
                                    vec![
                                        GradientStop::new(0.0, rgb(0x6c5ce7)),
                                        GradientStop::new(1.0, rgb(0x00cec9)),
                                    ],
                                ))
                                .shadow(BoxShadow::new(
                                    Point::new(px(0.0), px(8.0)),
                                    px(24.0),
                                    px(0.0),
                                    rgba(0x6c5ce766),
                                )),
                        )
                        .child(
                            div()
                                .w(px(180.0))
                                .h(px(90.0))
                                .rounded(12.0)
                                .bg(Paint::radial(vec![
                                    GradientStop::new(0.0, rgb(0xe05561)),
                                    GradientStop::new(1.0, rgb(0x11111b)),
                                ])),
                        )
                        .child(
                            div()
                                .w(px(180.0))
                                .h(px(90.0))
                                .rounded_tl(28.0)
                                .rounded_br(28.0)
                                .border(2.0)
                                .border_color(rgb(0x6c5ce7))
                                .bg(rgb(0x181825)),
                        ),
                ),
        )
        .child(
            card()
                .child(text("Palette").class("section-title"))
                .child(row().gap(12.0).wrap().children(swatches.iter().map(
                    |(name, color)| {
                        column()
                            .gap(6.0)
                            .items_center()
                            .child(div().class("swatch").bg(rgb(*color)))
                            .child(text(*name).class("muted small"))
                    },
                ))),
        )
        .child(
            card()
                .child(text("Corner radii animate too").class("section-title"))
                .child(text("Hover a swatch above.").class("muted small")),
        )
}

fn data_tab(_state: &State) -> Div {
    let rows: [(&str, &str, &str); 7] = [
        ("guirs-core", "geometry, color, units, transforms", "3.1k"),
        ("guirs-style", "stylesheet and cascade", "6.2k"),
        ("guirs-text", "shaping, layout, rich text", "3.5k"),
        ("guirs-render", "wgpu pipelines", "3.6k"),
        ("guirs-ui", "elements, widgets, windows", "9.7k"),
        ("kitchen-sink", "this demo", "1.0k"),
        ("total", "", "27.1k"),
    ];

    column()
        .gap(18.0)
        .child(
            card()
                .child(text("Crates").class("section-title"))
                .children(rows.iter().enumerate().map(|(index, (name, note, size))| {
                    list_item(false)
                        .child(text(*name).class("mono").w(px(150.0)).whitespace_nowrap())
                        .child(text(*note).class("muted small").flex_1())
                        .child(text(*size).class("mono small").whitespace_nowrap())
                        .when(index + 1 == rows.len(), |row| row.class("bright"))
                })),
        )
        .child(
            card()
                .child(text("Scrolling").class("section-title"))
                .child(
                    scroll_view()
                        .h(px(190.0))
                        .gap(4.0)
                        .children((1..=40).map(|n| {
                            list_item(false)
                                .child(
                                    text(format!("{n:>3}"))
                                        .class("mono small muted")
                                        .whitespace_nowrap(),
                                )
                                .child(text(format!("Row number {n}")).whitespace_nowrap())
                        })),
                )
                .child(text("The list above has its own scrollbar.").class("muted small")),
        )
        .child(
            card()
                .child(text("Two hundred thousand rows").class("section-title"))
                .child(
                    scroll_view()
                        .h(px(190.0))
                        .virtual_rows(200_000, px(26.0), |index| {
                            list_item(false)
                                .child(
                                    text(format!("{:>7}", index + 1))
                                        .class("mono small muted")
                                        .whitespace_nowrap(),
                                )
                                .child(
                                    text(format!("Row number {}", index + 1))
                                        .whitespace_nowrap(),
                                )
                                .into_any()
                        }),
                )
                .child(
                    text(
                        "Only the rows on screen exist, plus a few either side. The \
                         scrollbar describes all two hundred thousand because the \
                         extent comes from the count rather than from measuring them.",
                    )
                    .class("muted small"),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs::{Cx, ScaleFactor, Size, StyleEngine, TextSystem};

    const THEME: &str = include_str!("../assets/theme.gss");

    /// Build, lay out and paint one element tree, the way a real frame does.
    ///
    /// No GPU is involved: the scene is built and thrown away. What this
    /// catches is a section that panics, which is otherwise only discovered by
    /// clicking to it.
    fn draw(mut root: AnyElement) -> Cx {
        let size = Size::new(px(1180.0), px(760.0));
        let mut cx = Cx::new(TextSystem::new(), StyleEngine::from_source(THEME));
        cx.begin_frame(size, ScaleFactor(1.0), 0.0);
        let node = root.request_layout(&mut cx);
        cx.layout.compute(node, size, &mut cx.text);
        root.paint(cx.layout.bounds(node), &mut cx);
        cx.end_frame();
        cx
    }

    #[test]
    fn every_section_lays_out() {
        let state = State::new();
        for (index, name) in NAV.iter().enumerate() {
            state.nav.set(index);
            let cx = draw(content(&state).into_any());
            assert!(
                cx.scene.stats().quads > 0,
                "{} drew nothing at all",
                name
            );
        }
    }

    #[test]
    fn every_tab_lays_out() {
        let state = State::new();
        state.nav.set(1);
        for index in 0..TABS.len() {
            state.tab.set(index);
            draw(content(&state).into_any());
        }
    }

    #[test]
    fn the_whole_window_lays_out() {
        let state = State::new();
        draw(
            div()
                .class("root")
                .size_full()
                .col()
                .child(titlebar(&state, 60.0, 1.0, "build 0.4  layout 0.3", &Keymap::new()))
                .child(row().flex_1().child(sidebar(&state, 60.0)).child(content(&state)))
                .into_any(),
        );
    }

    #[test]
    fn the_second_window_lays_out_and_counts() {
        let clicks = Model::new(0i32);
        draw(second_window(&clicks).into_any());
        // The state belongs to the caller, so the window can be rebuilt without
        // losing it. Building it twice must not reset the count.
        clicks.set(7);
        draw(second_window(&clicks).into_any());
        assert_eq!(clicks.get(), 7);
    }

    #[test]
    fn the_virtualized_list_builds_only_a_window_of_rows() {
        let state = State::new();
        state.nav.set(1);
        state.tab.set(2);
        let cx = draw(content(&state).into_any());
        // Two hundred thousand rows, and the frame is a few hundred boxes.
        // Two hundred thousand rows, and the frame is a few hundred boxes.
        // Without virtualization this assertion would not so much fail as
        // never finish, which is the point of it.
        assert!(
            cx.layout.node_count() < 1000,
            "laid out {} nodes for a screenful",
            cx.layout.node_count()
        );
    }
}
