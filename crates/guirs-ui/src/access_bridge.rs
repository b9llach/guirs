//! Handing the interface's own description over to the platform.
//!
//! [`crate::access`] builds a tree of what the interface is. This turns that
//! into the shape AccessKit wants and answers what a screen reader asks for.
//! Keeping the two apart means the model can be built and tested without a
//! screen reader anywhere near it, which is the only way this ever gets
//! exercised in a test suite.

use accesskit::{Action, Node, NodeId, Rect, Role, Toggled, Tree, TreeId, TreeUpdate};
use guirs_core::{GlobalElementId, ScaleFactor};

use crate::access::{AccessRole, AccessTree};

/// The identifier for the window itself, which owns everything else.
const ROOT: NodeId = NodeId(0);

/// AccessKit identifies nodes by number, and an element's own identity already
/// is one. Shifted so nothing collides with the root.
fn node_id(id: GlobalElementId) -> NodeId {
    NodeId(id.0.wrapping_add(1))
}

fn to_accesskit(role: AccessRole) -> Role {
    match role {
        AccessRole::Group => Role::GenericContainer,
        AccessRole::Window => Role::Window,
        AccessRole::Button => Role::Button,
        AccessRole::Label => Role::Label,
        AccessRole::TextInput => Role::TextInput,
        AccessRole::CheckBox => Role::CheckBox,
        AccessRole::RadioButton => Role::RadioButton,
        AccessRole::Switch => Role::Switch,
        AccessRole::Slider => Role::Slider,
        AccessRole::ProgressIndicator => Role::ProgressIndicator,
        AccessRole::ComboBox => Role::ComboBox,
        AccessRole::List => Role::List,
        AccessRole::ListItem => Role::ListItem,
        AccessRole::TabList => Role::TabList,
        AccessRole::Tab => Role::Tab,
        AccessRole::ScrollView => Role::ScrollView,
        AccessRole::Image => Role::Image,
        AccessRole::Link => Role::Link,
        AccessRole::Splitter => Role::Splitter,
        AccessRole::Tree => Role::Tree,
        AccessRole::TreeItem => Role::TreeItem,
        AccessRole::Table => Role::Table,
        AccessRole::Row => Role::Row,
        AccessRole::Cell => Role::Cell,
        AccessRole::ColumnHeader => Role::ColumnHeader,
    }
}

