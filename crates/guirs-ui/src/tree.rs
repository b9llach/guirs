//! A tree of things that can be opened, closed and walked.
//!
//! A file explorer, an outline, a scene graph. What makes a tree different
//! from a list is that what is on screen is not what is in the data: only the
//! open parts are visible, and moving down a row can mean stepping into a
//! branch or out of two.
//!
//! That flattening is the whole difficulty, so it is a function of its own
//! that can be tested without drawing anything, and applications that want to
//! virtualize a very large tree can call it and lay the rows out themselves.
//!
//! ```
//! # use guirs_ui::{tree::{node, tree, TreeState}, Model};
//! let state = Model::new(TreeState::default());
//! let roots = [
//!     node("src", "src").children([node("src/main.rs", "main.rs")]),
//!     node("Cargo.toml", "Cargo.toml"),
//! ];
//! tree(&roots, state, |_key| {});
//! ```

use std::collections::HashSet;
use std::rc::Rc;

use guirs_core::{Px, SharedString};
use guirs_style::{StateFlags, Styled};

use crate::div::{div, Div};
use crate::model::Model;
use crate::text::text;

/// How far each level is indented, in logical pixels.
pub const INDENT: f32 = 14.0;

/// One thing in a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    key: SharedString,
    label: SharedString,
    children: Vec<TreeNode>,
}

/// A node, identified by a key that has to be unique across the whole tree.
///
/// The key is what open and selected are remembered against, so it has to
/// survive the tree being rebuilt. A path does; a position in a list does not.
pub fn node(key: impl Into<SharedString>, label: impl Into<SharedString>) -> TreeNode {
    TreeNode {
        key: key.into(),
        label: label.into(),
        children: Vec::new(),
    }
}

impl TreeNode {
    pub fn children(mut self, children: impl IntoIterator<Item = TreeNode>) -> Self {
        self.children = children.into_iter().collect();
        self
    }

    pub fn key(&self) -> &SharedString {
        &self.key
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn child_nodes(&self) -> &[TreeNode] {
        &self.children
    }

    /// Whether this can be opened.
    ///
    /// A branch with nothing in it yet still counts as one, because a file
    /// explorer that has not read a directory cannot know whether it is empty
    /// and must not claim it is a leaf.
    pub fn is_branch(&self) -> bool {
        !self.children.is_empty()
    }
}

/// What is open and what is chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeState {
    expanded: HashSet<SharedString>,
    selected: Option<SharedString>,
}

impl TreeState {
    /// A tree with some branches already open.
    pub fn with_expanded(keys: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        TreeState {
            expanded: keys.into_iter().map(Into::into).collect(),
            selected: None,
        }
    }

    pub fn is_expanded(&self, key: &SharedString) -> bool {
        self.expanded.contains(key)
    }

    pub fn expand(&mut self, key: &SharedString) {
        self.expanded.insert(key.clone());
    }

    pub fn collapse(&mut self, key: &SharedString) {
        self.expanded.remove(key);
    }

    pub fn toggle(&mut self, key: &SharedString) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.clone());
        }
    }

    pub fn selected(&self) -> Option<&SharedString> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, key: impl Into<SharedString>) {
        self.selected = Some(key.into());
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }
}

/// One visible row of a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow<'a> {
    pub node: &'a TreeNode,
    /// How many levels in, for indenting.
    pub depth: usize,
    /// Whether this row's branch is open.
    pub expanded: bool,
}

/// Every row that is currently visible, in the order they appear.
///
/// Closed branches contribute one row and nothing beneath them. This is the
/// only place the shape of the tree turns into a list, so it is the only place
/// that has to get it right.
pub fn flatten<'a>(roots: &'a [TreeNode], state: &TreeState) -> Vec<TreeRow<'a>> {
    let mut rows = Vec::new();
    walk(roots, state, 0, &mut rows);
    rows
}

