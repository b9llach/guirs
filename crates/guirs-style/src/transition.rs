//! Style transitions.
//!
//! When a rule stops matching (the pointer leaves a button, say) the computed
//! style changes instantly. A transition smooths that change by remembering the
//! value that was last shown and easing toward the new one.
//!
//! State is keyed by element identity rather than by tree position, so a
//! transition survives the element tree being rebuilt, which happens every
//! frame in a declarative framework.

use std::collections::HashMap;

use guirs_core::{GlobalElementId, Length, Px};
use smallvec::SmallVec;

use crate::style::{ComputedStyle, ShadowList};
use crate::types::AnimatableProperty;

/// One track slot per animatable property.
///
/// Derived from the property list rather than written out, because the two
/// falling out of step is an index past the end of the track array, which is
/// a panic rather than a wrong picture.
const PROPERTY_COUNT: usize = AnimatableProperty::ALL.len();

#[inline]
fn property_index(property: AnimatableProperty) -> usize {
    match property {
        AnimatableProperty::Background => 0,
        AnimatableProperty::BorderColor => 1,
        AnimatableProperty::BorderWidth => 2,
        AnimatableProperty::BorderRadius => 3,
        AnimatableProperty::BoxShadow => 4,
        AnimatableProperty::Color => 5,
        AnimatableProperty::Opacity => 6,
        AnimatableProperty::Width => 7,
        AnimatableProperty::Height => 8,
        AnimatableProperty::Padding => 9,
        AnimatableProperty::Margin => 10,
        AnimatableProperty::FontSize => 11,
        AnimatableProperty::LetterSpacing => 12,
        AnimatableProperty::Inset => 13,
        AnimatableProperty::Transform => 14,
    }
}

/// The outcome of advancing one element's transitions.
#[derive(Clone, Debug)]
pub struct Animated {
    /// The style to paint this frame.
    pub style: ComputedStyle,
    /// Whether anything is still in motion. While true the window needs to keep
    /// drawing frames.
    pub active: bool,
}

#[derive(Debug)]
struct Entry {
    /// The value each running track started from.
    from: ComputedStyle,
    /// The most recent target, used to notice a retarget.
    target: ComputedStyle,
    /// What was actually painted last frame, when it differed from the target.
    ///
    /// `None` means nothing was in motion and the target was painted as is.
    /// Most elements are idle most of the time, and a computed style is not
    /// free to clone, so not storing a copy of something we already have is
    /// worth the extra branch.
    displayed: Option<ComputedStyle>,
    start: [f64; PROPERTY_COUNT],
    last_frame: u64,
}

/// Per element transition state.
#[derive(Debug, Default)]
pub struct StyleAnimator {
    entries: HashMap<GlobalElementId, Entry>,
    frame: u64,
}

impl StyleAnimator {
    pub fn new() -> Self {
        StyleAnimator::default()
    }

    /// Call once at the start of each frame.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Drop state for elements that were not drawn this frame.
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        self.entries.retain(|_, entry| entry.last_frame == frame);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn tracked_elements(&self) -> usize {
        self.entries.len()
    }

    /// Advance `id` toward `target` and return the style to paint.
    ///
    /// `now` is a monotonic time in seconds.
    pub fn animate(&mut self, id: GlobalElementId, target: ComputedStyle, now: f64) -> Animated {
        if !target.has_transitions() {
            // Nothing to interpolate. Drop any state so an element that later
            // gains a transition does not animate from a stale value.
            self.entries.remove(&id);
            return Animated {
                style: target,
                active: false,
            };
        }

        let frame = self.frame;
        let entry = self.entries.entry(id).or_insert_with(|| Entry {
            from: target.clone(),
            target: target.clone(),
            displayed: None,
            start: [now; PROPERTY_COUNT],
            last_frame: frame,
        });
        entry.last_frame = frame;

        // Restart any track whose destination moved, starting from whatever
        // was actually on screen rather than from the old target.
        for property in AnimatableProperty::ALL {
            if property_differs(&entry.target, &target, property) {
                let shown = entry.displayed.as_ref().unwrap_or(&entry.target);
                copy_property(&mut entry.from, shown, property);
                entry.start[property_index(property)] = now;
            }
        }
        let mut out = target.clone();
        let mut active = false;

        for property in AnimatableProperty::ALL {
            let Some(transition) = target.transition_for(property) else {
                continue;
            };
            if !property_differs(&entry.from, &target, property) {
                continue;
            }

            let started = entry.start[property_index(property)];
            let duration = (transition.duration_ms.0 as f64) / 1000.0;
            let delay = (transition.delay_ms.0 as f64) / 1000.0;
            let elapsed = now - started - delay;

            if elapsed < 0.0 {
                // Still inside the delay: hold the starting value.
                copy_property(&mut out, &entry.from, property);
                active = true;
                continue;
            }
            if duration <= 0.0 || elapsed >= duration {
                continue;
            }

            let progress = transition.easing.apply((elapsed / duration) as f32);
            lerp_property(&mut out, &entry.from, &target, property, progress);
            active = true;
        }

        // `target` is not needed past this point, so it moves into the entry
        // rather than being cloned again. When nothing is in motion the painted
        // style is the target, so there is nothing extra to remember.
        entry.target = target;
        entry.displayed = if active { Some(out.clone()) } else { None };
        Animated {
            style: out,
            active,
        }
    }
}

