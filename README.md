# guirs

An extensible, GPU accelerated GUI framework for Rust. No HTML, no JavaScript,
no web view: elements are Rust values, layout is flexbox and grid, and
everything is drawn by two wgpu pipelines.

Menus, trees, tables, split panes, drag and drop, keymaps with chords, images,
and a screen reader that sees a real tree, all of it in Rust.

The styling system is a real cascading stylesheet language with its own parser,
so an application can be re-themed by editing a text file while it is running.

```
cargo run --release -p chat            # a chat application
cargo run --release -p kitchen-sink    # every widget and feature, as a gallery
```

Then edit the `.gss` file next to either one while the window is open. Both are
watched and reloaded, so a color, a radius or a transition changes without a
rebuild.

## The chat application

`apps/chat` is a real application rather than a demonstration: a conversation
list, a transcript that stays pinned to the end as replies arrive, selectable
message text, and a composer that sends on Enter. Replies are canned, because
the interesting part is the interface, but they arrive a piece at a time the way
a real one would.

It exercises the parts of the framework that only show up in something you
actually use: selection and clipboard, a scroll view that behaves when content
is appended, transitions that survive a rebuilt tree, text that wraps and
reflows as the window resizes, replies with bold, code and links in a single
paragraph, files dropped onto the window, a native file picker, and a second
window that is a peer rather than a child.

## What it looks like

```rust
use guirs::prelude::*;

fn main() -> Result<(), AppError> {
    let count = Model::new(0i32);

    App::new()
        .title("Counter")
        .stylesheet_file("assets/theme.gss")
        .run(move |_cx| {
            let increment = count.clone();
            div()
                .class("root")
                .size_full()
                .col()
                .center()
                .gap(12.0)
                .child(text(format!("{}", count.get())).class("display"))
                .child(
                    button("Increment")
                        .class("primary")
                        .on_click(move |_, _| increment.update(|n| *n += 1)),
                )
                .into_any()
        })
}
```

The element tree is rebuilt every frame from state held in `Model`s. Nothing in
the tree outlives a frame, so there is no way for the screen to disagree with
the model.

## Architecture

Five layers, each depending only on the ones beneath it, under a facade. A
renderer or a text stack can be replaced without touching anything above it.

| Crate | Responsibility |
| --- | --- |
| `guirs-core` | Units, geometry, color, paint. Almost no dependencies. |
| `guirs-style` | The `.gss` language, selector matching, the cascade, transitions. |
| `guirs-text` | Font discovery, shaping, glyph rasterization, line breaking, hit testing. |
| `guirs-render` | Scene building, atlas packing, the wgpu renderer and its two shaders. |
| `guirs-ui` | Element tree, layout, input dispatch, widgets, the application runtime. |
| `guirs` | A facade that re-exports the lot, plus a prelude. |