fn walk<'a>(nodes: &'a [TreeNode], state: &TreeState, depth: usize, rows: &mut Vec<TreeRow<'a>>) {
    for node in nodes {
        let expanded = node.is_branch() && state.is_expanded(&node.key);
        rows.push(TreeRow {
            node,
            depth,
            expanded,
        });
        if expanded {
            walk(&node.children, state, depth + 1, rows);
        }
    }
}

/// Where the selection goes when a key is pressed.
///
/// Separate from the element so the rules can be checked without a window.
/// Returns the new selection, or `None` if the press means nothing here.
pub fn navigate(
    roots: &[TreeNode],
    state: &mut TreeState,
    key: crate::event::Key,
) -> Option<SharedString> {
    use crate::event::Key;

    let rows = flatten(roots, state);
    if rows.is_empty() {
        return None;
    }
    let current = state
        .selected()
        .and_then(|key| rows.iter().position(|row| row.node.key() == key));

    match key {
        Key::Down => {
            let next = match current {
                Some(index) => (index + 1).min(rows.len() - 1),
                None => 0,
            };
            Some(rows[next].node.key().clone())
        }
        Key::Up => {
            let next = match current {
                Some(index) => index.saturating_sub(1),
                None => rows.len() - 1,
            };
            Some(rows[next].node.key().clone())
        }
        Key::Right => {
            let row = rows.get(current?)?;
            if row.node.is_branch() && !row.expanded {
                // Opening a closed branch, rather than moving.
                state.expand(row.node.key());
                return Some(row.node.key().clone());
            }
            // Already open, or a leaf: step into the first child if there is
            // one, which is what makes holding Right walk down a path.
            if row.expanded {
                return rows.get(current? + 1).map(|row| row.node.key().clone());
            }
            None
        }
        Key::Left => {
            let index = current?;
            let row = rows.get(index)?;
            if row.expanded {
                state.collapse(row.node.key());
                return Some(row.node.key().clone());
            }
            // On something already closed, go out to whatever contains it,
            // which is the nearest row above that is one level shallower.
            let parent = rows[..index]
                .iter()
                .rev()
                .find(|above| above.depth < row.depth)?;
            Some(parent.node.key().clone())
        }
        Key::Home => Some(rows[0].node.key().clone()),
        Key::End => Some(rows[rows.len() - 1].node.key().clone()),
        _ => None,
    }
}

