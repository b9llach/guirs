//! Matching rules against elements and cascading them into a style.
//!
//! Testing every rule against every element is quadratic and shows up
//! immediately on an IDE sized tree. Instead each rule is filed under the most
//! selective simple selector in its subject compound, so an element only ever
//! considers rules that stand a chance of matching it.

use std::collections::HashMap;

use guirs_core::{Px, Rgba, SharedString};
use smallvec::SmallVec;

use crate::parser::{ParseError, Rule, Stylesheet};
use crate::selector::{ElementDescriptor, Specificity};
use crate::style::{ComputedStyle, Style};
use guirs_core::Length;

/// A stylesheet prepared for matching.
#[derive(Debug, Default)]
pub struct StyleEngine {
    sheet: Stylesheet,
    index: RuleIndex,
}

#[derive(Debug, Default)]
struct RuleIndex {
    by_id: HashMap<SharedString, Vec<u32>>,
    by_class: HashMap<SharedString, Vec<u32>>,
    by_type: HashMap<SharedString, Vec<u32>>,
    /// Rules whose subject names neither a type, a class nor an id.
    universal: Vec<u32>,
}

impl StyleEngine {
    pub fn new(sheet: Stylesheet) -> Self {
        let index = RuleIndex::build(&sheet.rules);
        StyleEngine { sheet, index }
    }

    /// Parse and prepare in one step.
    pub fn from_source(source: &str) -> Self {
        StyleEngine::new(Stylesheet::parse(source))
    }

    /// Replace the stylesheet, as a hot reload does.
    pub fn set_stylesheet(&mut self, sheet: Stylesheet) {
        self.index = RuleIndex::build(&sheet.rules);
        self.sheet = sheet;
    }

    pub fn stylesheet(&self) -> &Stylesheet {
        &self.sheet
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.sheet.errors
    }

    pub fn rule_count(&self) -> usize {
        self.sheet.rules.len()
    }

    /// A design token declared as a `:root` custom property.
    pub fn color_token(&self, name: &str) -> Option<Rgba> {
        self.sheet.color_variable(name)
    }

    /// A length token declared as a `:root` custom property.
    pub fn length_token(&self, name: &str) -> Option<Length> {
        self.sheet.length_variable(name)
    }

    /// A duration token declared as a `:root` custom property, in
    /// milliseconds. `none` reads as zero.
    pub fn duration_token(&self, name: &str) -> Option<f32> {
        self.sheet.duration_variable(name)
    }

    /// Merge every matching rule, lowest specificity first, then the element's
    /// own inline style on top.
    pub fn cascade(&self, chain: &[ElementDescriptor], inline: &Style) -> Style {
        let mut matched = self.matching_rules(chain);
        // Ascending, so the winner is applied last. Source order breaks ties,
        // which is what makes a later rule in the file override an earlier one.
        matched.sort_unstable_by_key(|(specificity, order, _)| (*specificity, *order));

        let mut style = Style::new();
        for (_, _, rule_index) in &matched {
            style.merge_from(&self.sheet.rules[*rule_index as usize].style);
        }
        style.merge_from(inline);
        style
    }

    /// Cascade and then resolve against the parent's computed style.
    pub fn compute(
        &self,
        chain: &[ElementDescriptor],
        inline: &Style,
        parent: &ComputedStyle,
        root_font_size: Px,
    ) -> ComputedStyle {
        self.cascade(chain, inline)
            .resolve(parent, root_font_size)
    }

    fn matching_rules(&self, chain: &[ElementDescriptor]) -> SmallVec<[(Specificity, u32, u32); 8]> {
        let Some(subject) = chain.last() else {
            return SmallVec::new();
        };

        let mut candidates: SmallVec<[u32; 16]> = SmallVec::new();
        candidates.extend_from_slice(&self.index.universal);
        if let Some(id) = &subject.id {
            if let Some(bucket) = self.index.by_id.get(id.as_str()) {
                candidates.extend_from_slice(bucket);
            }
        }
        for class in &subject.classes {
            if let Some(bucket) = self.index.by_class.get(class.as_str()) {
                candidates.extend_from_slice(bucket);
            }
        }
        if let Some(bucket) = self.index.by_type.get(subject.element_type.as_str()) {
            candidates.extend_from_slice(bucket);
        }

        // A rule reachable through several buckets must only be applied once.
        candidates.sort_unstable();
        candidates.dedup();

        let mut matched = SmallVec::new();
        for rule_index in candidates {
            let rule = &self.sheet.rules[rule_index as usize];
            if let Some(specificity) = rule.match_specificity(chain) {
                matched.push((specificity, rule.order, rule_index));
            }
        }
        matched
    }
}

