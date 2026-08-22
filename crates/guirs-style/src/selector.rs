//! Selectors, specificity and matching.
//!
//! Matching runs against a chain of [`ElementDescriptor`] values from the root
//! down to the element being styled. Compound selectors are evaluated right to
//! left, which lets the common case (a selector whose rightmost compound does
//! not match) bail out after a single comparison.

use std::fmt;

use guirs_core::SharedString;
use smallvec::SmallVec;

/// Interaction and structural state an element can be in.
///
/// These are the pseudo classes a stylesheet can select on. They are packed
/// into a bitset because every element carries one and it is compared on every
/// cascade.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StateFlags(pub u16);

impl StateFlags {
    pub const EMPTY: StateFlags = StateFlags(0);
    /// The pointer is over this element.
    pub const HOVER: StateFlags = StateFlags(1 << 0);
    /// A pointer button is held down on this element.
    pub const ACTIVE: StateFlags = StateFlags(1 << 1);
    /// This element holds keyboard focus.
    pub const FOCUS: StateFlags = StateFlags(1 << 2);
    /// This element or one of its descendants holds keyboard focus.
    pub const FOCUS_WITHIN: StateFlags = StateFlags(1 << 3);
    /// The element does not respond to input.
    pub const DISABLED: StateFlags = StateFlags(1 << 4);
    /// A checkbox, radio or toggle is on.
    pub const CHECKED: StateFlags = StateFlags(1 << 5);
    /// A list item, tab or option is the current selection.
    pub const SELECTED: StateFlags = StateFlags(1 << 6);
    /// The element is open, as in an expanded dropdown or disclosure.
    pub const OPEN: StateFlags = StateFlags(1 << 7);
    /// Focus arrived from the keyboard rather than the pointer.
    pub const FOCUS_VISIBLE: StateFlags = StateFlags(1 << 8);
    /// Something is being dragged and would be dropped here.
    pub const DRAG_OVER: StateFlags = StateFlags(1 << 9);
    /// This is the thing being dragged.
    pub const DRAGGING: StateFlags = StateFlags(1 << 10);

