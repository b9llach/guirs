//! Menus: a bar across the top, a list that drops from it, one that opens
//! under the pointer, and lists that open from those.
//!
//! A menu item names a command rather than carrying an action. That is what
//! makes a menu and a keyboard shortcut the same thing said twice: the item
//! shows the key because it can ask the keymap what runs its command, and both
//! end up calling the same handler. Nothing has to be written down twice and
//! the two cannot drift apart.
//!
//! ```no_run
//! # use guirs_ui::{
//! #     keymap::Keymap,
//! #     menu::{menu_bar, menu_item, menu_separator, submenu, MenuState},
//! #     Model,
//! # };
//! # let open = Model::new(MenuState::default());
//! # let keymap = Keymap::new();
//! menu_bar(
//!     &[
//!         submenu("File", [
//!             menu_item("New").command("file:new"),
//!             menu_item("Open").command("file:open"),
//!             menu_separator(),
//!             menu_item("Save").command("file:save"),
//!         ]),
//!         submenu("Edit", [
//!             menu_item("Undo").command("edit:undo").enabled(false),
//!             menu_item("Redo").command("edit:redo"),
//!         ]),
//!     ],
//!     open,
//!     &keymap,
//! );
//! ```

use guirs_core::{Length, Px, SharedString};
use guirs_style::{StateFlags, Styled};

use crate::div::{div, Div};
use crate::model::Model;
use crate::text::text;

/// One entry in a menu.
///
/// Built rather than constructed, so an entry that is only a line has nothing
/// to say about labels and an entry with children has nothing to say about
/// commands.
#[derive(Clone, Debug)]
pub struct MenuItem {
    label: SharedString,
    command: Option<SharedString>,
    enabled: bool,
    checked: Option<bool>,
    is_separator: bool,
    children: Vec<MenuItem>,
}

/// An entry that runs a command.
pub fn menu_item(label: impl Into<SharedString>) -> MenuItem {
    MenuItem {
        label: label.into(),
        command: None,
        enabled: true,
        checked: None,
        is_separator: false,
        children: Vec::new(),
    }
}

/// A dividing line.
pub fn menu_separator() -> MenuItem {
    MenuItem {
        label: SharedString::default(),
        command: None,
        enabled: false,
        checked: None,
        is_separator: true,
        children: Vec::new(),
    }
}

/// An entry that opens a menu of its own.
pub fn submenu(
    label: impl Into<SharedString>,
    children: impl IntoIterator<Item = MenuItem>,
) -> MenuItem {
    MenuItem {
        label: label.into(),
        command: None,
        enabled: true,
        checked: None,
        is_separator: false,
        children: children.into_iter().collect(),
    }
}

impl MenuItem {
    /// What choosing this asks for.
    ///
    /// A name rather than a handler, so the same command can be reached from a
    /// key, from a menu, and from anywhere else, and answered in one place.
    pub fn command(mut self, command: impl Into<SharedString>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Whether this can be chosen. A disabled entry is shown and greyed rather
    /// than hidden, so a menu does not change shape as its state changes.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Show a tick beside this entry.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn is_separator(&self) -> bool {
        self.is_separator
    }

    pub fn children(&self) -> &[MenuItem] {
        &self.children
    }

    /// Whether choosing this does anything at all.
    fn is_choosable(&self) -> bool {
        self.enabled && !self.is_separator && (self.command.is_some() || !self.children.is_empty())
    }
}

/// Which menu is open, and which path of submenus inside it.
///
/// One value rather than a flag per menu, because at most one menu is open at
/// a time and two that both believed they were open would draw over each
/// other.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// Which top level menu is open, if any.
    pub open: Option<usize>,
    /// The submenu opened inside it, as a path of indices.
    pub path: Vec<usize>,
}

impl MenuState {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
        self.path.clear();
    }

    /// Open a top level menu, closing whatever was open.
    pub fn open_menu(&mut self, index: usize) {
        self.open = Some(index);
        self.path.clear();
    }

    /// Note that a submenu at `depth` is open at `index`.
    ///
    /// Everything deeper is closed, because moving along a menu to a different
    /// entry has to shut whatever the previous entry had opened.
    pub fn open_at(&mut self, depth: usize, index: usize) {
        self.path.truncate(depth);
        self.path.push(index);
    }

    /// Whether the entry at `index`, `depth` levels in, is the open one.
    pub fn is_open_at(&self, depth: usize, index: usize) -> bool {
        self.path.get(depth).copied() == Some(index)
    }
}