impl RuleIndex {
    fn build(rules: &[Rule]) -> RuleIndex {
        let mut index = RuleIndex::default();
        for (i, rule) in rules.iter().enumerate() {
            let i = i as u32;
            for selector in &rule.selectors {
                let subject = selector.subject();
                // File under the most selective part available. An id narrows
                // the candidate set far more than a type name does.
                if let Some(id) = &subject.id {
                    index.by_id.entry(id.clone()).or_default().push(i);
                } else if let Some(class) = subject.classes.first() {
                    index.by_class.entry(class.clone()).or_default().push(i);
                } else if let Some(ty) = &subject.element_type {
                    index.by_type.entry(ty.clone()).or_default().push(i);
                } else {
                    index.universal.push(i);
                }
            }
        }
        for bucket in index
            .by_id
            .values_mut()
            .chain(index.by_class.values_mut())
            .chain(index.by_type.values_mut())
            .chain(std::iter::once(&mut index.universal))
        {
            bucket.sort_unstable();
            bucket.dedup();
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::StateFlags;
    use crate::style::Styled;
    use guirs_core::Paint;

    fn chain(descriptors: Vec<ElementDescriptor>) -> Vec<ElementDescriptor> {
        descriptors
    }

    fn el(ty: &str) -> ElementDescriptor {
        ElementDescriptor::new(ty)
    }

    fn classed(ty: &str, class: &str) -> ElementDescriptor {
        let mut e = el(ty);
        e.classes.push(class.into());
        e
    }

    fn transform_of(source: &str) -> guirs_core::Transform2D {
        let engine = StyleEngine::from_source(source);
        engine
            .cascade(&chain(vec![el("card")]), &Style::new())
            .transform
            .expect("no transform parsed")
    }

    #[test]
    fn transform_functions_parse_in_any_order() {
        let forwards = transform_of("card { transform: translate(10px, 4px) rotate(45deg) scale(2); }");
        assert_eq!(forwards.translate_x, Px(10.0));
        assert_eq!(forwards.translate_y, Px(4.0));
        assert_eq!(forwards.rotate, 45.0);
        assert_eq!(forwards.scale_x, 2.0);
        assert_eq!(forwards.scale_y, 2.0);

        // Decomposed rather than multiplied out, so the order of the functions
        // does not change the result.
        let backwards = transform_of("card { transform: scale(2) rotate(45deg) translate(10px, 4px); }");
        assert_eq!(forwards, backwards);
    }

    #[test]
    fn the_single_axis_transform_functions_work() {
        let moved = transform_of("card { transform: translateX(12px) translateY(-3px); }");
        assert_eq!(moved.translate_x, Px(12.0));
        assert_eq!(moved.translate_y, Px(-3.0));

        let stretched = transform_of("card { transform: scaleX(0.5) scaleY(3); }");
        assert_eq!(stretched.scale_x, 0.5);
        assert_eq!(stretched.scale_y, 3.0);

        let two = transform_of("card { transform: scale(2, 0.5); }");
        assert_eq!((two.scale_x, two.scale_y), (2.0, 0.5));
    }

    #[test]
    fn a_transform_of_none_is_the_identity() {
        assert!(transform_of("card { transform: none; }").is_identity());
    }

    #[test]
    fn the_transform_origin_survives_a_later_transform() {
        // The origin is its own property, so setting the transform afterwards
        // must not reset it.
        let transform = transform_of(
            "card { transform-origin: 0% 100%; transform: scale(2); }",
        );
        assert_eq!(transform.origin_x, 0.0);
        assert_eq!(transform.origin_y, 1.0);
        assert_eq!(transform.scale_x, 2.0);
    }

    #[test]
    fn the_transform_origin_takes_keywords_and_percentages() {
        let named = transform_of("card { transform: scale(2); transform-origin: right bottom; }");
        assert_eq!((named.origin_x, named.origin_y), (1.0, 1.0));

        let single = transform_of("card { transform: scale(2); transform-origin: 25%; }");
        assert_eq!((single.origin_x, single.origin_y), (0.25, 0.25));

        let centered = transform_of("card { transform: scale(2); transform-origin: center; }");
        assert_eq!((centered.origin_x, centered.origin_y), (0.5, 0.5));
    }

    #[test]
    fn nonsense_in_a_transform_is_ignored_rather_than_half_applied() {
        let engine = StyleEngine::from_source("card { transform: wobble(3); }");
        let style = engine.cascade(&chain(vec![el("card")]), &Style::new());
        assert!(style.transform.is_none());
    }

    #[test]
    fn a_matching_rule_applies() {
        let engine = StyleEngine::from_source("button { background: #ff0000; }");
        let style = engine.cascade(&chain(vec![el("button")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0xff0000))));
    }

    #[test]
    fn a_non_matching_rule_does_not() {
        let engine = StyleEngine::from_source("button { background: #ff0000; }");
        let style = engine.cascade(&chain(vec![el("select")]), &Style::new());
        assert_eq!(style.background, None);
    }

    #[test]
    fn higher_specificity_wins_regardless_of_order() {
        let engine = StyleEngine::from_source(
            "button.primary { background: #00ff00; } button { background: #ff0000; }",
        );
        let style = engine.cascade(&chain(vec![classed("button", "primary")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0x00ff00))));
    }