    #[inline]
    pub fn contains(self, other: StateFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub fn insert(&mut self, other: StateFlags) {
        self.0 |= other.0;
    }

    #[inline]
    pub fn remove(&mut self, other: StateFlags) {
        self.0 &= !other.0;
    }

    #[inline]
    pub fn set(&mut self, other: StateFlags, on: bool) {
        if on {
            self.insert(other);
        } else {
            self.remove(other);
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for StateFlags {
    type Output = StateFlags;
    #[inline]
    fn bitor(self, rhs: StateFlags) -> StateFlags {
        StateFlags(self.0 | rhs.0)
    }
}

/// Everything matching needs to know about one element.
#[derive(Clone, Debug, Default)]
pub struct ElementDescriptor {
    /// The widget's type name, as in `button` or `select`.
    pub element_type: SharedString,
    pub id: Option<SharedString>,
    pub classes: SmallVec<[SharedString; 4]>,
    pub state: StateFlags,
    /// Zero based position among siblings.
    pub child_index: usize,
    /// Number of siblings including this one.
    pub sibling_count: usize,
    /// Whether this element has any children.
    pub has_children: bool,
    /// Whether this is the window root.
    pub is_root: bool,
}

impl ElementDescriptor {
    pub fn new(element_type: impl Into<SharedString>) -> Self {
        ElementDescriptor {
            element_type: element_type.into(),
            sibling_count: 1,
            ..Default::default()
        }
    }

    #[inline]
    pub fn has_class(&self, name: &str) -> bool {
        self.classes.iter().any(|c| c.as_str() == name)
    }
}

/// A structural or state based pseudo class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudoClass {
    Hover,
    Active,
    Focus,
    FocusWithin,
    FocusVisible,
    Disabled,
    Enabled,
    Checked,
    Selected,
    Open,
    Root,
    FirstChild,
    LastChild,
    OnlyChild,
    Empty,
    /// Something is being dragged and would be dropped here.
    DragOver,
    /// This is the thing being dragged.
    Dragging,
    /// Negation. The argument is a single compound selector.
    Not(Box<CompoundSelector>),
}

impl PseudoClass {
    pub(crate) fn parse(name: &str) -> Option<PseudoClass> {
        Some(match name {
            "hover" => PseudoClass::Hover,
            "active" => PseudoClass::Active,
            "focus" => PseudoClass::Focus,
            "focus-within" => PseudoClass::FocusWithin,
            "focus-visible" => PseudoClass::FocusVisible,
            "disabled" => PseudoClass::Disabled,
            "enabled" => PseudoClass::Enabled,
            "checked" => PseudoClass::Checked,
            "selected" => PseudoClass::Selected,
            "open" => PseudoClass::Open,
            "drag-over" => PseudoClass::DragOver,
            "dragging" => PseudoClass::Dragging,
            "root" => PseudoClass::Root,
            "first-child" => PseudoClass::FirstChild,
            "last-child" => PseudoClass::LastChild,
            "only-child" => PseudoClass::OnlyChild,
            "empty" => PseudoClass::Empty,
            _ => return None,
        })
    }

    fn matches(&self, el: &ElementDescriptor) -> bool {
        match self {
            PseudoClass::Hover => el.state.contains(StateFlags::HOVER),
            PseudoClass::Active => el.state.contains(StateFlags::ACTIVE),
            PseudoClass::Focus => el.state.contains(StateFlags::FOCUS),
            PseudoClass::FocusWithin => el.state.contains(StateFlags::FOCUS_WITHIN),
            PseudoClass::FocusVisible => el.state.contains(StateFlags::FOCUS_VISIBLE),
            PseudoClass::Disabled => el.state.contains(StateFlags::DISABLED),
            PseudoClass::Enabled => !el.state.contains(StateFlags::DISABLED),
            PseudoClass::Checked => el.state.contains(StateFlags::CHECKED),
            PseudoClass::Selected => el.state.contains(StateFlags::SELECTED),
            PseudoClass::Open => el.state.contains(StateFlags::OPEN),
            PseudoClass::DragOver => el.state.contains(StateFlags::DRAG_OVER),
            PseudoClass::Dragging => el.state.contains(StateFlags::DRAGGING),
            PseudoClass::Root => el.is_root,
            PseudoClass::FirstChild => el.child_index == 0,
            PseudoClass::LastChild => {
                el.sibling_count > 0 && el.child_index + 1 == el.sibling_count
            }
            PseudoClass::OnlyChild => el.sibling_count == 1,
            PseudoClass::Empty => !el.has_children,
            PseudoClass::Not(inner) => !inner.matches(el),
        }
    }

    /// Whether this pseudo class can change without the tree changing shape.
    ///
    /// The cascade uses this to decide whether a rule needs re-evaluating when
    /// only interaction state changed.
    pub fn is_dynamic(&self) -> bool {
        match self {
            PseudoClass::Root
            | PseudoClass::FirstChild
            | PseudoClass::LastChild
            | PseudoClass::OnlyChild
            | PseudoClass::Empty => false,
            PseudoClass::Not(inner) => inner.pseudo_classes.iter().any(|p| p.is_dynamic()),
            _ => true,
        }
    }
}

impl fmt::Display for PseudoClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            PseudoClass::Hover => "hover",
            PseudoClass::Active => "active",
            PseudoClass::Focus => "focus",
            PseudoClass::FocusWithin => "focus-within",
            PseudoClass::FocusVisible => "focus-visible",
            PseudoClass::Disabled => "disabled",
            PseudoClass::Enabled => "enabled",
            PseudoClass::Checked => "checked",
            PseudoClass::Selected => "selected",
            PseudoClass::Open => "open",
            PseudoClass::DragOver => "drag-over",
            PseudoClass::Dragging => "dragging",
            PseudoClass::Root => "root",
            PseudoClass::FirstChild => "first-child",
            PseudoClass::LastChild => "last-child",
            PseudoClass::OnlyChild => "only-child",
            PseudoClass::Empty => "empty",
            PseudoClass::Not(inner) => return write!(f, ":not({inner})"),
        };
        write!(f, ":{name}")
    }
}

/// A run of simple selectors with no combinator between them, such as
/// `button.primary:hover`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompoundSelector {
    /// `None` means the universal selector.
    pub element_type: Option<SharedString>,
    pub id: Option<SharedString>,
    pub classes: SmallVec<[SharedString; 4]>,
    pub pseudo_classes: SmallVec<[PseudoClass; 2]>,
}

impl CompoundSelector {
    pub fn matches(&self, el: &ElementDescriptor) -> bool {
        if let Some(ty) = &self.element_type {
            if ty.as_str() != el.element_type.as_str() {
                return false;
            }
        }
        if let Some(id) = &self.id {
            match &el.id {
                Some(actual) if actual.as_str() == id.as_str() => {}
                _ => return false,
            }
        }
        for class in &self.classes {
            if !el.has_class(class) {
                return false;
            }
        }
        for pseudo in &self.pseudo_classes {
            if !pseudo.matches(el) {
                return false;
            }
        }
        true
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.element_type.is_none()
            && self.id.is_none()
            && self.classes.is_empty()
            && self.pseudo_classes.is_empty()
    }
}

