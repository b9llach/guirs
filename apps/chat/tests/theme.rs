//! The theme is part of the application, so it gets checked like code.

use guirs::Stylesheet;

const THEME: &str = include_str!("../assets/theme.gss");

#[test]
fn the_theme_parses_without_errors() {
    let sheet = Stylesheet::parse(THEME);
    assert!(
        sheet.errors.is_empty(),
        "theme.gss has {} problems:\n{}",
        sheet.errors.len(),
        sheet
            .errors
            .iter()
            .map(|e| format!("  {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Both the transcript and the composer are capped at a readable width, and
/// both sit in a column that is wider than that cap. A cap with nothing
/// centring it sits at the left, so the two drift apart as the window widens
/// and the conversation stops looking like one column.
#[test]
fn everything_capped_at_a_reading_width_is_centred() {
    let sheet = Stylesheet::parse(THEME);
    for class in ["thread", "composer"] {
        let centred = sheet.rules.iter().any(|rule| {
            rule.selectors
                .iter()
                .any(|selector| format!("{selector:?}").contains(class))
                && rule.style.align_items == Some(guirs::AlignItems::Center)
        });
        assert!(centred, ".{class} does not centre what it holds");
    }
}

#[test]
fn the_tokens_the_widgets_read_are_defined() {
    let sheet = Stylesheet::parse(THEME);
    for token in ["selection", "caret", "scrollbar-thumb", "accent", "bg"] {
        assert!(sheet.variables.contains_key(token), "missing --{token}");
    }
}
