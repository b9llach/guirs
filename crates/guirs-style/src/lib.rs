//! The guirs styling system.
//!
//! Styles can be written two ways, and both land on the same [`Style`] struct:
//!
//! ```ignore
//! // A stylesheet, hot reloadable at runtime.
//! :root { --primary: #6c5ce7; }
//! button.primary          { background: var(--primary); border-radius: 8px; }
//! button.primary:hover    { background: lighten(var(--primary), 12%); }
//!
//! // The typed builder API, checked by the compiler.
//! Style::new().bg(rgb(0x6c5ce7)).rounded(8.0)
//! ```
//!
//! The pipeline is:
//!
//! 1. [`Stylesheet::parse`] turns source into rules, resolving `var()` up front.
//! 2. [`StyleEngine`] indexes those rules and cascades the matching ones onto an
//!    element, inline style last.
//! 3. [`Style::resolve`] flattens the result into a [`ComputedStyle`], pulling
//!    inherited properties down from the parent.
//! 4. [`StyleAnimator`] eases between successive computed styles wherever a
//!    `transition` says to.

pub mod cascade;
pub mod lexer;
pub mod parser;
pub mod selector;
pub mod style;
pub mod transition;
pub mod types;

pub use cascade::StyleEngine;
pub use lexer::{tokenize, Token, TokenKind, Unit};
pub use parser::{parse_selector_list, ParseError, Rule, Stylesheet};
pub use selector::{
    Combinator, CompoundSelector, ElementDescriptor, PseudoClass, Selector, Specificity, StateFlags,
};
pub use style::{ComputedStyle, ShadowList, Style, Styled, TransitionList};
pub use transition::{Animated, StyleAnimator};
pub use types::{
    AlignItems, AnimatableProperty, CursorStyle, Display, Easing, FlexDirection, FlexWrap,
    FontFamily, FontStyle, FontWeight, JustifyContent, LineHeight, OrderedF32, Overflow,
    Position, TextAlign, TextDecoration, TextOverflow, Transition, TransitionTarget, WhiteSpace,
};

// Re-exported so downstream crates can style without depending on guirs-core
// directly.
pub use guirs_core::{
    px, rems, BorderStyle, BoxShadow, Corners, Edges, Gradient, GradientStop, Length, Paint, Point,
    Px, Rgba, SharedString, Size,
};