impl fmt::Display for CompoundSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.element_type {
            Some(t) => write!(f, "{t}")?,
            None if self.id.is_none() && self.classes.is_empty() => write!(f, "*")?,
            None => {}
        }
        if let Some(id) = &self.id {
            write!(f, "#{id}")?;
        }
        for c in &self.classes {
            write!(f, ".{c}")?;
        }
        for p in &self.pseudo_classes {
            write!(f, "{p}")?;
        }
        Ok(())
    }
}

/// The relationship between two compound selectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    /// A space: any ancestor.
    Descendant,
    /// `>`: the direct parent.
    Child,
}

/// A full selector such as `.sidebar > button.primary:hover`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    /// Compounds in source order. The last one is the subject.
    pub compounds: SmallVec<[CompoundSelector; 2]>,
    /// `combinators[i]` links `compounds[i]` to `compounds[i + 1]`.
    pub combinators: SmallVec<[Combinator; 2]>,
}

impl Selector {
    /// The subject compound, the one that describes the element being styled.
    #[inline]
    pub fn subject(&self) -> &CompoundSelector {
        self.compounds
            .last()
            .expect("a selector always has at least one compound")
    }

    /// Match against a root to leaf chain, where the last entry is the element
    /// being styled.
    pub fn matches(&self, chain: &[ElementDescriptor]) -> bool {
        let Some(subject) = chain.last() else {
            return false;
        };
        if !self.subject().matches(subject) {
            return false;
        }
        if self.compounds.len() == 1 {
            return true;
        }

        // Walk leftward through the ancestors. Descendant combinators may skip
        // any number of levels, so they backtrack; child combinators may not.
        let mut ancestors = &chain[..chain.len() - 1];
        for i in (0..self.compounds.len() - 1).rev() {
            let compound = &self.compounds[i];
            match self.combinators[i] {
                Combinator::Child => {
                    let Some(parent) = ancestors.last() else {
                        return false;
                    };
                    if !compound.matches(parent) {
                        return false;
                    }
                    ancestors = &ancestors[..ancestors.len() - 1];
                }
                Combinator::Descendant => {
                    let mut found = None;
                    for (idx, candidate) in ancestors.iter().enumerate().rev() {
                        if compound.matches(candidate) {
                            found = Some(idx);
                            break;
                        }
                    }
                    let Some(idx) = found else {
                        return false;
                    };
                    ancestors = &ancestors[..idx];
                }
            }
        }
        true
    }

    /// CSS specificity, packed so that plain integer comparison orders rules.
    pub fn specificity(&self) -> Specificity {
        let mut ids = 0u32;
        let mut classes = 0u32;
        let mut types = 0u32;
        for compound in &self.compounds {
            if compound.id.is_some() {
                ids += 1;
            }
            classes += compound.classes.len() as u32;
            for pseudo in &compound.pseudo_classes {
                match pseudo {
                    // Negation itself adds nothing; its argument counts.
                    PseudoClass::Not(inner) => {
                        if inner.id.is_some() {
                            ids += 1;
                        }
                        classes += inner.classes.len() as u32 + inner.pseudo_classes.len() as u32;
                        if inner.element_type.is_some() {
                            types += 1;
                        }
                    }
                    _ => classes += 1,
                }
            }
            if compound.element_type.is_some() {
                types += 1;
            }
        }
        Specificity::new(ids, classes, types)
    }

    /// Whether any part of this selector depends on interaction state.
    pub fn is_dynamic(&self) -> bool {
        self.compounds
            .iter()
            .any(|c| c.pseudo_classes.iter().any(|p| p.is_dynamic()))
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, compound) in self.compounds.iter().enumerate() {
            if i > 0 {
                match self.combinators[i - 1] {
                    Combinator::Descendant => write!(f, " ")?,
                    Combinator::Child => write!(f, " > ")?,
                }
            }
            write!(f, "{compound}")?;
        }
        Ok(())
    }
}

/// Packed CSS specificity. Higher sorts later, and therefore wins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Specificity(pub u32);

impl Specificity {
    /// Each component saturates at 255, which is far past anything a real
    /// stylesheet reaches and keeps the packed form a single comparison.
    pub fn new(ids: u32, classes: u32, types: u32) -> Specificity {
        Specificity((ids.min(255) << 16) | (classes.min(255) << 8) | types.min(255))
    }