/// A bar of menus across the top of a window.
pub fn menu_bar(
    menus: &[MenuItem],
    state: Model<MenuState>,
    keymap: &crate::keymap::Keymap,
) -> Div {
    let mut bar = div().class("menu-bar").row().items_center();

    for (index, menu) in menus.iter().enumerate() {
        let is_open = state.read().open == Some(index);
        let items = menu.children().to_vec();

        let toggle = state.clone();
        let hover = state.clone();

        let mut title = div()
                .class("menu-title")
                .relative()
                .row()
                .items_center()
                .cursor_pointer()
                .state_if(StateFlags::OPEN, is_open)
                // Named itself, because the menu it opens is painted inside
                // it and a name gathered from its contents would be the title
                // followed by everything on the menu.
                .label(menu.label().clone())
                .child(text(menu.label().clone()).whitespace_nowrap())
                .on_click(move |_, cx| {
                    toggle.update(|state| {
                        if state.open == Some(index) {
                            state.close();
                        } else {
                            state.open_menu(index);
                        }
                    });
                    cx.stop_propagation();
                    cx.request_redraw();
                })
                // Once one menu is open, moving along the bar opens the next
                // rather than needing another click, which is how every menu
                // bar behaves.
                .on_mouse_move(move |_, cx| {
                    let already = hover.read().open;
                    if already.is_some() && already != Some(index) {
                        hover.update(|state| state.open_menu(index));
                        cx.request_redraw();
                    }
                });

        // Added with an `if` rather than through a closure, because the panel
        // borrows the keymap and a closure would outlive that borrow.
        if is_open {
            title = title.child(
                panel(&items, state.clone(), 0, keymap)
                    .absolute()
                    .top(Length::Percent(1.0))
                    .left(Px(0.0)),
            );
        }
        bar = bar.child(title);
    }

    let dismiss = state.clone();
    bar.on_dismiss(move |cx| {
        if dismiss.read().is_open() {
            dismiss.update(|state| state.close());
            cx.request_redraw();
        }
    })
}

/// A menu that opens where it is put.
///
/// Used for a context menu, or anywhere a list of commands belongs that is not
/// hanging off a bar. Place it inside a `relative()` parent.
pub fn context_menu(
    items: &[MenuItem],
    state: Model<MenuState>,
    keymap: &crate::keymap::Keymap,
) -> Div {
    panel(items, state, 0, keymap).absolute()
}