// ---------------------------------------------------------------------------
// Per property access
// ---------------------------------------------------------------------------

fn property_differs(a: &ComputedStyle, b: &ComputedStyle, property: AnimatableProperty) -> bool {
    match property {
        AnimatableProperty::Background => a.background != b.background,
        AnimatableProperty::BorderColor => a.border_color != b.border_color,
        AnimatableProperty::BorderWidth => a.border_widths != b.border_widths,
        AnimatableProperty::BorderRadius => a.border_radius != b.border_radius,
        AnimatableProperty::BoxShadow => a.box_shadows != b.box_shadows,
        AnimatableProperty::Color => a.color != b.color,
        AnimatableProperty::Opacity => a.opacity != b.opacity,
        AnimatableProperty::Width => a.size.width != b.size.width,
        AnimatableProperty::Height => a.size.height != b.size.height,
        AnimatableProperty::Padding => a.padding != b.padding,
        AnimatableProperty::Margin => a.margin != b.margin,
        AnimatableProperty::FontSize => a.font_size != b.font_size,
        AnimatableProperty::LetterSpacing => a.letter_spacing != b.letter_spacing,
        AnimatableProperty::Transform => a.transform != b.transform,
        AnimatableProperty::Inset => a.inset != b.inset,
    }
}

fn copy_property(dst: &mut ComputedStyle, src: &ComputedStyle, property: AnimatableProperty) {
    match property {
        AnimatableProperty::Background => dst.background = src.background.clone(),
        AnimatableProperty::BorderColor => dst.border_color = src.border_color,
        AnimatableProperty::BorderWidth => dst.border_widths = src.border_widths,
        AnimatableProperty::BorderRadius => dst.border_radius = src.border_radius,
        AnimatableProperty::BoxShadow => dst.box_shadows = src.box_shadows.clone(),
        AnimatableProperty::Color => dst.color = src.color,
        AnimatableProperty::Opacity => dst.opacity = src.opacity,
        AnimatableProperty::Width => dst.size.width = src.size.width,
        AnimatableProperty::Height => dst.size.height = src.size.height,
        AnimatableProperty::Padding => dst.padding = src.padding,
        AnimatableProperty::Margin => dst.margin = src.margin,
        AnimatableProperty::Transform => dst.transform = src.transform,
        AnimatableProperty::FontSize => dst.font_size = src.font_size,
        AnimatableProperty::LetterSpacing => dst.letter_spacing = src.letter_spacing,
        AnimatableProperty::Inset => dst.inset = src.inset,
    }
}