    #[inline]
    pub fn ids(self) -> u32 {
        (self.0 >> 16) & 0xff
    }

    #[inline]
    pub fn classes(self) -> u32 {
        (self.0 >> 8) & 0xff
    }

    #[inline]
    pub fn types(self) -> u32 {
        self.0 & 0xff
    }
}

impl fmt::Display for Specificity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.ids(), self.classes(), self.types())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_selector_list;

    fn sel(src: &str) -> Selector {
        parse_selector_list(src).unwrap().into_iter().next().unwrap()
    }

    fn el(ty: &str) -> ElementDescriptor {
        ElementDescriptor::new(ty)
    }

    fn with_class(ty: &str, class: &str) -> ElementDescriptor {
        let mut e = el(ty);
        e.classes.push(class.into());
        e
    }

    #[test]
    fn type_selectors_match_by_name() {
        assert!(sel("button").matches(&[el("button")]));
        assert!(!sel("button").matches(&[el("select")]));
    }

    #[test]
    fn universal_selector_matches_anything() {
        assert!(sel("*").matches(&[el("anything")]));
    }

    #[test]
    fn compound_selectors_require_every_part() {
        let s = sel("button.primary");
        assert!(s.matches(&[with_class("button", "primary")]));
        assert!(!s.matches(&[el("button")]));
        assert!(!s.matches(&[with_class("select", "primary")]));
    }

    #[test]
    fn pseudo_classes_read_state_flags() {
        let mut hovered = el("button");
        hovered.state.insert(StateFlags::HOVER);
        assert!(sel("button:hover").matches(&[hovered.clone()]));
        assert!(!sel("button:hover").matches(&[el("button")]));
        assert!(sel("button:enabled").matches(&[hovered]));
    }

    #[test]
    fn descendant_combinator_skips_levels() {
        let chain = [with_class("div", "sidebar"), el("div"), el("button")];
        assert!(sel(".sidebar button").matches(&chain));
    }

    #[test]
    fn child_combinator_does_not_skip_levels() {
        let chain = [with_class("div", "sidebar"), el("div"), el("button")];
        assert!(!sel(".sidebar > button").matches(&chain));

        let direct = [with_class("div", "sidebar"), el("button")];
        assert!(sel(".sidebar > button").matches(&direct));
    }

    #[test]
    fn descendant_matching_backtracks() {
        // The first `div` encountered walking up is not the one that has the
        // class, so matching has to keep looking.
        let chain = [
            with_class("div", "outer"),
            el("div"),
            el("div"),
            el("span"),
        ];
        assert!(sel(".outer span").matches(&chain));
    }

    #[test]
    fn negation_inverts_a_compound() {
        assert!(sel("button:not(.primary)").matches(&[el("button")]));
        assert!(!sel("button:not(.primary)").matches(&[with_class("button", "primary")]));
    }

    #[test]
    fn structural_pseudo_classes_use_sibling_position() {
        let mut first = el("li");
        first.child_index = 0;
        first.sibling_count = 3;
        let mut last = el("li");
        last.child_index = 2;
        last.sibling_count = 3;

        assert!(sel("li:first-child").matches(&[first.clone()]));
        assert!(!sel("li:last-child").matches(&[first]));
        assert!(sel("li:last-child").matches(&[last]));
    }

    #[test]
    fn specificity_orders_id_over_class_over_type() {
        assert!(sel("#a").specificity() > sel(".a").specificity());
        assert!(sel(".a").specificity() > sel("a").specificity());
        assert!(sel("a.b.c").specificity() > sel("a.b").specificity());
        assert_eq!(sel("*").specificity(), Specificity::new(0, 0, 0));
    }

    #[test]
    fn specificity_counts_every_compound_in_the_chain() {
        assert_eq!(sel(".sidebar button").specificity(), Specificity::new(0, 1, 1));
    }

    #[test]
    fn dynamic_detection_ignores_structural_pseudo_classes() {
        assert!(sel("button:hover").is_dynamic());
        assert!(!sel("li:first-child").is_dynamic());
        assert!(!sel("button").is_dynamic());
    }

    #[test]
    fn selectors_round_trip_through_display() {
        for src in [
            "button",
            "*",
            "#sidebar",
            "button.primary:hover",
            ".sidebar > button",
            ".a .b .c",
        ] {
            assert_eq!(sel(src).to_string(), src);
        }
    }
}