/// A tree, drawn.
///
/// Every visible row is built, so a very large tree wants
/// [`flatten`] and a virtualized list instead of this.
pub fn tree(
    roots: &[TreeNode],
    state: Model<TreeState>,
    on_select: impl Fn(&SharedString) + 'static,
) -> Div {
    let rows = flatten(roots, &state.read());
    let selected = state.read().selected().cloned();
    let callback = Rc::new(on_select);

    // The roots are needed inside the key handler, which outlives this call.
    let owned: Vec<TreeNode> = roots.to_vec();
    let keys = state.clone();
    let chosen = callback.clone();

    let mut list = div()
        .class("tree")
        .col()
        .focusable()
        .role(crate::access::AccessRole::Tree)
        .on_key_down(move |event, cx| {
            if !event.modifiers.none() {
                return;
            }
            let moved = keys.update(|state| navigate(&owned, state, event.key));
            if let Some(key) = moved {
                keys.update(|state| state.select(key.clone()));
                chosen(&key);
                cx.stop_propagation();
                cx.request_redraw();
            }
        });

    for row in &rows {
        let key = row.node.key().clone();
        let is_selected = selected.as_ref() == Some(&key);
        let branch = row.node.is_branch();

        let toggle = state.clone();
        let toggle_key = key.clone();
        let choose = callback.clone();
        let choose_state = state.clone();
        let choose_key = key.clone();

        let mut entry = div()
            .class("tree-row")
            .row()
            .items_center()
            .cursor_pointer()
            .state_if(StateFlags::SELECTED, is_selected)
            .state_if(StateFlags::OPEN, row.expanded)
            .role(crate::access::AccessRole::TreeItem)
            // Named itself: a branch that is open paints its children inside
            // the same list, and a name gathered from what is inside would
            // pick up the rows below it.
            .label(row.node.label().clone())
            .pl(Px(row.depth as f32 * INDENT));

        // A twist that is a picture of open or closed, and nothing to read.
        entry = entry.child(if branch {
            text(if row.expanded { "\u{25be}" } else { "\u{25b8}" })
                .class("tree-twist")
                .decorative()
        } else {
            text(" ").class("tree-twist").decorative()
        });

        entry = entry
            .child(text(row.node.label().clone()).whitespace_nowrap())
            .on_click(move |_, cx| {
                choose_state.update(|state| state.select(choose_key.clone()));
                choose(&choose_key);
                cx.request_redraw();
            });

        if branch {
            // The twist opens and closes; the rest of the row chooses. Both
            // happen when the twist is clicked, which is what every file
            // explorer does: opening a folder selects it too.
            //
            // Measured from the row's content box, which already has the
            // indent taken off it, so the twist is simply the first column.
            entry = entry.on_mouse_down(move |event, cx| {
                if event.local.x < Px(INDENT) {
                    toggle.update(|state| state.toggle(&toggle_key));
                    cx.request_redraw();
                }
            });
        }

        list = list.child(entry);
    }

    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Key;

    fn sample() -> Vec<TreeNode> {
        vec![
            node("src", "src").children([
                node("src/main.rs", "main.rs"),
                node("src/ui", "ui").children([node("src/ui/mod.rs", "mod.rs")]),
            ]),
            node("Cargo.toml", "Cargo.toml"),
        ]
    }

    fn keys<'a>(rows: &'a [TreeRow<'a>]) -> Vec<&'a str> {
        rows.iter().map(|row| row.node.key().as_ref()).collect()
    }

    #[test]
    fn a_closed_tree_shows_only_its_roots() {
        let roots = sample();
        let rows = flatten(&roots, &TreeState::default());
        assert_eq!(keys(&rows), ["src", "Cargo.toml"]);
    }

    #[test]
    fn opening_a_branch_shows_what_is_directly_inside_it() {
        let state = TreeState::with_expanded(["src"]);
        let roots = sample();
        let rows = flatten(&roots, &state);
        assert_eq!(keys(&rows), ["src", "src/main.rs", "src/ui", "Cargo.toml"]);
        // And not what is inside the branch that is still closed.
        assert_eq!(rows[2].depth, 1);
        assert!(!rows[2].expanded);
    }

    #[test]
    fn opening_a_branch_inside_a_closed_one_shows_nothing() {
        // The inner branch is open, but nothing above it is. Getting this
        // wrong shows rows with no visible parent.
        let state = TreeState::with_expanded(["src/ui"]);
        let roots = sample();
        let rows = flatten(&roots, &state);
        assert_eq!(keys(&rows), ["src", "Cargo.toml"]);
    }

    #[test]
    fn depth_counts_levels_rather_than_rows() {
        let state = TreeState::with_expanded(["src", "src/ui"]);
        let roots = sample();
        let rows = flatten(&roots, &state);
        let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
        assert_eq!(keys(&rows), ["src", "src/main.rs", "src/ui", "src/ui/mod.rs", "Cargo.toml"]);
        assert_eq!(depths, [0, 1, 1, 2, 0]);
    }

    #[test]
    fn a_leaf_is_never_open_however_the_state_is_set() {
        // A key can be left in the open set by a tree that changed shape.
        // A leaf that claimed to be open would draw a twist pointing down
        // with nothing under it.
        let state = TreeState::with_expanded(["Cargo.toml"]);
        let roots = sample();
        let rows = flatten(&roots, &state);
        let leaf = rows.iter().find(|row| row.node.key() == "Cargo.toml").unwrap();
        assert!(!leaf.expanded);
    }

    #[test]
    fn toggling_opens_then_closes() {
        let mut state = TreeState::default();
        let key = SharedString::from("src");
        state.toggle(&key);
        assert!(state.is_expanded(&key));
        state.toggle(&key);
        assert!(!state.is_expanded(&key));
    }

    // -- walking with the keyboard ------------------------------------------

    #[test]
    fn down_and_up_move_through_what_is_visible() {
        let roots = sample();
        let mut state = TreeState::with_expanded(["src"]);

        // Nothing chosen yet, so down starts at the top.
        let first = navigate(&roots, &mut state, Key::Down).unwrap();
        assert_eq!(first.as_ref(), "src");
        state.select(first);

        for expected in ["src/main.rs", "src/ui", "Cargo.toml"] {
            let next = navigate(&roots, &mut state, Key::Down).unwrap();
            assert_eq!(next.as_ref(), expected);
            state.select(next);
        }
        // And it stops at the end rather than wrapping.
        let last = navigate(&roots, &mut state, Key::Down).unwrap();
        assert_eq!(last.as_ref(), "Cargo.toml");

        let back = navigate(&roots, &mut state, Key::Up).unwrap();
        assert_eq!(back.as_ref(), "src/ui");
    }

    #[test]
    fn right_opens_a_closed_branch_before_moving_into_it() {
        // Two presses: the first opens, the second steps in. Doing both at
        // once skips past whatever the branch contains.
        let roots = sample();
        let mut state = TreeState::default();
        state.select("src");

        let stayed = navigate(&roots, &mut state, Key::Right).unwrap();
        assert_eq!(stayed.as_ref(), "src", "it moved before opening");
        assert!(state.is_expanded(&SharedString::from("src")));

        let moved = navigate(&roots, &mut state, Key::Right).unwrap();
        assert_eq!(moved.as_ref(), "src/main.rs");
    }

    #[test]
    fn left_closes_an_open_branch_and_then_steps_out() {
        let roots = sample();
        let mut state = TreeState::with_expanded(["src"]);
        state.select("src/main.rs");

        // On a leaf, out to whatever contains it.
        let out = navigate(&roots, &mut state, Key::Left).unwrap();
        assert_eq!(out.as_ref(), "src");
        state.select(out);

        // On the open branch, closed rather than moved.
        let closed = navigate(&roots, &mut state, Key::Left).unwrap();
        assert_eq!(closed.as_ref(), "src");
        assert!(!state.is_expanded(&SharedString::from("src")));
    }

    #[test]
    fn left_at_the_top_level_has_nowhere_to_go() {
        let roots = sample();
        let mut state = TreeState::default();
        state.select("Cargo.toml");
        assert_eq!(navigate(&roots, &mut state, Key::Left), None);
    }

    #[test]
    fn stepping_out_skips_past_siblings_to_the_real_parent() {
        // The row above a deeply nested node is often a sibling rather than
        // its parent. Taking the row above would step sideways.
        let roots = sample();
        let mut state = TreeState::with_expanded(["src", "src/ui"]);
        state.select("src/ui/mod.rs");
        let out = navigate(&roots, &mut state, Key::Left).unwrap();
        assert_eq!(out.as_ref(), "src/ui");
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_what_is_visible() {
        let roots = sample();
        let mut state = TreeState::with_expanded(["src"]);
        state.select("src/ui");
        assert_eq!(navigate(&roots, &mut state, Key::Home).unwrap().as_ref(), "src");
        assert_eq!(
            navigate(&roots, &mut state, Key::End).unwrap().as_ref(),
            "Cargo.toml"
        );
    }

    #[test]
    fn navigating_an_empty_tree_does_nothing() {
        let mut state = TreeState::default();
        assert_eq!(navigate(&[], &mut state, Key::Down), None);
    }

    #[test]
    fn a_selection_that_is_no_longer_visible_does_not_strand_the_keyboard() {
        // The branch containing the selection was closed. Down has to start
        // again from somewhere rather than doing nothing forever.
        let roots = sample();
        let mut state = TreeState::default();
        state.select("src/main.rs");
        let next = navigate(&roots, &mut state, Key::Down).unwrap();
        assert_eq!(next.as_ref(), "src");
    }
}