One crate sits outside that stack. `guirs-build` is a build dependency rather
than a dependency: nothing links against it, and it exists because an
executable's icon is decided when the linker runs. See
[Shipping an application](#shipping-an-application).

`apps/chat` and `examples/kitchen-sink` sit on top of the facade and use
nothing private.

### Rendering

Everything on screen is one of two shapes: a rounded box or a textured sprite.

The box shader evaluates a signed distance field, so borders, per corner radii,
linear and radial gradients, and Gaussian box shadows are all computed
analytically in the fragment shader. There is no tessellation and no CPU path
work, antialiasing is exact at any scale factor, and thousands of boxes cost one
instanced draw call.

Sprites cover glyphs and images from two array atlases, one single channel for
antialiased glyphs and one RGBA for color emoji and images.

Primitives are recorded in painting order and merged into batches whenever the
kind and clip match, so a full interface collapses to a few dozen draw calls.

Colors are authored, interpolated and blended in sRGB, which is what a
stylesheet author expects and what every other interface toolkit does.

### Text

Shaping runs through `swash`, which applies the font's own substitution and
positioning tables. That is what produces ligatures, mark attachment and the
contextual forms complex scripts need. Bidi analysis assigns embedding levels,
runs are split by level, script and font, and font fallback kicks in per
character so a missing glyph pulls in the first fallback face that covers it.

Glyphs are rasterized at device resolution at one of four horizontal subpixel
phases and cached in an atlas. Laid out blocks are cached too, keyed by text,
style and constraint, which is what keeps a screen full of text nearly free
after the first frame.

### Rich text

A paragraph is rarely uniform. Part of it is bold, a word is a piece of code, a
phrase is a link. A `RichText` is one string plus a sorted list of spans over
it, and a span only describes what differs, so a span saying "bold" stays in
step with a stylesheet that later changes the font:

```rust
rich_text(
    rich("Rust is ")
        .bold("fast")
        .push(" and ")
        .code("mem::forget")
        .push(", see ")
        .link("the book", "https://doc.rust-lang.org/book/"),
)
.on_link(|href, _| open_in_browser(href))
```

This is one laid out paragraph, not several elements pushed side by side. That
distinction is the whole point: the line breaker measures each piece with its
own font, so a bold word wraps where a bold word should; a line takes its height
and its baseline from the tallest face actually on it, so a larger span pushes
the whole line down rather than overprinting its neighbours; and the spans wrap
as one block instead of as separate boxes that flexbox happens to place in a
row.

Spans carry color, background and decoration as well as font selection, and an
optional link target. The framework reports that a link was clicked and stops
there, because what a link means is the application's decision.

Plain text is the same path with an empty span table, sharing one empty table
across every label in the tree, so a plain string costs a reference count rather
than an allocation and none of the above is paid for until a span exists.

Deciding that two asterisks mean bold is a decision about a document format
rather than about text, so markdown lives in the chat application rather than in
the framework. It is under two hundred lines, in `apps/chat/src/markdown.rs`.

### Transforms

`translate`, `rotate` and `scale` are style properties, and they animate:

```css
.icon-button          { transition: transform 90ms ease-out; }
.icon-button:active   { transform: scale(0.90); }
```

They apply at paint time. Layout has already run by then, so a transform costs
no measuring and moves nothing around it, which is what makes one cheap enough
to run every frame on a press.

The transform is kept decomposed rather than multiplied into a matrix, because
that is the form that interpolates the way a reader expects: halfway between no
rotation and a half turn should be a quarter turn, where the midpoint of the two
matrices is a shape squashed flat. As a consequence the functions commute, which
differs from CSS and is the deliberate trade.

A transformed element is clickable where it appears rather than where it was
laid out: the pointer is mapped back through the inverse before anything is
hit tested. An element scaled to nothing cannot be clicked, because there is
nothing there to click.

### Rounded clipping

A scissor rectangle cannot describe a corner, so a scrolling list inside a
rounded card used to spill its content into the curve. The straight edges are
still the scissor's job, which is most of the work; the corners are evaluated in
the fragment shader from the same signed distance field the shapes use, so they
get the same antialiasing.

The clip a fragment belongs to is an index into a small per frame table rather
than a copy on every instance, so neither instance layout grew for it. Rounded
clips come from elements that have both a radius and children to clip, which is
a handful in any real interface; past sixteen in one frame the rest keep their
scissor and lose their corners.

Nesting two rounded clips keeps the inner one's corners and leaves the outer's
to the scissor. The intersection of two rounded rectangles is not a rounded
rectangle, and the inner one is the one a reader notices.

### Virtualized lists

A scrolling container can build its rows on demand:

```rust
scroll_view().virtual_rows(200_000, px(28.0), move |index| {
    row(index).into_any()
})
```

Only the rows on screen exist, plus a few either side. A hundred thousand rows
lay out a few hundred boxes rather than a few hundred thousand, and the number
is a function of the viewport rather than of the count.

When the rows are not all the same height, `virtual_rows_measured` measures
each one as it builds it and remembers the answer, so nothing has to work out
in advance how tall a wrapped, styled, marked up block of text is:

```rust
scroll_view().virtual_rows_measured(messages.len(), px(150.0), move |index| {
    message(index).into_any()
})
```

Rows nobody has scrolled to are worth the estimate until they are reached,
which makes the scrollbar approximate on a list that has never been read
through. That is the price of not laying out what nobody is looking at. The
rows themselves are always exact, and correcting a guess above the window moves
the scroll offset with it so the text does not slide under the reader. The chat
application's transcript is built this way: a hundred messages cost the same
frame as ten.

`virtual_rows_variable` is the same thing for a caller that already knows the
offsets and would rather supply them than have them discovered.

Which rows are visible is answered from the previous frame's scroll offset,
because a frame decides what to build before it knows how big it is. The
overscan margin covers the difference during a fast scroll.

Rows are keyed by index rather than by position among the built children. A
row's slot changes every time the list scrolls, and without a stable key the
hover, the transitions and every other piece of retained state would stay with
the slot while the rows slid underneath.

Every row has to be the same height, because the scroll extent is computed from
the count rather than measured.

## More than one window

A window is opened from anywhere with a handle on the context:

```rust
cx.open_window(WindowOptions::new("Preferences").sized(420.0, 520.0), |cx| {
    preferences(cx).into_any()
})
```

The new window is a peer rather than a child: it has its own element state,
scroll positions and focus, it shares the stylesheet and the font database, and
closing the window that opened it leaves it open. The application exits when the
last window goes, not when the first does.

### Dialogs

File and message dialogs are the platform's own rather than something drawn
here. A file picker is where a person expects their sidebar, their recent
places, their network drives and their search to be, and no framework reproduces
those convincingly:

```rust
if let Some(paths) = FileDialog::new()
    .title("Attach files")
    .filter("Images", &["png", "jpg"])
    .open_files()
{
    attach(&paths);
}
```

Each call blocks until the person answers, which is what modal means.

### Files from the desktop

Dropping files onto an element fires one event carrying all of them, because a
drop of four files is one gesture rather than four:

```rust
div().on_file_drop(|event, cx| attach(&event.paths))
     .on_file_hover(|event, cx| highlight(!event.paths.is_empty()))
```

`on_file_hover` fires while a drag is over the element and again with an empty
list when it leaves, so a drop target can light up and go dark again.

## Typing

### Fields of more than one line

`text_area` wraps at its own width, grows downwards as lines are added, and
takes Enter as a newline. `text_input` stays one line and leaves Enter for the
application, which is what a search box or a chat composer wants.

```rust
text_area(notes.clone(), "Notes\u{2026}").w_full().max_h(px(160.0))
```

Moving between lines is the window's job rather than the field's, for the same
reason placing a caret with the pointer is: it is a question about laid out
text, and no element has the layout while an event is being handled. Up and down
remember the column they set out from, so passing through a short line and
coming out the other side returns to where it started rather than to the end of
the short line. Home and End work on the line the caret is on once there is more
than one.

A single line field leaves the arrows alone. It has nowhere to move to, and an
application is entitled to use them for something else, a history of what was
typed being the usual thing. Pasting behaves differently too: an area keeps the
newlines, a single line field flattens them, because a newline in a value that
never renders is a character the caret cannot get past.

### The caret

It blinks, and everything about it comes from the stylesheet:

```gss
:root {
    --caret:         #ededf0;
    --caret-blink:   530ms;   /* `none` for a caret that stays lit */
    --caret-width:   1.5px;
    --caret-radius:  1px;
}
```

The phase runs from the last edit rather than from an absolute clock, so the
caret is solid the instant a key is pressed and only starts blinking once
someone stops typing. A caret that happened to blink out exactly as a letter
appeared would read as a dropped keystroke.

Blinking does not cost frames. Rather than animating, the field asks for one
frame at the moment the caret next changes, and the runtime sleeps until then:
a window sitting with a focused field draws about two frames a second, not
sixty. Anything else that changes on its own but rarely can do the same with
`cx.request_redraw_in(seconds)`.

### Input methods

Typing Japanese, Chinese or Korean does not produce a character per keystroke.
An input method composes a proposal, shows it in place, and only commits it once
the person has chosen. Accented Latin works the same way on several platforms:
pressing the accent key and then the letter is one composition, not two
characters.

Composing text is held apart from the field's value and spliced in only for
display. That distinction is the whole of it: an application reading `value`
mid-composition sees what has actually been entered rather than the guesses, and
the caret, the selection and everything the pointer does stay in the value's own
coordinates.

```rust
state.set_preedit("\u{304b}", Some((3, 3)));
state.value          // unchanged
state.display_text() // has the composing text in it, underlined when drawn
state.commit("\u{79c1}");
state.value          // now it is real
```

While a composition is open the arrows and backspace belong to the input method,
which reports the result as a new proposal, and the field leaves them alone. A
click elsewhere abandons the composition, as a native field does.

A focused field publishes where its caret is, and the runtime turns the input
method on and tells the platform where to put the candidate window. Without
that last part the candidate list opens in a corner of the screen, which is
unusable for the languages that need one.

## Design tokens

A `:root` custom property is how the parts of the framework that draw
themselves are themed, because they have no element for a selector to reach:

| Token | What it colours |
| --- | --- |
| `--background`, `--window-background` | the surface behind everything |
| `--selection` | selected text, in fields and in prose |
| `--caret`, `--caret-blink`, `--caret-width`, `--caret-radius` | the caret |
| `--scrollbar-thumb`, `--scrollbar-track` | overlay scrollbars |

Everything else is an ordinary element with an element type name, so it is
reached by a selector rather than a token: `input`, `textarea`, `button`,
`select`, `scrollbar` and the rest all take the full property set.

## Work off the drawing thread

A frame has about sixteen milliseconds. Anything that might take longer, reading
a file, talking to a network, searching an index, has to happen elsewhere or the
window stops responding while it runs.

```rust
// In a click handler.
app.search = Some(spawn(move || read_the_index(&query)));

// In the root, next frame and every frame after.
if let Some(results) = app.search.as_ref().and_then(Task::take) {
    app.results = results;
}
```

Finishing wakes the event loop, so the frame that collects the result happens on
its own. Nothing polls on a timer: a window with a task outstanding sleeps
exactly as deeply as one without.

Polling rather than a callback, because a callback would arrive on a worker
thread and everything an interface owns, the element state, the `Model`s, the
text caches, belongs to the thread that draws. Both the closure and its result
are `Send`, which rules out passing a `Model` in. That is the point rather than
a limitation: a task returns a value for the drawing thread to apply, instead of
reaching across and applying it.

The pool is small and built on first use, so an application that never spawns
anything pays for nothing. Cancellation is cooperative, because a thread cannot
be safely killed from outside while it holds a lock or a file; work that can be
interrupted checks `is_cancelled` as it goes, and work that cannot runs to the
end and has its result dropped.

## The stylesheet language

`.gss` is deliberately a fixed grammar rather than a general CSS engine, but
what is there behaves the way CSS does.

```gss
:root {
    --primary: #6c5ce7;
    --radius: 8px;
    --speed: 140ms;
}

button {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 15px;
    transition: background var(--speed) ease-out;
}

button:hover        { background: lighten(var(--surface), 22%); }
button.primary      { background: linear-gradient(180deg, #8071ec, #6c5ce7); }
select:open         { border-color: var(--primary); }
.sidebar > button   { width: 100%; }
input:not(.plain)   { box-shadow: 0 0 0 3px rgba(108, 92, 231, 0.3); }
```

**Selectors.** Type, class, id, universal, descendant and child combinators,
selector lists, and `:not()`. Specificity and source order resolve conflicts
exactly as CSS does.

**Pseudo classes.** `:hover`, `:active`, `:focus`, `:focus-within`,
`:focus-visible`, `:disabled`, `:enabled`, `:checked`, `:selected`, `:open`,
`:root`, `:first-child`, `:last-child`, `:only-child`, `:empty`.

**Properties.** Layout (`display`, `position`, `inset`, sizes, `margin`,
`padding`, `flex`, `gap`, `overflow`), paint (`background`, per side borders,
per corner `border-radius`, `box-shadow`, `opacity`, `cursor`), text (`color`,
`font-family`, `font-size`, `font-weight`, `font-style`, `line-height`,
`letter-spacing`, `text-align`, `white-space`, `text-decoration`,
`text-overflow`), `transform` and `transform-origin`, and `transition`.

**Values.** `px`, `rem`, `%`, `fr`, `deg`, `ms`, `s`. Hex colors in all four
lengths, CSS color keywords, `rgb()`, `rgba()`, `hsl()`, `hsla()`, and the
derivation helpers `lighten()`, `darken()`, `alpha()` and `mix()`.
`linear-gradient()` and `radial-gradient()` with any number of stops.
`cubic-bezier()` and `steps()` easing.

**Custom properties.** `var(--name, fallback)`, resolved at parse time so the
cascade never touches a token again. Nested references work; cycles are caught
and reported.

Parsing is error tolerant. A malformed declaration is reported with its line
number and skipped, and everything around it still applies, which matters when
the file is being edited live.

The same properties are available as a typed builder API, so
`Style::new().bg(color).rounded(8.0)` and the stylesheet land on the same struct.

## Widgets

`button`, `icon_button`, `text_button`, `checkbox`, `radio`, `toggle`, `slider`,
`progress`, `select`, `tab_bar`, `list_item`, `text_input`, `scroll_view`,
`text_area`, `panel`, `card`, `row`, `column`, `stack`, `spacer`, `separator`.

Almost every one is a `Div` with a distinct element type name, so the whole set
is styleable from a stylesheet with no special cases, and every widget composes
with the entire styling API rather than a curated subset.

Widgets are controlled: they render the value they are handed and report changes
through a callback. Nothing in the widget set owns application state.

The text field is a real editing model. Byte offsets always land on character
boundaries, so arrow keys step by character rather than by byte, selection
extends with shift, and `Home`, `End` and the platform's select-all shortcut all
behave.

### Icons

Icons are SVG path data, filled or stroked into a coverage mask at device
resolution and cached in the same atlas as glyphs. An icon therefore costs no
more to draw than a letter, stays exact at any size, and takes its size from
`font-size` and its color from `color` like text does.

```rust
icon(icons::search())                       // 26 built in, on a 24 unit grid
icon(Icon::stroked(my_path, 24, 2.0))       // or any path data
```

```gss
icon         { width: 16px; height: 16px; }
button icon  { color: var(--text-soft); }
```

That is all of SVG an icon set actually uses, which is why it fits in a
hundred lines rather than pulling in a rendering library.

### Window chrome

`App::undecorated()` removes the platform title bar. What replaces it is drawn
by the application, so it can look like the rest of it:

```rust
title_bar()                     // drag to move, double click to maximize
window_controls(maximized)      // minimize, maximize or restore, close
resize_borders()                // grab strips along the edges and corners
window_frame(title, maximized, content)   // all three, wired together
```

The gestures are handed to the compositor rather than reimplemented. Dragging
and resizing a window is the compositor's job, and doing it by hand is what
gives custom title bars their reputation for lag and for failing to snap.

Corners are rounded by asking the compositor too, so the window keeps its drop
shadow and matches every other window on the desktop. `App::square_corners()`
turns it off.

### Native window controls

A window that draws its own title bar has to draw its own controls, and drawn
controls usually give themselves away. Two things close most of that gap.

The glyphs are the system's. Windows ships the font it draws its own title bars
with, so on Windows the controls render text from it rather than an
approximation of it: the real shapes at the real weight, over the application's
own background. A minimum size guards the case where that font is missing, so a
machine without it still gets a window it can close.

The behaviour is the system's too. Windows 11 offers a layout picker when the
pointer rests on a maximize button, and decides whether it is on one by sending
the window `WM_NCHITTEST` and looking for `HTMAXBUTTON` in the reply. Marking an
element with `snap_target()` is how it gets the right answer:

```rust
window_button(icons::maximize()).snap_target()
```

That is all an application says. The window procedure is subclassed underneath,
and the element's own painted bounds are what gets claimed, so moving the
control moves what the platform asks about. Everywhere else `snap_target()` does
nothing.

Claiming a rectangle has a consequence worth knowing: Windows then treats it as
part of the frame, so ordinary mouse messages stop arriving over it. The button
would go dead and stop lighting up. The non client messages that replace them
are fed back into the interface as an ordinary move and click, which is why it
still behaves like the rest of the window.

### Selecting text

Any text can opt into selection:

```rust
text(message).selectable()
```

Press and sweep to select, double click for a word, triple click for a line, and
the platform copy shortcut puts it on the clipboard. The highlight is drawn per
line from the shaped glyphs rather than from two caret positions, so it stays
correct when a line wraps or mixes directions.

Selection is off by default because most text in an interface is a label, and
making labels selectable turns every stray drag into a highlight.

A sweep that leaves one run carries on into the next. Runs are numbered as they
are painted, which is reading order, and that ordering is what makes a
selection spanning several of them meaningful: the run it starts in gives up
its beginning, the one it ends in gives up its end, and everything between is
taken whole. Copying them puts each on its own line, because separate runs are
separate blocks of text rather than one sentence.

Text fields get the same treatment without asking: click to place the caret,
sweep to select, double click for a word, and cut, copy, paste and select all
on the platform shortcuts. The window owns that rather than the widget, because
it needs the focused field and the system clipboard at once.

## Commands, keys and menus

Three things that are usually written down three times, written down once.

A **command** is a name rather than a call. `"file:save"` is a string, and
whoever knows how to save answers it:

```rust
div().key_context("editor").on_command("file:save", |cx| { /* save */ })
```

A **keymap** says which keys ask for which command, and where:

```rust
App::new().keymap(
    Keymap::new()
        .bind("cmd-s", "file:save")
        .bind("cmd-k cmd-d", "editor:duplicate-line")
        .bind_in("editor", "cmd-f", "editor:find")
        .bind_in("tree", "cmd-f", "tree:filter"),
)
```

A **menu** names the same commands, and reads its own shortcuts back out of
the keymap:

```rust
menu_bar(&[
    submenu("File", [
        menu_item("New").command("file:new"),
        menu_separator(),
        menu_item("Save").command("file:save"),
        submenu("Open Recent", recent_files),
    ]),
], state.menus.clone(), &cx.keymap)
```

Nothing is repeated, so nothing can drift: rebinding a key changes what the
menu shows, and an entry and its shortcut reach the same handler because they
are the same command.

**Where a binding applies.** A context is a name an element claims while it or
anything inside it has focus. `Ctrl+F` above means one thing in an editor and
another in a file tree, and the innermost context that has something to say
wins. A binding with no context applies everywhere and is reached only when
nothing nearer claims the key.

**Sequences.** `"cmd-k cmd-d"` is two presses. After the first the keymap holds
still and waits. A binding that is a prefix of a longer one waits rather than
firing, because running it immediately would make the longer one unreachable.

**What a command does not do.** A binding whose command nothing answers is not
consumed. A keymap is allowed to name commands a particular window does not do,
and swallowing the key would stop somebody typing a letter that happens to be
bound elsewhere.

A command travels outwards from whatever has focus until something answers it,
the way a click travels outwards from whatever was under the pointer. If
nothing along that path answers, and when nothing is focused at all, it is
offered to anything else that handles commands. That last part is what makes a
menu work: nothing is focused while one is open.

**Menus** nest as deep as they like, and entries can be disabled, checked, or
separators. `context_menu` is the same list placed where the pointer is. An
entry announces its own name and offers its shortcut as a shortcut rather than
as part of the name, so a reader says "New" and offers Ctrl+N, rather than
reading out "New Ctrl plus N" as though that were what it was called.

## Trees

```rust
tree(&roots, state, |key| open_file(key))
```

A file explorer, an outline, a scene graph. What makes a tree different from a
list is that what is on screen is not what is in the data: only the open parts
are visible, and moving down one row can mean stepping into a branch or out of
two.

That flattening is the whole difficulty, so it is [`flatten`] on its own, a
function of the nodes and what is open. It can be checked without drawing
anything, and an application with a very large tree can call it and lay the
rows out through a virtualized list rather than using `tree` directly.

Nodes are identified by a key rather than by position, because open and
selected have to survive the tree being rebuilt: a path does, an index does
not.

The arrow keys walk it the way a file explorer does. Right opens a closed
branch **without** moving, and only steps into it once it is open, so holding
it walks down a path rather than skipping past what was just revealed. Left
closes an open branch, and on something already closed steps out to whatever
contains it, which is the nearest row above that is one level shallower rather
than simply the row above.

## Grid

```rust
div().grid()
     .grid_columns([TrackSize::Px(px(200.0)), TrackSize::Fr(1.0), TrackSize::Fr(2.0)])
     .child(header().col_span(3))
     .child(sidebar())
```

or from the stylesheet:

```css
.dashboard {
    display: grid;
    grid-template-columns: 200px repeat(2, 1fr);
    gap: 12px;
}
.dashboard .banner { grid-column: 1 / -1; }
```

Tracks can be a fixed size, a percentage, a share of what is left (`1fr`), or
sized to their contents (`auto`, `min-content`, `max-content`). `repeat(n, ...)`
is expanded when the stylesheet is read rather than passed along, so
`repeat(3, 1fr)` works and so does `repeat(2, 100px 1fr)`.

Items are placed automatically, or put where they are told with `grid-column`
and `grid-row`: a line number, a range like `2 / 5`, a `span 3`, or `-1` to
count back from the end.

## Tooltips

```rust
div().tooltip("Delete this file")
```

Appears after the pointer has rested, so moving across a row of controls does
not flash one on each; goes away as soon as the pointer moves on or anything is
pressed. It is a real element in an overlay rather than something drawn by
hand, so it is styled from the stylesheet like everything else and escapes
whatever its own element is clipped to.

The same text becomes the element's accessible description. Somebody using a
screen reader never rests a pointer on anything, so a tooltip that only ever
appeared on hover would say nothing to them at all.

## Tables

```rust
table(&columns, rows.len(), state, |row, col| {
    div().child(text(cell_text(row, col.key())))
})
```

The table does not hold the data and does not sort it. It reports which column
somebody asked to sort by and in which direction, and the application sorts its
own rows. Anything else means copying the data in, or teaching the table how to
compare values it has never seen. Cells are asked for one at a time and can
hold anything, so a cell is a widget rather than a string.

Clicking a heading sorts ascending, then descending, then not at all. The third
press is the one that matters: without it there is no way back to whatever
order the data was in to begin with.

Dragging a heading's trailing edge resizes the column, and a column cannot be
dragged shut, because the handle that would widen it again goes with it.

One thing worth knowing if you build something similar: stopping a press from
propagating does **not** stop the click that follows it. The heading has to
recognise a press on its own trailing edge as a resize, or every column dragged
wider is also sorted.

## Dragging inside the window

A different thing from files arriving from the desktop: nothing leaves the
process, so what travels is a value rather than a path.

```rust
div().draggable("row", index)
     .on_drop("row", move |dropped, cx| {
         if let Some(from) = dropped.value::<usize>() {
             reorder(*from, index);
             cx.request_redraw();
         }
     })
```

A drag is named, and a target says which name it accepts. A tab dropped on a
file tree does nothing rather than something surprising, and neither side has
to know what the other is. The value is carried as itself, so a target that
recognises the name asks for the type it expects and is told if it is wrong.

A press is not a drag until the pointer has moved far enough to mean it, so
anything draggable can still be clicked. Letting go over nothing that accepts
what is being carried drops nothing, which is how a drag is cancelled.

While it is happening, the thing being carried matches `:dragging` and the
place it would land matches `:drag-over`, so both can be shown without an
application tracking either:

```css
.row:dragging  { border-style: dashed; }
.row:drag-over { border-color: var(--primary); }
```

## Split panes

```rust
split_x(sidebar_width, file_tree(), split_y(panel_height, editor(), terminal()))
```

Two panes and a divider you can drag. Nest them and you have the layout an
editor or a set of developer tools is made of, every boundary movable.

The first pane is measured in pixels and the second takes what is left. That is
deliberate: a sidebar should stay the width somebody dragged it to when the
window is resized, rather than growing in proportion and having to be dragged
back. `SplitState::new(240.0).range(150.0, 500.0)` gives it a size and limits,
and a pane without limits can be dragged shut by accident.

The divider is wider to the pointer than it looks, because a hairline is what
it should look like and is close to impossible to grab. It reports itself as a
splitter, so a screen reader finds it and says what it is for.

Three things a splitter has to say out loud, all of which are flexbox defaults
working against it: the first pane must not shrink, or content in the second
one squeezes it and the divider stops short of where it was dragged; the
divider must not shrink, or it is the first thing squeezed and quietly
vanishes; and both panes must be allowed to shrink below their content, or the
divider stops early and looks stuck. All three are set here so an application
does not have to know about any of them.

## Shipping an application

Two things separate a program that runs from one that looks like it belongs on
the desktop, and neither is about drawing.

**It should be a window, not a command.** On Windows a program is linked as
either a console application or a windowed one, and the default is a console
application, which opens a black rectangle behind the window. No library can
settle this, because it is an attribute the compiler reads at the top of the
crate root. One line, above the first item in `main.rs`:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

A released build is then a window, and a debug build keeps its console, which
is somewhere for a `println!` to land while developing. It does nothing on any
other platform, so it is safe to leave in.

**It should have an icon.** There are two, they are different, and an
application usually wants both pointing at the same file.

The window's own icon is shown in its title bar, when switching windows, and
on its taskbar button while it is running:

```rust
App::new().icon(include_bytes!("../assets/icon.png"))
```

The executable's icon is shown in Explorer, on a pinned shortcut, and in the
task manager. That one lives in the binary's resource section and is put there
by the linker, so nothing a running program does can change it. It needs a
build script:

```toml
[build-dependencies]
guirs-build = { path = "crates/guirs-build" }
```

```rust
// build.rs
fn main() {
    guirs_build::icon("assets/icon.png").unwrap();
}
```

Hand it one square PNG. It is resized to every size Windows asks for, from 16
up to 256, and written into the executable, so there is no need to prepare an
`.ico`. The resource file is generated directly rather than by running the
Windows SDK's `rc.exe`, so nothing beyond a Rust toolchain has to be installed.
On other platforms the call does nothing and can be left in.

## Images

```rust
img("assets/photo.jpg").w(px(320.0)).h(px(180.0)).fit(ObjectFit::Cover)
img(include_bytes!("../assets/logo.png")).h(px(24.0))
```

PNG and JPEG, from a path or from bytes already in the binary. A source is
whatever converts into one, so a `&str`, a `PathBuf` or an `include_bytes!`
all work without ceremony.

Reading and decoding happen off the drawing thread, because decoding a
photograph takes tens of milliseconds and a frame's whole budget is sixteen.
A picture that has not arrived occupies its space and draws nothing; the frame
it becomes ready is the frame the window is asked to draw again. Nothing
blocks, and a missing or corrupt file leaves a gap rather than stopping the
interface.

The store belongs to the window, so the same file drawn in ten places is read
once, decoded once, and held once.

**Fitting.** A picture and the box it is given are rarely the same shape.

| | |
|---|---|
| `Contain` | the whole picture inside the box, space along one axis. The default, because it is the only one that neither crops nor distorts. |
| `Cover` | the box filled, the picture's shape kept, the overflow cropped. What a thumbnail or an avatar wants. |
| `Fill` | the box filled by changing the picture's shape. The only one that distorts. |
| `None` | the picture's own pixels, centered, cropped if they do not fit. |

Cropping is done with texture coordinates rather than by drawing a smaller
rectangle, so `Cover` costs exactly what the others cost.

**Sizing.** A picture with no size takes its own. A picture given one dimension
takes its shape from the other. A picture given both keeps what it was given,
whatever shape it is.

**Corners.** `rounded()` applies to the picture itself, evaluated in the
fragment shader like every other rounded box here, so a circular avatar is one
sprite and no mask.

**Where they live.** Pictures get their own texture atlas rather than sharing
the one color glyphs use, because one page size cannot suit both: a page large
enough for a photograph wastes most of itself holding emoji, and a page sized
for emoji cannot hold a photograph at all. Pages are 1024 square and are
created when the first picture is drawn, so an application showing none pays
nothing. Anything larger than a page is scaled down to fit rather than refused,
which is usually invisible: a photograph from a camera is several times the
size of any box it will be drawn into.

**Running out of room.** The packer only ever appends, so the space one
picture occupies cannot be handed back on its own: reclaiming any of it means
reclaiming all of it. When the atlas fills, everything in it is dropped and
whatever is still on screen is decoded again over the next frame or two, which
already happens off the drawing thread.

That only happens if something in the atlas has gone unwanted. An atlas full of
pictures somebody is still looking at is not a cache in need of clearing, and
emptying it would free nothing that stays free: every one of them would be
decoded again to draw the very next frame. So a window whose pictures are all
in use is told there is no room, rather than being put into a loop.

**Describing them.** A picture says what it shows, or says that it is
decoration and stays out of the way entirely:

```rust
img(source).alt("Nine people standing on a beach")
img(texture).decorative()
```

## Accessibility

An interface describes itself to whatever is reading it. On Windows that is UI
Automation, so Narrator, Windows Speech Recognition and any automation client
see a real tree: named controls, correct bounds, and actions that do what they
say. AccessKit carries it to the platform.

Nothing is built until something attaches. A run with no screen reader present
pushes no nodes and sends no updates, so the cost is a branch per element.
When a client attaches, the tree is built from the frame that was just painted,
which means it describes what is actually on screen rather than a parallel
model that can drift from it.

Most of it needs no work from an application. Painting is what produces the
description, so an element's position, its role and its state are already
known:

```rust
// Button, named "Send", offers Click.
button("Send")
// CheckBox, named "Telemetry", carries whether it is ticked.
checkbox("Telemetry", on, |v| ..)
// Slider, carries where it sits between its ends.
slider(volume, |v| ..)
// Named by its placeholder, valued by what has been typed into it.
text_input(state, "Ask anything")
```

Two rules cover almost everything an application still has to say.

**Name what has no words in it.** A control takes its name from the text
inside it, which is why `button("Send")` needs nothing. An icon only control
has no text to take, so it says its own name:

```rust
div()
    .class("icon-button")
    .child(icon(icons::compose()))
    .label("New chat")
    .on_click(..)
```

**Hide what is a picture rather than words.** A tick inside a checkbox, a
chevron on a menu, a glyph from an icon font: announced, these give a person a
character they cannot act on, and worse, they become part of the name of
whatever contains them. `decorative()` leaves an element and everything under
it out of the description without affecting what is drawn:

```rust
text(glyph).decorative()
```

Anything with a click handler is reachable, whatever it is called. A plain
`div()` with an `on_click` is described as a button, because that is what it
is to somebody who cannot see it, and because a platform drops anonymous
containers from the tree it builds. A `div()` with no handler stays scenery.

`role()`, `access_checked()`, `access_value()` and `access_range()` override
the derived answers where a widget is doing something the element type does not
imply.

What a reader asks for goes through the same handler a pointer would have run,
so an activation from Narrator and a click are the same event as far as an
application is concerned. Focus is declined for anything outside the focus
order, rather than stranding it somewhere the keyboard cannot leave.

The model is in `access.rs` and the platform side in `access_bridge.rs`, kept
apart so the tree can be built and tested with no screen reader anywhere near
it. That is what the tests do, through the same paint path a window uses.

## Performance

The kitchen sink demo runs at 265 to 1500 frames per second in release,
depending on how much is on screen. The pages holding a lot of shaped text sit
at the bottom of that range and the sparse ones at the top, which is the shape
of the cost: laying text out is the expensive part, not drawing it.

The event loop waits rather than spinning. A frame is drawn in response to
input, to a transition in progress, or to the stylesheet changing on disk. An
idle interface costs nothing.

### Memory

The chat application holds about **9 MB**, and stays there: 4.4 MB of font data
across six faces, 1 to 2.5 MB of shaped and laid out text, 2.1 MB of atlas
textures, and 0.2 MB of instance buffers. Every window reports its own
breakdown:

```rust
cx.memory.fonts          // font files held resident
cx.memory.text_caches    // shaped runs and laid out blocks
cx.memory.textures       // atlas and ramp textures
cx.memory.element_state  // retained per element state
```

The process as a whole is larger than that, and it is worth being precise about
why. On a Windows machine with an NVIDIA card, the Direct3D driver maps 163 MB
of DLLs into the process and commits around 100 MB of its own. That is the price
of GPU acceleration and every accelerated application pays it, this one and
Chromium alike. What a framework controls is the part above that line, and
keeping it in single digits is the point.

### Comparing this against a web view application

Not the way a task manager invites you to. An application built on a web view
is several processes, and the one carrying the application's name draws
nothing: the rendering happens in the web view's own processes, which the task
manager files under the runtime rather than under the application. Reading the
row with the application's name on it therefore reports the size of a shell.

Measured on one Windows machine with an NVIDIA card, against a released Tauri
application, by private working set, which is the number a task manager shows:

| | private working set |
| --- | --- |
| the Tauri application's own process | 10.6 MB |
| its six web view processes | 132.9 MB |
| **its interface, in total** | **143.5 MB** |
| the chat application here, one process | **111.7 MB** |

Two numbers rather than one, because the honest comparison is the whole tree
against the whole process. Its shell looks small for the same reason our
process looks large: ours is the only process we have, and the graphics driver
lives inside it. Theirs lives in a web view process listed somewhere else.

Five things were found by measuring rather than guessing:

- **Every vendor's driver was loading.** Asking wgpu for all backends made it
  enumerate Vulkan, Direct3D and OpenGL, which pulled in the AMD Vulkan driver
  and four NVIDIA drivers to choose one adapter. Asking for one native backend
  freed about 190 MB of mapped DLLs. `App::backend()` overrides it.
- **The atlases were allocated for the worst case.** Four pages each, 20 MB, to
  hold 289 glyphs using 0.07 MB. They now start at a single page and grow only
  when the packer spills, which is 2.1 MB in practice.
- **The shaped run cache had no bound.** It has a `trim_cache`, and nothing
  called it, so a long session with varied text grew forever. It is now bounded
  per frame.
- **Both text caches were bounded by entry count, which bounds nothing.** One
  entry is a word and another is a wrapped paragraph of a hundred lines, so four
  thousand of them is anywhere between one megabyte and forty. Watching a
  session that redraws continuously, the pair climbed to 14 MB before clearing.
  Bounded by bytes instead, they hold 1 to 2.5 MB and stay flat over tens of
  thousands of frames.
- **Font data was copied onto the heap.** Every face was read out of the font
  database into a private `Vec`. Sharing the database's own data instead maps
  the file, which makes those pages file backed rather than dirty: the system
  can drop them under pressure, and two processes using the same font share one
  copy. Measured on the release build by private working set, that is
  **119.5 MB down to 112.2 MB**
  for six Latin faces. A CJK face is tens of megabytes on its own, so it matters
  far more there.

Measuring is worth doing on the release build. The same application in a debug
build sits at 272 MB, almost none of which is anything a framework controls.

### Measuring it

Guessing at where a frame goes is a good way to optimize the wrong thing, so
the timings are part of the framework rather than something to bolt on. Every
phase is measured and exposed on the context:

```rust
cx.build_ms     // element tree and the style cascade
cx.layout_ms    // flexbox and text measurement
cx.paint_ms     // emitting primitives
cx.prepare_ms   // instance buffers, glyph rasterization, uploads
cx.acquire_ms   // waiting for the swap chain
cx.submit_ms    // recording and submitting
cx.rasterized   // glyphs rasterized this frame; zero in steady state
```

The demo prints all of them in its header. That breakdown is how the two real
performance bugs in this codebase were found: the first was an inherited style
field backed by a `Vec`, which allocated on every one of the several computed
style clones each element makes per frame; the second was blank glyphs. A space
has no bitmap, so the rasterizer returned nothing, so nothing was cached, so
every space on screen was rasterized again on every frame. That alone was 5.3ms
of a 8.3ms frame.

Vsync and GPU selection are both configurable, because a frame time measured
under vsync tells you about the display rather than the program:

```rust
App::new().without_vsync()   // uncapped, for measuring
App::new().low_power()       // prefer the integrated GPU
```

## What is not there yet

Stated plainly, because a framework that overstates its coverage is worse than
one that does less:

- **Dashed and dotted borders.** They parse and round trip, but render solid.
- **Group opacity.** `opacity` multiplies down the tree per element rather than
  compositing the subtree once, so overlapping translucent children show through
  each other.
- **`em` units.** `rem`, `px` and `%` are supported.
- **3D transforms.** `translate`, `rotate` and `scale` are there and they
  animate. Perspective, `rotateX` and `rotateY` are not.
- **Scrolling a text area.** An area grows to fit and can be capped with
  `max_h`, but it does not scroll to keep the caret in view once it is capped.
- **Walking a menu with the keyboard.** A menu opens, chooses and closes with
  the pointer, and every entry is reachable through the accessibility layer,
  but the arrow keys do not yet move through an open one.
- **Image formats beyond PNG and JPEG.** GIF, WebP and the rest are not
  compiled in, so they do not decode at all rather than decoding poorly. The
  decoder supports them; it is a feature flag and a decision about what a
  default build should weigh.
- **Animated images.** Nothing animates. A picture is one frame.
- **Nested rounded clips.** The innermost one's corners are kept and the outer
  one's are left to the scissor, which squares them.
- **Dialogs drawn in the window.** The file and message dialogs are the
  platform's own, which is the right default and not always the right answer:
  an application wanting a sheet that matches its own chrome has to build one.
- **Dragging out of the window.** Files can be dropped onto a window, and
  things can be dragged around inside one, but starting a drag that leaves the
  window is not implemented.
- **Reading inside a text field.** A field reports its name and its whole
  value, so a reader announces it and reads it back. Navigating it by character,
  word or line, and following the caret while doing so, needs the text range
  interface, which is not implemented.
- **Live regions.** Something changing without focus moving is not announced,
  so a reader hears nothing when a message arrives or a task finishes.
- **Accessibility on macOS and Linux.** Wired the same way through the same
  AccessKit adapters, but only verified against a live client on Windows.
- **Snap Layouts beyond Windows.** The maximize control claims its rectangle on
  Windows only. Nothing equivalent is wired up for the other platforms, which
  have their own conventions for this.

## Building and testing

Rust 1.90 or newer. Set by dependencies rather than by anything here:
`ordered-float` asks for 1.90 and `image` for 1.88, both above the 1.87 wgpu
wants.

```
cargo build --workspace
cargo test --workspace
```

On Linux the file dialogs go through the desktop portal, which needs the D-Bus
development headers: `libdbus-1-dev` on Debian and Ubuntu.

Every check that CI runs can be run locally, and none of them need a GPU:

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```

612 tests. They cover the parts where being wrong is quiet rather than loud:
selector specificity and matching, the cascade, variable resolution and cycles,
transition retargeting and interruption, atlas packing overlap, scene batching,
culling and layer ordering, flexbox and grid translation, element identity
stability, text editing across multi byte characters, which keys mean what and
where, and what part of a selection covers which run of text.

Several of them exist because they caught something. A right to left phrase with
a bold word in it came out with the two halves the wrong way round, because
shaping emitted the pieces of a right to left range in logical order. A
virtualized list renamed all its rows every time it scrolled by one, because
they were keyed by their position among the built children rather than by which
row they were. Animating a transform indexed one past the end of the transition
track array. The tests for those three fail if the fix is removed, which is the
only thing that makes them worth keeping.

Input routing is split out of the window into `InputRouter` specifically so it
can be driven without a GPU, because that is where the subtle bugs live. An
event travels along the whole chain of elements under the pointer rather than to
the innermost one, and the tests pin the consequences: a press on a button's
label still clicks the button, a release that drifts onto the parent's padding
still counts, clicking a text field's own text focuses it rather than blurring
it, a drag keeps reaching the slider it started on after the pointer leaves it,
a scrollbar wins the press over the row underneath it, a press outside an open
dropdown closes it while a press on its list does not, and adding a handler to
a widget does not silently destroy the one it already had.

Both stylesheets are checked by a test, because a theme that fails to parse is
a broken feature. So is the gallery itself: every section and tab is built, laid
out and painted headlessly, which catches a section that panics without anyone
having to click to it.

## License

Dual licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