/// Turn a painted frame into something a screen reader can walk.
///
/// Bounds go over in physical pixels relative to the window, which is what
/// every platform's accessibility layer expects: it is placing a highlight on
/// a screen, not inside a layout.
pub fn build(
    tree: &AccessTree,
    focused: Option<GlobalElementId>,
    scale: ScaleFactor,
    title: &str,
) -> TreeUpdate {
    let scale = scale.0.max(0.01) as f64;
    let mut nodes: Vec<(NodeId, Node)> = Vec::with_capacity(tree.nodes.len() + 1);

    let mut root = Node::new(Role::Window);
    root.set_label(title.to_string());

    // Everything with no parent hangs off the window.
    let top: Vec<NodeId> = tree
        .nodes
        .iter()
        .filter(|node| node.parent.is_none())
        .map(|node| node_id(node.id))
        .collect();
    root.set_children(top);

    for node in &tree.nodes {
        let mut out = Node::new(to_accesskit(node.role));

        if let Some(label) = &node.label {
            out.set_label(label.to_string());
        }
        if let Some(value) = &node.value {
            out.set_value(value.to_string());
        } else if node.role == AccessRole::Label {
            // A run of text is read from its value, not its label. Setting
            // only the label leaves every piece of prose in the interface
            // nameless, which is most of what there is to read.
            if let Some(label) = &node.label {
                out.set_value(label.to_string());
            }
        }
        if let Some(checked) = node.checked {
            out.set_toggled(if checked { Toggled::True } else { Toggled::False });
        }
        if let Some([value, min, max]) = node.numeric {
            out.set_numeric_value(value);
            out.set_min_numeric_value(min);
            out.set_max_numeric_value(max);
        }
        if let Some(description) = &node.description {
            out.set_description(description.to_string());
        }
        if let Some(shortcut) = &node.shortcut {
            out.set_keyboard_shortcut(shortcut.to_string());
        }
        if node.disabled {
            out.set_disabled();
        }

        out.set_bounds(Rect {
            x0: node.bounds.origin.x.0 as f64 * scale,
            y0: node.bounds.origin.y.0 as f64 * scale,
            x1: (node.bounds.origin.x.0 + node.bounds.size.width.0) as f64 * scale,
            y1: (node.bounds.origin.y.0 + node.bounds.size.height.0) as f64 * scale,
        });

        // What a reader is allowed to ask for. Clicking something that cannot
        // be clicked is worse than not offering it: a person hears an action
        // and nothing happens.
        if !node.disabled {
            // From the handler rather than the role. A control that does
            // nothing should not be offered, and a navigation item that does
            // something should be, whatever it is called.
            if node.clickable {
                out.add_action(Action::Click);
            }
            if node.focusable {
                out.add_action(Action::Focus);
            }
        }

        out.set_children(
            node.children
                .iter()
                .filter_map(|index| tree.nodes.get(*index))
                .map(|child| node_id(child.id))
                .collect::<Vec<_>>(),
        );

        nodes.push((node_id(node.id), out));
    }

    nodes.insert(0, (ROOT, root));

    let mut update = TreeUpdate {
        nodes,
        tree: Some(Tree::new(ROOT)),
        // One window, one tree, so the reserved root identifier is the right
        // one rather than a fresh one per frame.
        tree_id: TreeId::ROOT,
        focus: ROOT,
    };

    // A reader follows the keyboard, so it has to be told where it went. An
    // element that has focus but was not described falls back to the window,
    // which is at least somewhere real.
    if let Some(id) = focused {
        if tree.index_of(id).is_some() {
            update.focus = node_id(id);
        }
    }

    update
}

