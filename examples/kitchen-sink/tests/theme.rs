//! The demo theme is part of the demo, so it gets checked like code.

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
    assert!(sheet.rules.len() > 40, "only {} rules parsed", sheet.rules.len());
}

#[test]
fn the_design_tokens_resolve() {
    let sheet = Stylesheet::parse(THEME);
    for token in ["primary", "surface", "text", "border", "radius", "speed"] {
        assert!(
            sheet.variables.contains_key(token),
            "missing token --{token}"
        );
    }
    assert_eq!(
        sheet.color_variable("primary"),
        Some(guirs::Rgba::hex(0x6c5ce7))
    );
}