    #[test]
    fn source_order_breaks_specificity_ties() {
        let engine =
            StyleEngine::from_source("button { background: #ff0000; } button { background: #0000ff; }");
        let style = engine.cascade(&chain(vec![el("button")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0x0000ff))));
    }

    #[test]
    fn inline_style_beats_every_rule() {
        let engine = StyleEngine::from_source("#special { background: #ff0000; }");
        let mut subject = el("button");
        subject.id = Some("special".into());
        let inline = Style::new().bg(Rgba::hex(0x123456));
        let style = engine.cascade(&chain(vec![subject]), &inline);
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0x123456))));
    }

    #[test]
    fn state_rules_apply_only_in_that_state() {
        let engine = StyleEngine::from_source(
            "button { background: #111111; } button:hover { background: #222222; }",
        );

        let resting = engine.cascade(&chain(vec![el("button")]), &Style::new());
        assert_eq!(resting.background, Some(Paint::Solid(Rgba::hex(0x111111))));

        let mut hovered = el("button");
        hovered.state.insert(StateFlags::HOVER);
        let active = engine.cascade(&chain(vec![hovered]), &Style::new());
        assert_eq!(active.background, Some(Paint::Solid(Rgba::hex(0x222222))));
    }

    #[test]
    fn descendant_rules_see_the_whole_chain() {
        let engine = StyleEngine::from_source(".panel button { color: #abcdef; }");
        let inside = chain(vec![classed("div", "panel"), el("div"), el("button")]);
        let outside = chain(vec![el("div"), el("button")]);

        assert_eq!(
            engine.cascade(&inside, &Style::new()).color,
            Some(Rgba::hex(0xabcdef))
        );
        assert_eq!(engine.cascade(&outside, &Style::new()).color, None);
    }

    #[test]
    fn a_rule_matched_by_two_selectors_applies_once_at_its_best_specificity() {
        let engine = StyleEngine::from_source(
            "button, button.primary { background: #ff0000; }
             .primary { background: #00ff00; }",
        );
        // The first rule matches through `button.primary`, specificity (0,1,1),
        // which beats `.primary` at (0,1,0).
        let style = engine.cascade(&chain(vec![classed("button", "primary")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0xff0000))));
    }

    #[test]
    fn variables_resolve_before_the_cascade_runs() {
        let engine = StyleEngine::from_source(
            ":root { --brand: #6c5ce7; --pad: 12px; }
             button { background: var(--brand); padding: var(--pad); }",
        );
        let style = engine.cascade(&chain(vec![el("button")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0x6c5ce7))));
        assert_eq!(style.padding_top, Some(Length::Px(Px(12.0))));
        assert!(engine.errors().is_empty(), "{:?}", engine.errors());
    }

    #[test]
    fn variables_can_reference_other_variables() {
        let engine = StyleEngine::from_source(
            ":root { --base: #101010; --surface: var(--base); }
             div { background: var(--surface); }",
        );
        let style = engine.cascade(&chain(vec![el("div")]), &Style::new());
        assert_eq!(style.background, Some(Paint::Solid(Rgba::hex(0x101010))));
    }

    #[test]
    fn a_missing_variable_falls_back() {
        let engine = StyleEngine::from_source("div { color: var(--nope, #00ff00); }");
        let style = engine.cascade(&chain(vec![el("div")]), &Style::new());
        assert_eq!(style.color, Some(Rgba::hex(0x00ff00)));
        assert!(engine.errors().is_empty(), "{:?}", engine.errors());
    }

    #[test]
    fn color_functions_compose_with_variables() {
        let engine = StyleEngine::from_source(
            ":root { --surface: #202020; }
             div:hover { background: lighten(var(--surface), 50%); }",
        );
        let mut hovered = el("div");
        hovered.state.insert(StateFlags::HOVER);
        let style = engine.cascade(&chain(vec![hovered]), &Style::new());
        match style.background {
            Some(Paint::Solid(c)) => {
                assert!(c.luminance() > Rgba::hex(0x202020).luminance());
            }
            other => panic!("expected a lightened solid, got {other:?}"),
        }
    }

    #[test]
    fn the_index_does_not_change_which_rules_match() {
        // A rule filed under `.b` must still match an element that also carries
        // other classes, and one filed under a type must survive an id lookup.
        let engine = StyleEngine::from_source(
            ".b { color: #010101; } span { color: #020202; } #x { color: #030303; }",
        );
        let mut subject = ElementDescriptor::new("span");
        subject.id = Some("x".into());
        subject.classes.push("a".into());
        subject.classes.push("b".into());

        let style = engine.cascade(&chain(vec![subject]), &Style::new());
        assert_eq!(style.color, Some(Rgba::hex(0x030303)));
    }

    #[test]
    fn tokens_are_readable_from_the_engine() {
        let engine = StyleEngine::from_source(":root { --primary: #6c5ce7; --radius: 8px; }");
        assert_eq!(engine.color_token("primary"), Some(Rgba::hex(0x6c5ce7)));
        assert_eq!(engine.length_token("radius"), Some(Length::Px(Px(8.0))));
        assert_eq!(engine.color_token("missing"), None);
    }
}