fn lerp_property(
    out: &mut ComputedStyle,
    from: &ComputedStyle,
    to: &ComputedStyle,
    property: AnimatableProperty,
    t: f32,
) {
    match property {
        AnimatableProperty::Background => out.background = from.background.lerp(&to.background, t),
        // Interpolated in its decomposed form, so a half turn passes through a
        // quarter turn rather than through a shape squashed flat.
        AnimatableProperty::Transform => out.transform = from.transform.lerp(&to.transform, t),
        AnimatableProperty::BorderColor => {
            out.border_color = from.border_color.lerp(to.border_color, t)
        }
        AnimatableProperty::BorderWidth => {
            out.border_widths.top = lerp_px(from.border_widths.top, to.border_widths.top, t);
            out.border_widths.right = lerp_px(from.border_widths.right, to.border_widths.right, t);
            out.border_widths.bottom =
                lerp_px(from.border_widths.bottom, to.border_widths.bottom, t);
            out.border_widths.left = lerp_px(from.border_widths.left, to.border_widths.left, t);
        }
        AnimatableProperty::BorderRadius => {
            out.border_radius.top_left =
                lerp_px(from.border_radius.top_left, to.border_radius.top_left, t);
            out.border_radius.top_right =
                lerp_px(from.border_radius.top_right, to.border_radius.top_right, t);
            out.border_radius.bottom_right = lerp_px(
                from.border_radius.bottom_right,
                to.border_radius.bottom_right,
                t,
            );
            out.border_radius.bottom_left = lerp_px(
                from.border_radius.bottom_left,
                to.border_radius.bottom_left,
                t,
            );
        }
        AnimatableProperty::BoxShadow => {
            out.box_shadows = lerp_shadows(&from.box_shadows, &to.box_shadows, t)
        }
        AnimatableProperty::Color => out.color = from.color.lerp(to.color, t),
        AnimatableProperty::Opacity => {
            out.opacity = from.opacity + (to.opacity - from.opacity) * t
        }
        AnimatableProperty::Width => {
            out.size.width = lerp_length(from.size.width, to.size.width, t)
        }
        AnimatableProperty::Height => {
            out.size.height = lerp_length(from.size.height, to.size.height, t)
        }
        AnimatableProperty::Padding => {
            out.padding.top = lerp_length(from.padding.top, to.padding.top, t);
            out.padding.right = lerp_length(from.padding.right, to.padding.right, t);
            out.padding.bottom = lerp_length(from.padding.bottom, to.padding.bottom, t);
            out.padding.left = lerp_length(from.padding.left, to.padding.left, t);
        }
        AnimatableProperty::Margin => {
            out.margin.top = lerp_length(from.margin.top, to.margin.top, t);
            out.margin.right = lerp_length(from.margin.right, to.margin.right, t);
            out.margin.bottom = lerp_length(from.margin.bottom, to.margin.bottom, t);
            out.margin.left = lerp_length(from.margin.left, to.margin.left, t);
        }
        AnimatableProperty::FontSize => out.font_size = lerp_px(from.font_size, to.font_size, t),
        AnimatableProperty::LetterSpacing => {
            out.letter_spacing = lerp_px(from.letter_spacing, to.letter_spacing, t)
        }
        AnimatableProperty::Inset => {
            out.inset.top = lerp_length(from.inset.top, to.inset.top, t);
            out.inset.right = lerp_length(from.inset.right, to.inset.right, t);
            out.inset.bottom = lerp_length(from.inset.bottom, to.inset.bottom, t);
            out.inset.left = lerp_length(from.inset.left, to.inset.left, t);
        }
    }
}

#[inline]
fn lerp_px(a: Px, b: Px, t: f32) -> Px {
    a + (b - a) * t
}

