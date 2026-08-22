//! What the interface looks like to something that cannot see it.
//!
//! A screen reader does not read the screen. It reads a tree the application
//! publishes: what each thing is, what it is called, what it currently says,
//! and where it sits. Everything guirs draws is a rounded box and a glyph, so
//! without that tree an application built on it is an unlabelled rectangle.
//!
//! The tree is built during paint, alongside the hit regions, because paint is
//! where an element's identity and its final box are both known. It is only
//! built when something is actually listening: an application with no screen
//! reader attached does none of this work, which is why it can be done every
//! frame without thinking about it.
//!
//! Roles come from an element's type name, so the widget set is described
//! correctly without every widget having to say so. A `Div::new("button")` is a
//! button to a screen reader for the same reason it is a button to a
//! stylesheet.

use guirs_core::{Bounds, GlobalElementId, Px, SharedString};

/// What a thing is, to something reading the interface aloud.
///
/// A deliberately small set. Each one has to mean something to a screen reader
/// on every platform, and a role nobody maps is worse than no role at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AccessRole {
    /// A box holding other things, with no meaning of its own.
    #[default]
    Group,
    /// The window's own contents.
    Window,
    Button,
    /// Text that is not a control.
    Label,
    TextInput,
    CheckBox,
    RadioButton,
    /// An on and off control, which reads differently from a checkbox.
    Switch,
    Slider,
    ProgressIndicator,
    ComboBox,
    List,
    ListItem,
    TabList,
    Tab,
    ScrollView,
    Image,
    Link,
    /// The draggable boundary between two panes.
    Splitter,
    /// A tree of things that open and close.
    Tree,
    /// One thing in a tree.
    TreeItem,
    /// Rows and columns of data.
    Table,
    /// One row of a table.
    Row,
    /// One cell of a table.
    Cell,
    /// The heading at the top of a column.
    ColumnHeader,
}

impl AccessRole {
    /// The role an element type implies.
    ///
    /// Widgets are named for what they are, so this is nearly a lookup. An
    /// unknown name is a group rather than a guess: describing something
    /// wrongly sends a reader somewhere that does not exist.
    pub fn for_element(element_type: &str) -> AccessRole {
        match element_type {
            "button" | "icon-button" | "text-button" | "window-button" => AccessRole::Button,
            "text" | "label" => AccessRole::Label,
            "input" | "textarea" => AccessRole::TextInput,
            "checkbox" => AccessRole::CheckBox,
            "radio" => AccessRole::RadioButton,
            "toggle" => AccessRole::Switch,
            "slider" => AccessRole::Slider,
            "progress" => AccessRole::ProgressIndicator,
            "select" => AccessRole::ComboBox,
            "tab-bar" => AccessRole::TabList,
            "tab" => AccessRole::Tab,
            "list-item" => AccessRole::ListItem,
            "scroll-view" => AccessRole::ScrollView,
            "icon" => AccessRole::Image,
            _ => AccessRole::Group,
        }
    }

    /// Whether a role describes something a person operates.
    ///
    /// A reader announces these differently from ordinary content, and skips
    /// between them when asked for "the next control".
    pub fn is_control(self) -> bool {
        matches!(
            self,
            AccessRole::Button
                | AccessRole::TextInput
                | AccessRole::CheckBox
                | AccessRole::RadioButton
                | AccessRole::Switch
                | AccessRole::Slider
                | AccessRole::ComboBox
                | AccessRole::Tab
                | AccessRole::Link
        )
    }

    /// Whether this takes its name from the text inside it.
    ///
    /// A control does: a button is a box with a word in it. A list item does
    /// too, because its content is what distinguishes it from its neighbours.
    /// A plain group does not, or the outermost container in a window would be
    /// named after everything in it.
    pub fn names_from_contents(self) -> bool {
        self.is_control() || matches!(self, AccessRole::ListItem)
    }
}