/// The element an action was requested on, if it is one of ours.
pub fn element_for(node: NodeId) -> Option<GlobalElementId> {
    if node == ROOT {
        return None;
    }
    Some(GlobalElementId(node.0.wrapping_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::AccessRole;
    use guirs_core::{Bounds, Px};

    fn sample() -> AccessTree {
        let mut tree = AccessTree::default();
        tree.active = true;
        tree.push(
            GlobalElementId(10),
            AccessRole::Button,
            Bounds::from_xywh(Px(4.0), Px(2.0), Px(20.0), Px(10.0)),
        );
        if let Some(node) = tree.current_mut() {
            node.label = Some("Save".into());
            node.focusable = true;
            node.clickable = true;
        }
        tree.pop();
        tree
    }

    #[test]
    fn every_described_element_reaches_the_platform() {
        let update = build(&sample(), None, ScaleFactor(1.0), "guirs");
        // The window, plus the button.
        assert_eq!(update.nodes.len(), 2);
        assert_eq!(update.nodes[0].0, ROOT);
        assert_eq!(update.nodes[1].0, node_id(GlobalElementId(10)));
    }

    #[test]
    fn an_identity_survives_the_round_trip() {
        let id = GlobalElementId(4321);
        assert_eq!(element_for(node_id(id)), Some(id));
        // The window is not an element, and must not be mistaken for one.
        assert!(element_for(ROOT).is_none());
    }

    #[test]
    fn the_first_element_does_not_collide_with_the_window() {
        // Identities start at zero, and so does the root, so one of them has
        // to move or the first element painted would replace the window.
        assert_ne!(node_id(GlobalElementId(0)), ROOT);
    }

    #[test]
    fn bounds_go_over_in_physical_pixels() {
        let update = build(&sample(), None, ScaleFactor(1.5), "guirs");
        let bounds = update.nodes[1].1.bounds().expect("no bounds");
        // A highlight is drawn on a screen, not inside a layout, so the scale
        // has to be applied or it lands in the wrong place on a scaled display.
        assert!((bounds.x0 - 6.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.y0 - 3.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.x1 - 36.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.y1 - 18.0).abs() < 0.01, "{bounds:?}");
    }

    #[test]
    fn focus_follows_the_keyboard() {
        let tree = sample();
        let update = build(&tree, Some(GlobalElementId(10)), ScaleFactor(1.0), "guirs");
        assert_eq!(update.focus, node_id(GlobalElementId(10)));

        // Something focused that was never described falls back to the window
        // rather than pointing a reader at a node that does not exist.
        let update = build(&tree, Some(GlobalElementId(999)), ScaleFactor(1.0), "guirs");
        assert_eq!(update.focus, ROOT);
    }

    #[test]
    fn a_control_offers_the_actions_it_can_actually_perform() {
        let update = build(&sample(), None, ScaleFactor(1.0), "guirs");
        let node = &update.nodes[1].1;
        assert!(node.supports_action(Action::Click));
        assert!(node.supports_action(Action::Focus));
    }

    #[test]
    fn anything_with_a_handler_can_be_pressed() {
        // A navigation item is activated the same way a button is. Offering
        // the action only to the things called buttons leaves the rest of an
        // interface unreachable to anyone using a reader.
        let mut tree = AccessTree::default();
        tree.active = true;
        tree.push(
            GlobalElementId(1),
            AccessRole::ListItem,
            Bounds::from_xywh(Px::ZERO, Px::ZERO, Px(10.0), Px(10.0)),
        );
        tree.current_mut().unwrap().clickable = true;
        tree.pop();

        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        assert!(update.nodes[1].1.supports_action(Action::Click));
    }

    #[test]
    fn a_control_that_does_nothing_is_not_offered() {
        let mut tree = sample();
        tree.nodes[0].clickable = false;
        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        assert!(
            !update.nodes[1].1.supports_action(Action::Click),
            "a reader was offered a press that goes nowhere"
        );
    }

    #[test]
    fn a_disabled_control_offers_nothing() {
        let mut tree = sample();
        tree.nodes[0].disabled = true;
        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        let node = &update.nodes[1].1;
        assert!(node.is_disabled());
        assert!(
            !node.supports_action(Action::Click),
            "a reader was offered an action that does nothing"
        );
    }

    #[test]
    fn a_checkbox_carries_whether_it_is_ticked() {
        let mut tree = AccessTree::default();
        tree.active = true;
        tree.push(
            GlobalElementId(3),
            AccessRole::CheckBox,
            Bounds::from_xywh(Px::ZERO, Px::ZERO, Px(10.0), Px(10.0)),
        );
        tree.current_mut().unwrap().checked = Some(true);
        tree.pop();

        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        assert_eq!(update.nodes[1].1.toggled(), Some(Toggled::True));
    }

    #[test]
    fn a_run_of_text_is_readable() {
        // AccessKit takes a label node's name from its value rather than its
        // label, so text that sets only the label reaches a screen reader
        // nameless, and the interface has nothing to read.
        let mut tree = AccessTree::default();
        tree.active = true;
        tree.push(
            GlobalElementId(7),
            AccessRole::Label,
            Bounds::from_xywh(Px::ZERO, Px::ZERO, Px(80.0), Px(20.0)),
        );
        tree.current_mut().unwrap().text = Some("What can I help with?".into());
        tree.pop();
        tree.resolve_labels();

        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        assert_eq!(
            update.nodes[1].1.value(),
            Some("What can I help with?"),
            "a screen reader would announce nothing here"
        );
    }

    #[test]
    fn the_window_owns_everything_that_has_no_parent() {
        let mut tree = AccessTree::default();
        tree.active = true;
        for n in 0..3u64 {
            tree.push(
                GlobalElementId(n),
                AccessRole::Button,
                Bounds::from_xywh(Px::ZERO, Px::ZERO, Px(10.0), Px(10.0)),
            );
            tree.pop();
        }
        let update = build(&tree, None, ScaleFactor(1.0), "guirs");
        assert_eq!(update.nodes[0].1.children().len(), 3);
    }
}