/// Interpolate two lengths where that is meaningful.
///
/// Mixing units has no single correct answer without knowing the containing
/// block, which is not available here, so those cases swap at the midpoint.
fn lerp_length(a: Length, b: Length, t: f32) -> Length {
    match (a, b) {
        (Length::Px(x), Length::Px(y)) => Length::Px(lerp_px(x, y, t)),
        (Length::Percent(x), Length::Percent(y)) => Length::Percent(x + (y - x) * t),
        (Length::Fraction(x), Length::Fraction(y)) => Length::Fraction(x + (y - x) * t),
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

fn lerp_shadows(a: &ShadowList, b: &ShadowList, t: f32) -> ShadowList {
    if a.len() == b.len() {
        return a.iter().zip(b.iter()).map(|(x, y)| x.lerp(y, t)).collect();
    }
    // Different shadow counts have no natural correspondence. Fade the shorter
    // list's missing entries in and out by treating them as transparent.
    let mut out: ShadowList = SmallVec::new();
    let count = a.len().max(b.len());
    for i in 0..count {
        match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) => out.push(x.lerp(y, t)),
            (Some(x), None) => {
                let mut faded = *x;
                faded.color = x.color.with_alpha(x.color.a * (1.0 - t));
                out.push(faded);
            }
            (None, Some(y)) => {
                let mut faded = *y;
                faded.color = y.color.with_alpha(y.color.a * t);
                out.push(faded);
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_property_has_its_own_track_slot() {
        use super::{property_index, AnimatableProperty, PROPERTY_COUNT};
        let mut seen = [false; PROPERTY_COUNT];
        for property in AnimatableProperty::ALL {
            let index = property_index(property);
            assert!(
                index < PROPERTY_COUNT,
                "{property:?} indexes past the track array"
            );
            assert!(!seen[index], "{property:?} shares a slot with another");
            seen[index] = true;
        }
        assert!(seen.iter().all(|used| *used));
    }

    use super::*;
    use crate::style::{Style, Styled};
    use crate::types::{AnimatableProperty, TransitionTarget};
    use guirs_core::{Paint, Rgba};

    fn id(n: u64) -> GlobalElementId {
        GlobalElementId(n)
    }

    fn styled(color: u32) -> ComputedStyle {
        Style::new()
            .bg(Rgba::hex(color))
            .transition_all(100.0)
            .resolve(&ComputedStyle::default(), Px(16.0))
    }

    #[test]
    fn a_transform_animates_through_its_decomposed_form() {
        use guirs_core::Transform2D;

        let mut animator = StyleAnimator::new();
        let at_rest = Style::new()
            .transition_all(100.0)
            .resolve(&ComputedStyle::default(), Px(16.0));
        let turned = Style::new()
            .rotate(180.0)
            .transition_all(100.0)
            .resolve(&ComputedStyle::default(), Px(16.0));

        animator.begin_frame();
        animator.animate(id(9), at_rest, 0.0);
        animator.begin_frame();
        animator.animate(id(9), turned.clone(), 0.0);

        animator.begin_frame();
        let half = animator.animate(id(9), turned.clone(), 0.05);
        assert!(half.active);
        // Partway through a half turn, and turning rather than jumping. The
        // exact angle is the easing curve's business, not this test's.
        let angle = half.style.transform.rotate;
        assert!(angle > 1.0 && angle < 179.0, "angle was {angle}");
        // Nothing is squashed on the way, which interpolating the two matrices
        // instead would do.
        assert_eq!(half.style.transform.scale_x, 1.0);
        assert_eq!(half.style.transform.scale_y, 1.0);

        animator.begin_frame();
        let done = animator.animate(id(9), turned, 1.0);
        assert!(!done.active);
        assert_eq!(done.style.transform, Transform2D::rotate(180.0));
    }

    #[test]
    fn a_style_with_no_transitions_passes_straight_through() {
        let mut animator = StyleAnimator::new();
        let plain = Style::new()
            .bg(Rgba::BLACK)
            .resolve(&ComputedStyle::default(), Px(16.0));
        let out = animator.animate(id(1), plain.clone(), 0.0);
        assert!(!out.active);
        assert_eq!(out.style.background, plain.background);
        assert_eq!(animator.tracked_elements(), 0);
    }

    #[test]
    fn the_first_frame_shows_the_target_exactly() {
        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        let out = animator.animate(id(1), styled(0xff0000), 0.0);
        assert!(!out.active);
        assert_eq!(out.style.background, Paint::Solid(Rgba::hex(0xff0000)));
    }

    fn solid(animated: &Animated) -> Rgba {
        match animated.style.background {
            Paint::Solid(c) => c,
            ref other => panic!("expected a solid background, got {other:?}"),
        }
    }

    #[test]
    fn retargeting_starts_the_clock_without_jumping() {
        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), styled(0x000000), 0.0);

        // The frame that changes the target is progress zero, so it still
        // paints the old value. Jumping here is the bug this guards against.
        animator.begin_frame();
        let at_start = animator.animate(id(1), styled(0xffffff), 0.0);
        assert!(at_start.active);
        assert_eq!(solid(&at_start), Rgba::BLACK);
    }

    #[test]
    fn a_changed_target_interpolates_over_the_duration() {
        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), styled(0x000000), 0.0);
        animator.begin_frame();
        animator.animate(id(1), styled(0xffffff), 0.0);

        animator.begin_frame();
        let mid = animator.animate(id(1), styled(0xffffff), 0.05);
        assert!(mid.active);
        let c = solid(&mid);
        assert!(c.r > 0.05 && c.r < 0.995, "midpoint was {c:?}");

        animator.begin_frame();
        let done = animator.animate(id(1), styled(0xffffff), 0.2);
        assert!(!done.active);
        assert_eq!(solid(&done), Rgba::WHITE);
    }

    #[test]
    fn interrupting_a_transition_resumes_from_what_was_shown() {
        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), styled(0x000000), 0.0);
        animator.begin_frame();
        animator.animate(id(1), styled(0xffffff), 0.0);

        animator.begin_frame();
        let partway = animator.animate(id(1), styled(0xffffff), 0.05);
        let shown = solid(&partway);
        assert!(shown.r > 0.05 && shown.r < 0.995, "not midway: {shown:?}");

        // Reverse toward black. The reversal frame must show exactly what was
        // already on screen rather than snapping back to the old endpoint.
        animator.begin_frame();
        let reversed = animator.animate(id(1), styled(0x000000), 0.05);
        let resumed = solid(&reversed);
        assert!(
            (resumed.r - shown.r).abs() < 0.001,
            "reversal jumped from {shown:?} to {resumed:?}"
        );

        // And from there it heads back down, not up.
        animator.begin_frame();
        let later = animator.animate(id(1), styled(0x000000), 0.08);
        assert!(solid(&later).r < shown.r);
    }

    #[test]
    fn a_delay_holds_the_starting_value() {
        let with_delay = |color: u32| {
            let mut style = Style::new().bg(Rgba::hex(color));
            style.transitions = Some(smallvec::smallvec![crate::types::Transition {
                target: TransitionTarget::One(AnimatableProperty::Background),
                duration_ms: crate::types::OrderedF32(100.0),
                delay_ms: crate::types::OrderedF32(50.0),
                easing: crate::types::Easing::Linear,
            }]);
            style.resolve(&ComputedStyle::default(), Px(16.0))
        };

        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), with_delay(0x000000), 0.0);
        animator.begin_frame();
        animator.animate(id(1), with_delay(0xffffff), 0.0);

        // Inside the delay window nothing has moved yet, but a frame is still
        // owed so the animation can actually begin.
        animator.begin_frame();
        let during = animator.animate(id(1), with_delay(0xffffff), 0.02);
        assert!(during.active);
        assert_eq!(solid(&during), Rgba::BLACK);

        // Past the delay it is moving.
        animator.begin_frame();
        let after = animator.animate(id(1), with_delay(0xffffff), 0.1);
        assert!(after.active);
        assert!(solid(&after).r > 0.05);
    }

    #[test]
    fn only_the_named_property_transitions() {
        let build = |bg: u32, fg: u32| {
            Style::new()
                .bg(Rgba::hex(bg))
                .text_color(Rgba::hex(fg))
                .transition_property(AnimatableProperty::Background, 100.0)
                .resolve(&ComputedStyle::default(), Px(16.0))
        };

        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), build(0x000000, 0x000000), 0.0);
        animator.begin_frame();
        animator.animate(id(1), build(0xffffff, 0xffffff), 0.0);

        animator.begin_frame();
        let out = animator.animate(id(1), build(0xffffff, 0xffffff), 0.05);
        // The text color jumps, because no transition covers it.
        assert_eq!(out.style.color, Rgba::WHITE);
        // The background is still on its way.
        assert!(solid(&out).r < 0.995);
    }

    #[test]
    fn stale_entries_are_dropped_at_the_end_of_a_frame() {
        let mut animator = StyleAnimator::new();
        animator.begin_frame();
        animator.animate(id(1), styled(0x000000), 0.0);
        animator.animate(id(2), styled(0x000000), 0.0);
        animator.end_frame();
        assert_eq!(animator.tracked_elements(), 2);

        animator.begin_frame();
        animator.animate(id(1), styled(0x000000), 0.1);
        animator.end_frame();
        assert_eq!(animator.tracked_elements(), 1);
    }

    #[test]
    fn lengths_with_mixed_units_swap_rather_than_blend() {
        assert_eq!(
            lerp_length(Length::Px(Px(0.0)), Length::Percent(1.0), 0.25),
            Length::Px(Px(0.0))
        );
        assert_eq!(
            lerp_length(Length::Px(Px(0.0)), Length::Percent(1.0), 0.75),
            Length::Percent(1.0)
        );
        assert_eq!(
            lerp_length(Length::Px(Px(0.0)), Length::Px(Px(10.0)), 0.5),
            Length::Px(Px(5.0))
        );
    }
}