/// One thing in the interface, as described to a reader.
#[derive(Clone, Debug)]
pub struct AccessNode {
    pub id: GlobalElementId,
    pub role: AccessRole,
    pub bounds: Bounds<Px>,
    /// What it is called. Where an element does not say, this is filled in
    /// from the text inside it.
    pub label: Option<SharedString>,
    /// What it currently says, for anything holding a value.
    pub value: Option<SharedString>,
    /// The text of a label, which is also where a container gets its name.
    pub text: Option<SharedString>,
    /// The key that runs this, for a reader to offer alongside the name.
    pub shortcut: Option<SharedString>,
    pub focusable: bool,
    /// Whether pressing this does something. Taken from whether the element
    /// has a click handler rather than from its role, because a navigation
    /// item or a card is activated the same way a button is, and a reader
    /// that only offers to press the things called buttons leaves the rest of
    /// the interface unreachable.
    pub clickable: bool,
    pub disabled: bool,
    /// Whether a checkbox, radio or switch is on.
    pub checked: Option<bool>,
    /// Current, minimum and maximum, for a slider or a progress bar.
    pub numeric: Option<[f64; 3]>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

impl AccessNode {
    fn new(id: GlobalElementId, role: AccessRole, bounds: Bounds<Px>) -> Self {
        AccessNode {
            id,
            role,
            bounds,
            label: None,
            value: None,
            text: None,
            shortcut: None,
            focusable: false,
            clickable: false,
            disabled: false,
            checked: None,
            numeric: None,
            children: Vec::new(),
            parent: None,
        }
    }
}

/// The interface as a reader sees it, for one frame.
#[derive(Debug, Default)]
pub struct AccessTree {
    pub nodes: Vec<AccessNode>,
    /// Open elements, innermost last, while the tree is being built.
    stack: Vec<usize>,
    /// How many enclosing elements asked to be left out.
    ///
    /// A tick inside a checkbox, a knob inside a switch and the fill inside a
    /// progress bar are all drawn from text or shapes that mean nothing said
    /// aloud. Left in, the checkbox above would be announced as "tick
    /// Notifications", so the whole subtree is skipped rather than the one
    /// element.
    suppressed: usize,
    /// Whether anything is listening. Nothing is recorded when nothing is.
    pub active: bool,
}

impl AccessTree {
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.stack.clear();
        self.suppressed = 0;
    }

    /// Leave everything until the matching [`resume`](Self::resume) out.
    pub fn suppress(&mut self) {
        self.suppressed += 1;
    }

    pub fn resume(&mut self) {
        self.suppressed = self.suppressed.saturating_sub(1);
    }