/// One list of entries, and any list opened from it.
///
/// `depth` is how many submenus deep this one is, which is what lets an entry
/// know whether the path recorded in the state refers to it.
fn panel(
    items: &[MenuItem],
    state: Model<MenuState>,
    depth: usize,
    keymap: &crate::keymap::Keymap,
) -> Div {
    let mut list = div().class("menu").overlay().col();

    for (index, entry) in items.iter().enumerate() {
        if entry.is_separator() {
            list = list.child(div().class("menu-separator"));
            continue;
        }

        let has_children = !entry.children().is_empty();
        let open_here = has_children && state.read().is_open_at(depth, index);
        let choosable = entry.is_choosable();

        let mut row = div()
            .class("menu-item")
            .relative()
            .row()
            .items_center()
            .state_if(StateFlags::DISABLED, !entry.enabled)
            .state_if(StateFlags::OPEN, open_here)
            // Named itself for the same reason: an entry with a submenu open
            // inside it would otherwise be called after the whole submenu.
            .label(entry.label.clone());
        if choosable {
            row = row.cursor_pointer();
        }

        // A tick, kept out of the description: the item already says whether
        // it is checked, and reading the mark aloud as well says it twice.
        if let Some(checked) = entry.checked {
            row = row.child(
                text(if checked { "\u{2713}" } else { " " })
                    .class("menu-check")
                    .decorative(),
            );
        }
        row = row.child(
            text(entry.label.clone())
                .class("menu-label")
                .whitespace_nowrap(),
        );

        row = row.child(div().class("menu-spacer").flex_1());
        if has_children {
            row = row.child(text("\u{203a}").class("menu-arrow").decorative());
        } else if let Some(keys) = entry
            .command
            .as_ref()
            .and_then(|command| keymap.shortcut_for(command))
        {
            // Read from the keymap rather than written on the item, so
            // rebinding a key changes the menu with it and the two cannot
            // disagree about what runs the command.
            // Carried as the shortcut rather than as part of the name, so a
            // reader announces "New" and offers the key separately instead of
            // reading "New Ctrl plus N" as though that were its name.
            row = row.access_shortcut(keys.clone());
            row = row.child(
                text(keys)
                    .class("menu-shortcut")
                    .whitespace_nowrap()
                    .decorative(),
            );
        }

        // Entering an entry opens it if it has children and closes whatever
        // the previous entry opened if it does not. Without the second half a
        // submenu stays on screen after the pointer has moved past it.
        let enter = state.clone();
        row = row.on_mouse_move(move |_, cx| {
            let changed = enter.update(|state| {
                if has_children {
                    if state.is_open_at(depth, index) {
                        return false;
                    }
                    state.open_at(depth, index);
                    true
                } else if state.path.len() > depth {
                    state.path.truncate(depth);
                    true
                } else {
                    false
                }
            });
            if changed {
                cx.request_redraw();
            }
        });

        if let Some(command) = entry.command.clone() {
            if entry.enabled {
                let close = state.clone();
                row = row.on_click(move |_, cx| {
                    // Closed before the command runs, so a command that opens
                    // a window or a dialog does not leave the menu behind it.
                    close.update(|state| state.close());
                    cx.dispatch_command(command.clone());
                    cx.stop_propagation();
                    cx.request_redraw();
                });
            }
        }

        if open_here {
            row = row.child(
                panel(entry.children(), state.clone(), depth + 1, keymap)
                    .absolute()
                    .left(Length::Percent(1.0))
                    .top(Px(0.0)),
            );
        }

        list = list.child(row);
    }

    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<MenuItem> {
        vec![
            submenu(
                "File",
                [
                    menu_item("New").command("file:new"),
                    menu_separator(),
                    menu_item("Save").command("file:save"),
                    submenu("Recent", [menu_item("a.rs").command("file:open-recent")]),
                ],
            ),
            submenu("Edit", [menu_item("Undo").command("edit:undo").enabled(false)]),
        ]
    }

    #[test]
    fn at_most_one_menu_is_open() {
        let mut state = MenuState::default();
        state.open_menu(0);
        assert_eq!(state.open, Some(0));
        // Opening another closes the first rather than adding to it. Two menus
        // that both thought they were open would draw over each other.
        state.open_menu(1);
        assert_eq!(state.open, Some(1));
        assert!(state.path.is_empty(), "the previous menu's path survived");
    }

    #[test]
    fn opening_a_submenu_closes_the_one_beside_it() {
        // Moving down a menu from one entry with children to another has to
        // shut the first, or both submenus stay on screen at once.
        let mut state = MenuState::default();
        state.open_menu(0);
        state.open_at(0, 3);
        assert!(state.is_open_at(0, 3));
        state.open_at(0, 1);
        assert!(state.is_open_at(0, 1));
        assert!(!state.is_open_at(0, 3), "the first submenu stayed open");
    }

    #[test]
    fn opening_a_deeper_submenu_keeps_the_one_it_opened_from() {
        let mut state = MenuState::default();
        state.open_menu(0);
        state.open_at(0, 3);
        state.open_at(1, 0);
        assert!(state.is_open_at(0, 3), "the parent closed itself");
        assert!(state.is_open_at(1, 0));
    }

    #[test]
    fn going_back_up_closes_what_was_below() {
        let mut state = MenuState::default();
        state.open_menu(0);
        state.open_at(0, 3);
        state.open_at(1, 0);
        // Back to a shallower entry: everything deeper has to go.
        state.open_at(0, 1);
        assert_eq!(state.path, vec![1]);
    }

    #[test]
    fn closing_forgets_where_it_was() {
        let mut state = MenuState::default();
        state.open_menu(0);
        state.open_at(0, 3);
        state.close();
        assert!(!state.is_open());
        assert!(state.path.is_empty(), "reopening would restore a stale submenu");
    }

    #[test]
    fn a_separator_cannot_be_chosen() {
        assert!(!menu_separator().is_choosable());
        // Nor can an entry that names nothing, nor a disabled one.
        assert!(!menu_item("Nothing").is_choosable());
        assert!(!menu_item("Undo").command("edit:undo").enabled(false).is_choosable());
        assert!(menu_item("Save").command("file:save").is_choosable());
        // A submenu is choosable without a command of its own: opening it is
        // what choosing it does.
        assert!(submenu("Recent", [menu_item("a").command("b")]).is_choosable());
    }

    #[test]
    fn a_menu_keeps_the_shape_it_was_given() {
        let menus = sample();
        assert_eq!(menus.len(), 2);
        let file = &menus[0];
        assert_eq!(file.label().as_ref(), "File");
        assert_eq!(file.children().len(), 4);
        assert!(file.children()[1].is_separator());
        assert_eq!(file.children()[3].children().len(), 1);
    }
}