    /// Whether anything pushed right now would be recorded.
    #[inline]
    pub fn records(&self) -> bool {
        self.active && self.suppressed == 0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Open an element. Returns its index, or `None` when nothing is listening.
    pub fn push(
        &mut self,
        id: GlobalElementId,
        role: AccessRole,
        bounds: Bounds<Px>,
    ) -> Option<usize> {
        if !self.records() {
            return None;
        }
        let index = self.nodes.len();
        let mut node = AccessNode::new(id, role, bounds);
        node.parent = self.stack.last().copied();
        if let Some(parent) = node.parent {
            self.nodes[parent].children.push(index);
        }
        self.nodes.push(node);
        self.stack.push(index);
        Some(index)
    }

    pub fn pop(&mut self) {
        if self.active {
            self.stack.pop();
        }
    }

    /// The element being described right now.
    pub fn current_mut(&mut self) -> Option<&mut AccessNode> {
        let index = *self.stack.last()?;
        self.nodes.get_mut(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut AccessNode> {
        self.nodes.get_mut(index)
    }

    /// Give every unnamed element a name from the text inside it.
    ///
    /// A button is a box containing a label, and neither knows about the
    /// other. Without this the button has no name and a reader announces it as
    /// "button", which is no use to anyone. Called once, after the frame is
    /// painted and the whole tree is known.
    pub fn resolve_labels(&mut self) {
        for index in (0..self.nodes.len()).rev() {
            if self.nodes[index].label.is_some() {
                continue;
            }
            if let Some(text) = self.nodes[index].text.clone() {
                self.nodes[index].label = Some(text);
                continue;
            }
            // Only for something that needs a name of its own. A plain group
            // is scenery: gathering its contents would name the outermost
            // container after every word in the window, and a reader walking
            // the tree would hear the whole interface read out before reaching
            // anything in it.
            if !self.nodes[index].role.names_from_contents() {
                continue;
            }
            let gathered = self.gather_text(index);
            if !gathered.is_empty() {
                self.nodes[index].label = Some(SharedString::from(gathered.as_str()));
            }
        }
    }

    /// The text of everything inside an element, in the order it was painted.
    fn gather_text(&self, index: usize) -> String {
        let mut out = String::new();
        let mut stack: Vec<usize> = self.nodes[index].children.iter().rev().copied().collect();
        while let Some(next) = stack.pop() {
            let node = &self.nodes[next];
            if let Some(text) = &node.text {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text.as_str());
            }
            // A control inside another names itself, not its container: a card
            // holding three buttons is not called by all three of their names.
            if !node.role.is_control() {
                stack.extend(node.children.iter().rev().copied());
            }
        }
        out
    }

    /// The node for an element, if it has one.
    pub fn index_of(&self, id: GlobalElementId) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> AccessTree {
        AccessTree {
            active: true,
            ..Default::default()
        }
    }

    fn at(x: f32) -> Bounds<Px> {
        Bounds::from_xywh(Px(x), Px::ZERO, Px(10.0), Px(10.0))
    }

    fn id(n: u64) -> GlobalElementId {
        GlobalElementId(n)
    }

    #[test]
    fn a_widgets_type_name_is_its_role() {
        assert_eq!(AccessRole::for_element("button"), AccessRole::Button);
        assert_eq!(AccessRole::for_element("input"), AccessRole::TextInput);
        assert_eq!(AccessRole::for_element("textarea"), AccessRole::TextInput);
        assert_eq!(AccessRole::for_element("toggle"), AccessRole::Switch);
        assert_eq!(AccessRole::for_element("select"), AccessRole::ComboBox);
        // Anything unrecognised is a plain box rather than a guess.
        assert_eq!(AccessRole::for_element("div"), AccessRole::Group);
        assert_eq!(AccessRole::for_element("wobble"), AccessRole::Group);
    }

    #[test]
    fn nothing_is_recorded_while_nothing_is_listening() {
        let mut tree = AccessTree::default();
        assert!(!tree.active);
        assert!(tree.push(id(1), AccessRole::Button, at(0.0)).is_none());
        tree.pop();
        assert!(tree.is_empty(), "an idle tree still cost something");
    }

    #[test]
    fn the_tree_follows_the_way_it_was_painted() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Group, at(0.0));
        tree.push(id(2), AccessRole::Button, at(10.0));
        tree.pop();
        tree.push(id(3), AccessRole::Button, at(20.0));
        tree.pop();
        tree.pop();

        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.nodes[0].children, vec![1, 2]);
        assert_eq!(tree.nodes[1].parent, Some(0));
        assert_eq!(tree.nodes[2].parent, Some(0));
    }

    #[test]
    fn a_button_takes_its_name_from_the_text_inside_it() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Button, at(0.0));
        tree.push(id(2), AccessRole::Label, at(2.0));
        tree.current_mut().unwrap().text = Some(SharedString::from("Save"));
        tree.pop();
        tree.pop();

        tree.resolve_labels();
        assert_eq!(tree.nodes[0].label.as_deref(), Some("Save"));
    }

    #[test]
    fn several_words_inside_a_control_read_in_order() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Button, at(0.0));
        for (n, word) in ["Send", "message"].iter().enumerate() {
            tree.push(id(10 + n as u64), AccessRole::Label, at(2.0));
            tree.current_mut().unwrap().text = Some(SharedString::from(*word));
            tree.pop();
        }
        tree.pop();

        tree.resolve_labels();
        assert_eq!(tree.nodes[0].label.as_deref(), Some("Send message"));
    }

    #[test]
    fn a_container_is_not_named_by_the_controls_inside_it() {
        // A card holding two buttons is not called "Save Cancel".
        let mut tree = tree();
        tree.push(id(1), AccessRole::Group, at(0.0));
        for (n, word) in ["Save", "Cancel"].iter().enumerate() {
            tree.push(id(10 + n as u64), AccessRole::Button, at(2.0));
            tree.push(id(20 + n as u64), AccessRole::Label, at(2.0));
            tree.current_mut().unwrap().text = Some(SharedString::from(*word));
            tree.pop();
            tree.pop();
        }
        tree.pop();

        tree.resolve_labels();
        assert!(
            tree.nodes[0].label.is_none(),
            "the card was named {:?}",
            tree.nodes[0].label
        );
        assert_eq!(tree.nodes[1].label.as_deref(), Some("Save"));
        assert_eq!(tree.nodes[3].label.as_deref(), Some("Cancel"));
    }

    #[test]
    fn a_name_an_element_gave_itself_is_not_overwritten() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Button, at(0.0));
        tree.current_mut().unwrap().label = Some(SharedString::from("Close the window"));
        tree.push(id(2), AccessRole::Label, at(2.0));
        tree.current_mut().unwrap().text = Some(SharedString::from("x"));
        tree.pop();
        tree.pop();

        tree.resolve_labels();
        assert_eq!(tree.nodes[0].label.as_deref(), Some("Close the window"));
    }

    #[test]
    fn an_element_can_be_found_by_its_identity() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Group, at(0.0));
        tree.push(id(7), AccessRole::Button, at(10.0));
        tree.pop();
        tree.pop();

        assert_eq!(tree.index_of(id(7)), Some(1));
        assert_eq!(tree.index_of(id(99)), None);
    }

    #[test]
    fn clearing_leaves_it_ready_for_the_next_frame() {
        let mut tree = tree();
        tree.push(id(1), AccessRole::Group, at(0.0));
        tree.clear();
        assert!(tree.is_empty());
        // The stack went too, or the next frame would nest under a dead node.
        tree.push(id(2), AccessRole::Button, at(0.0));
        assert_eq!(tree.nodes[0].parent, None);
    }
}
