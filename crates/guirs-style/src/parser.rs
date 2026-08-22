//! Parser for the guirs stylesheet language.
//!
//! Parsing is error tolerant in the same way a browser is: a malformed
//! declaration is reported and skipped, and everything around it still applies.
//! That matters for a framework where stylesheets are hot reloaded, because one
//! bad character mid-edit should not blank the window.
//!
//! Custom properties are resolved at parse time rather than at cascade time.
//! The stylesheet is parsed once and consulted every frame, so doing the
//! substitution up front means the cascade never touches a token again.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use guirs_core::{
    BorderStyle, BoxShadow, Gradient, GradientStop, Length, Paint, Point, Px, Rgba, SharedString,
    Transform2D,
};
use smallvec::{smallvec, SmallVec};

use crate::lexer::{tokenize, Token, TokenKind, Unit};
use crate::selector::{Combinator, CompoundSelector, PseudoClass, Selector, Specificity};
use crate::style::{ShadowList, Style, TransitionList};
use crate::types::*;

/// How deep `var()` substitution will recurse before giving up.
const MAX_VAR_DEPTH: usize = 16;

/// A problem found while parsing. Parsing continues past it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// One parsed rule: a selector list and the declarations it applies.
#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: SmallVec<[Selector; 2]>,
    /// Shared so the cascade can hold on to it without copying.
    pub style: Arc<Style>,
    /// Position in the source, used to break specificity ties.
    pub order: u32,
}

impl Rule {
    /// The specificity of the most specific selector that matches, if any.
    pub fn match_specificity(
        &self,
        chain: &[crate::selector::ElementDescriptor],
    ) -> Option<Specificity> {
        self.selectors
            .iter()
            .filter(|s| s.matches(chain))
            .map(|s| s.specificity())
            .max()
    }

    /// Whether any selector depends on interaction state.
    pub fn is_dynamic(&self) -> bool {
        self.selectors.iter().any(|s| s.is_dynamic())
    }
}

/// A parsed stylesheet.
#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// Custom properties declared under `:root`, after substitution.
    pub variables: HashMap<SharedString, Vec<Token>>,
    /// Paths named by `@import`, in source order, relative to this sheet.
    pub imports: Vec<String>,
    /// Everything that went wrong. An empty list means a clean parse.
    pub errors: Vec<ParseError>,
}

impl Stylesheet {
    /// Parse stylesheet source.
    ///
    /// Never fails outright. Inspect [`Stylesheet::errors`] to see what was
    /// skipped.
    pub fn parse(source: &str) -> Stylesheet {
        Stylesheet::parse_with_variables(source, &HashMap::new())
    }

    /// Parse with a set of externally supplied variables layered underneath the
    /// sheet's own `:root` block.
    ///
    /// This is how a theme is swapped at runtime without editing the file: pass
    /// the overrides in and reparse, which costs microseconds for a sheet of
    /// any realistic size.
    pub fn parse_with_variables(
        source: &str,
        overrides: &HashMap<SharedString, String>,
    ) -> Stylesheet {
        let tokens = tokenize(source);
        let mut errors = Vec::new();
        let raw = collect_blocks(&tokens, &mut errors);

        // Pass one: gather custom properties from `:root` blocks.
        let mut variables: HashMap<SharedString, Vec<Token>> = HashMap::new();
        for (name, value) in overrides {
            variables.insert(name.clone(), tokenize(value));
        }
        for block in &raw.blocks {
            if !block.is_root_block {
                continue;
            }
            for decl in &block.declarations {
                if let Some(var_name) = decl.custom_property.as_ref() {
                    variables.insert(SharedString::from(var_name.as_str()), decl.value.clone());
                }
            }
        }

        // Resolve variables that reference other variables.
        let resolved = resolve_variable_table(variables, &mut errors);

        // Pass two: build rules with fully substituted values.
        let mut rules = Vec::new();
        for (order, block) in raw.blocks.iter().enumerate() {
            let selectors = match parse_selector_list_tokens(&block.prelude) {
                Ok(s) if !s.is_empty() => s,
                Ok(_) => continue,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };

            let mut style = Style::new();
            for decl in &block.declarations {
                if decl.custom_property.is_some() {
                    continue;
                }
                let value = substitute_vars(&decl.value, &resolved, 0, decl.line, &mut errors);
                apply_declaration(&mut style, &decl.name, &value, decl.line, &mut errors);
            }

            rules.push(Rule {
                selectors,
                style: Arc::new(style),
                order: order as u32,
            });
        }

        Stylesheet {
            rules,
            variables: resolved,
            imports: raw.imports,
            errors,
        }
    }

    /// Look up a custom property and parse it as a color.
    pub fn color_variable(&self, name: &str) -> Option<Rgba> {
        parse_color(self.variables.get(name)?)
    }

    /// Look up a custom property and parse it as a length.
    pub fn length_variable(&self, name: &str) -> Option<Length> {
        parse_length(self.variables.get(name)?)
    }

    /// Look up a custom property and parse it as a duration, in milliseconds.
    ///
    /// `none` reads as zero, so a token can turn something off as well as
    /// change how long it takes.
    pub fn duration_variable(&self, name: &str) -> Option<f32> {
        let tokens = self.variables.get(name)?;
        if single_ident(tokens).as_deref() == Some("none") {
            return Some(0.0);
        }
        parse_duration_ms(tokens)
    }

    /// Concatenate another sheet after this one. Later rules win ties, so the
    /// appended sheet acts as an override layer.
    pub fn extend(&mut self, other: Stylesheet) {
        let offset = self.rules.len() as u32;
        for mut rule in other.rules {
            rule.order += offset;
            self.rules.push(rule);
        }
        self.variables.extend(other.variables);
        self.imports.extend(other.imports);
        self.errors.extend(other.errors);
    }
}

// ---------------------------------------------------------------------------
// Block collection
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RawDeclaration {
    name: String,
    /// Set when the declaration defines a custom property.
    custom_property: Option<String>,
    value: Vec<Token>,
    line: u32,
}

#[derive(Debug)]
struct RawBlock {
    prelude: Vec<Token>,
    declarations: Vec<RawDeclaration>,
    is_root_block: bool,
}

#[derive(Debug, Default)]
struct RawSheet {
    blocks: Vec<RawBlock>,
    imports: Vec<String>,
}

fn collect_blocks(tokens: &[Token], errors: &mut Vec<ParseError>) -> RawSheet {
    let mut sheet = RawSheet::default();
    let mut i = 0;

    while i < tokens.len() {
        // Skip leading trivia.
        if matches!(tokens[i].kind, TokenKind::Whitespace | TokenKind::Semicolon) {
            i += 1;
            continue;
        }

        // At rules. Only `@import` is understood; anything else is skipped
        // whole so an unknown at rule cannot corrupt the rest of the sheet.
        if let TokenKind::AtKeyword(name) = &tokens[i].kind {
            let line = tokens[i].line;
            let keyword = name.clone();
            i += 1;
            let start = i;
            while i < tokens.len()
                && !matches!(tokens[i].kind, TokenKind::Semicolon | TokenKind::LBrace)
            {
                i += 1;
            }
            let prelude = &tokens[start..i];
            if i < tokens.len() && tokens[i].kind == TokenKind::LBrace {
                i = skip_balanced_block(tokens, i);
            } else if i < tokens.len() {
                i += 1; // consume the semicolon
            }

            if keyword == "import" {
                match prelude
                    .iter()
                    .find_map(|t| match &t.kind {
                        TokenKind::Str(s) => Some(s.clone()),
                        _ => None,
                    }) {
                    Some(path) => sheet.imports.push(path),
                    None => errors.push(ParseError {
                        line,
                        message: "@import needs a quoted path".into(),
                    }),
                }
            } else {
                errors.push(ParseError {
                    line,
                    message: format!("unknown at-rule @{keyword}"),
                });
            }
            continue;
        }

        // A normal rule: prelude up to `{`, then a declaration block.
        let start = i;
        while i < tokens.len() && tokens[i].kind != TokenKind::LBrace {
            if tokens[i].kind == TokenKind::RBrace {
                errors.push(ParseError {
                    line: tokens[i].line,
                    message: "unexpected closing brace".into(),
                });
                break;
            }
            i += 1;
        }

        if i >= tokens.len() || tokens[i].kind != TokenKind::LBrace {
            if start < tokens.len() {
                errors.push(ParseError {
                    line: tokens[start].line,
                    message: "rule is missing its declaration block".into(),
                });
            }
            i += 1;
            continue;
        }

        let prelude: Vec<Token> = tokens[start..i].to_vec();
        let block_end = skip_balanced_block(tokens, i);
        let body = &tokens[i + 1..block_end.saturating_sub(1).max(i + 1)];
        i = block_end;

        let is_root_block = prelude_is_root(&prelude);
        let declarations = parse_declaration_block(body, errors);
        sheet.blocks.push(RawBlock {
            prelude,
            declarations,
            is_root_block,
        });
    }

    sheet
}

/// Given an index pointing at `{`, return the index just past its `}`.
fn skip_balanced_block(tokens: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < tokens.len() {
        match tokens[i].kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    tokens.len()
}

fn prelude_is_root(prelude: &[Token]) -> bool {
    let significant: Vec<&TokenKind> = prelude
        .iter()
        .map(|t| &t.kind)
        .filter(|k| !matches!(k, TokenKind::Whitespace))
        .collect();
    significant.len() == 2
        && significant[0] == &TokenKind::Colon
        && significant[1] == &TokenKind::Ident("root".into())
}

fn parse_declaration_block(body: &[Token], errors: &mut Vec<ParseError>) -> Vec<RawDeclaration> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < body.len() {
        if matches!(body[i].kind, TokenKind::Whitespace | TokenKind::Semicolon) {
            i += 1;
            continue;
        }

        let line = body[i].line;
        let (name, custom_property) = match &body[i].kind {
            TokenKind::Ident(n) => (n.to_ascii_lowercase(), None),
            TokenKind::CustomProperty(n) => (format!("--{n}"), Some(n.clone())),
            other => {
                errors.push(ParseError {
                    line,
                    message: format!("expected a property name, found `{other}`"),
                });
                i = skip_to_declaration_end(body, i);
                continue;
            }
        };
        i += 1;

        while i < body.len() && body[i].kind == TokenKind::Whitespace {
            i += 1;
        }
        if i >= body.len() || body[i].kind != TokenKind::Colon {
            errors.push(ParseError {
                line,
                message: format!("expected `:` after `{name}`"),
            });
            i = skip_to_declaration_end(body, i);
            continue;
        }
        i += 1;

        let value_start = i;
        let mut depth = 0usize;
        while i < body.len() {
            match body[i].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth = depth.saturating_sub(1),
                TokenKind::Semicolon if depth == 0 => break,
                _ => {}
            }
            i += 1;
        }

        let value: Vec<Token> = body[value_start..i]
            .iter()
            .cloned()
            .skip_while(|t| t.kind == TokenKind::Whitespace)
            .collect();
        let value = trim_trailing_whitespace(value);

        if value.is_empty() {
            errors.push(ParseError {
                line,
                message: format!("`{name}` has no value"),
            });
        } else {
            out.push(RawDeclaration {
                name,
                custom_property,
                value,
                line,
            });
        }
        i += 1;
    }

    out
}

fn skip_to_declaration_end(body: &[Token], mut i: usize) -> usize {
    let mut depth = 0usize;
    while i < body.len() {
        match body[i].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth = depth.saturating_sub(1),
            TokenKind::Semicolon if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    i
}

fn trim_trailing_whitespace(mut tokens: Vec<Token>) -> Vec<Token> {
    while matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Whitespace)) {
        tokens.pop();
    }
    tokens
}

// ---------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------

fn resolve_variable_table(
    table: HashMap<SharedString, Vec<Token>>,
    errors: &mut Vec<ParseError>,
) -> HashMap<SharedString, Vec<Token>> {
    let mut resolved: HashMap<SharedString, Vec<Token>> = HashMap::new();
    for (name, tokens) in &table {
        let line = tokens.first().map(|t| t.line).unwrap_or(0);
        let value = substitute_vars(tokens, &table, 0, line, errors);
        resolved.insert(name.clone(), value);
    }
    resolved
}

/// Replace every `var(--name[, fallback])` with the variable's tokens.
fn substitute_vars(
    tokens: &[Token],
    table: &HashMap<SharedString, Vec<Token>>,
    depth: usize,
    line: u32,
    errors: &mut Vec<ParseError>,
) -> Vec<Token> {
    if depth > MAX_VAR_DEPTH {
        errors.push(ParseError {
            line,
            message: "var() nested too deeply, which usually means a cycle".into(),
        });
        return Vec::new();
    }
    if !tokens
        .iter()
        .any(|t| matches!(&t.kind, TokenKind::Function(f) if f == "var"))
    {
        return tokens.to_vec();
    }

    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let is_var = matches!(&tokens[i].kind, TokenKind::Function(f) if f == "var");
        if !is_var {
            out.push(tokens[i].clone());
            i += 1;
            continue;
        }

        let call_line = tokens[i].line;
        let args_start = i + 1;
        let args_end = find_matching_paren(tokens, args_start);
        let args = &tokens[args_start..args_end];
        i = (args_end + 1).min(tokens.len());

        let mut parts = split_on_commas(args);
        let name_tokens = if parts.is_empty() {
            Vec::new()
        } else {
            parts.remove(0)
        };
        let name = name_tokens.iter().find_map(|t| match &t.kind {
            TokenKind::CustomProperty(n) => Some(n.clone()),
            _ => None,
        });

        let Some(name) = name else {
            errors.push(ParseError {
                line: call_line,
                message: "var() needs a --custom-property name".into(),
            });
            continue;
        };

        match table.get(name.as_str()) {
            Some(value) => {
                let expanded = substitute_vars(value, table, depth + 1, call_line, errors);
                out.extend(expanded);
            }
            None if !parts.is_empty() => {
                let fallback: Vec<Token> = parts.concat();
                let expanded = substitute_vars(&fallback, table, depth + 1, call_line, errors);
                out.extend(expanded);
            }
            None => errors.push(ParseError {
                line: call_line,
                message: format!("undefined variable --{name} and no fallback given"),
            }),
        }
    }
    out
}

/// Given the index just past a `Function` token, find the index of its `)`.
fn find_matching_paren(tokens: &[Token], start: usize) -> usize {
    let mut depth = 1usize;
    let mut i = start;
    while i < tokens.len() {
        match tokens[i].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::Function(_) => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    tokens.len()
}

fn split_on_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            TokenKind::LParen | TokenKind::Function(_) => {
                depth += 1;
                current.push(token.clone());
            }
            TokenKind::RParen => {
                depth = depth.saturating_sub(1);
                current.push(token.clone());
            }
            TokenKind::Comma if depth == 0 => {
                out.push(trim_ws(std::mem::take(&mut current)));
            }
            _ => current.push(token.clone()),
        }
    }
    let last = trim_ws(current);
    if !last.is_empty() || !out.is_empty() {
        out.push(last);
    }
    out.retain(|p| !p.is_empty());
    out
}

fn trim_ws(tokens: Vec<Token>) -> Vec<Token> {
    let mut t = tokens;
    while matches!(t.first().map(|x| &x.kind), Some(TokenKind::Whitespace)) {
        t.remove(0);
    }
    trim_trailing_whitespace(t)
}

// ---------------------------------------------------------------------------
// Selector parsing
// ---------------------------------------------------------------------------

/// Parse a comma separated selector list from source text.
pub fn parse_selector_list(source: &str) -> Result<SmallVec<[Selector; 2]>, ParseError> {
    parse_selector_list_tokens(&tokenize(source))
}

/// Parse a comma separated selector list from tokens.
pub fn parse_selector_list_tokens(tokens: &[Token]) -> Result<SmallVec<[Selector; 2]>, ParseError> {
    let mut out = SmallVec::new();
    for part in split_on_commas(tokens) {
        out.push(parse_one_selector(&part)?);
    }
    Ok(out)
}

fn parse_one_selector(tokens: &[Token]) -> Result<Selector, ParseError> {
    let line = tokens.first().map(|t| t.line).unwrap_or(0);
    let mut compounds: SmallVec<[CompoundSelector; 2]> = SmallVec::new();
    let mut combinators: SmallVec<[Combinator; 2]> = SmallVec::new();
    let mut current = CompoundSelector::default();
    let mut pending: Option<Combinator> = None;
    let mut started = false;

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i].kind {
            TokenKind::Whitespace => {
                if started {
                    pending = Some(pending.unwrap_or(Combinator::Descendant));
                }
                i += 1;
            }
            TokenKind::Delim('>') => {
                if !started {
                    return Err(ParseError {
                        line: tokens[i].line,
                        message: "`>` needs a selector on its left".into(),
                    });
                }
                pending = Some(Combinator::Child);
                i += 1;
            }
            _ => {
                // A new compound starts here if a combinator is pending.
                if let Some(combinator) = pending.take() {
                    if started {
                        compounds.push(std::mem::take(&mut current));
                        combinators.push(combinator);
                    }
                }
                started = true;

                match &tokens[i].kind {
                    TokenKind::Ident(name) => {
                        if current.element_type.is_some() {
                            return Err(ParseError {
                                line: tokens[i].line,
                                message: format!("two type selectors in one compound: `{name}`"),
                            });
                        }
                        current.element_type = Some(SharedString::from(name.as_str()));
                        i += 1;
                    }
                    TokenKind::Delim('*') => {
                        i += 1;
                    }
                    TokenKind::Hash(name) => {
                        current.id = Some(SharedString::from(name.as_str()));
                        i += 1;
                    }
                    TokenKind::Delim('.') => {
                        i += 1;
                        match tokens.get(i).map(|t| &t.kind) {
                            Some(TokenKind::Ident(name)) => {
                                current.classes.push(SharedString::from(name.as_str()));
                                i += 1;
                            }
                            _ => {
                                return Err(ParseError {
                                    line,
                                    message: "`.` must be followed by a class name".into(),
                                })
                            }
                        }
                    }
                    TokenKind::Colon => {
                        i += 1;
                        // A second colon marks a pseudo element, which this
                        // language does not have. Treat it as a pseudo class so
                        // `::placeholder` style syntax at least parses.
                        if tokens.get(i).map(|t| &t.kind) == Some(&TokenKind::Colon) {
                            i += 1;
                        }
                        match tokens.get(i).map(|t| t.kind.clone()) {
                            Some(TokenKind::Ident(name)) => {
                                let pseudo =
                                    PseudoClass::parse(&name.to_ascii_lowercase()).ok_or_else(
                                        || ParseError {
                                            line,
                                            message: format!("unknown pseudo class `:{name}`"),
                                        },
                                    )?;
                                current.pseudo_classes.push(pseudo);
                                i += 1;
                            }
                            Some(TokenKind::Function(name)) if name == "not" => {
                                let args_start = i + 1;
                                let args_end = find_matching_paren(tokens, args_start);
                                let inner = parse_one_selector(&tokens[args_start..args_end])?;
                                if inner.compounds.len() != 1 {
                                    return Err(ParseError {
                                        line,
                                        message: ":not() takes a single compound selector".into(),
                                    });
                                }
                                current.pseudo_classes.push(PseudoClass::Not(Box::new(
                                    inner.compounds.into_iter().next().unwrap(),
                                )));
                                i = (args_end + 1).min(tokens.len());
                            }
                            Some(TokenKind::Function(name)) => {
                                return Err(ParseError {
                                    line,
                                    message: format!("unsupported functional pseudo class `{name}()`"),
                                })
                            }
                            _ => {
                                return Err(ParseError {
                                    line,
                                    message: "`:` must be followed by a pseudo class".into(),
                                })
                            }
                        }
                    }
                    other => {
                        return Err(ParseError {
                            line: tokens[i].line,
                            message: format!("unexpected `{other}` in a selector"),
                        })
                    }
                }
            }
        }
    }

    if !started {
        return Err(ParseError {
            line,
            message: "empty selector".into(),
        });
    }
    compounds.push(current);

    Ok(Selector {
        compounds,
        combinators,
    })
}

// ---------------------------------------------------------------------------
// Value parsing helpers
// ---------------------------------------------------------------------------

/// Significant tokens, with whitespace stripped.
fn significant(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Whitespace)
        .collect()
}

/// Split a value on whitespace at the top nesting level.
fn split_on_whitespace(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            TokenKind::LParen | TokenKind::Function(_) => {
                depth += 1;
                current.push(token.clone());
            }
            TokenKind::RParen => {
                depth = depth.saturating_sub(1);
                current.push(token.clone());
            }
            TokenKind::Whitespace if depth == 0 => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(token.clone()),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn single_ident(tokens: &[Token]) -> Option<String> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Ident(name) => Some(name.to_ascii_lowercase()),
        _ => None,
    }
}

/// Parse a length. Bare numbers are treated as pixels, which is what almost
/// everyone means and avoids a class of silently ignored declarations.
pub fn parse_length(tokens: &[Token]) -> Option<Length> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Ident(name) if name.eq_ignore_ascii_case("auto") => Some(Length::Auto),
        TokenKind::Number(v) => Some(Length::Px(Px(*v))),
        TokenKind::Percentage(v) => Some(Length::Percent(*v)),
        TokenKind::Dimension(v, Unit::Px) => Some(Length::Px(Px(*v))),
        TokenKind::Dimension(v, Unit::Rem) => Some(Length::Rems(guirs_core::rems(*v))),
        TokenKind::Dimension(v, Unit::Fr) => Some(Length::Fraction(*v)),
        _ => None,
    }
}

/// Parse an absolute pixel value. Rejects percentages, which have no meaning
/// for border widths or corner radii here.
pub fn parse_px(tokens: &[Token]) -> Option<Px> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Number(v) => Some(Px(*v)),
        TokenKind::Dimension(v, Unit::Px) => Some(Px(*v)),
        _ => None,
    }
}

fn parse_number(tokens: &[Token]) -> Option<f32> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Number(v) => Some(*v),
        TokenKind::Percentage(v) => Some(*v),
        _ => None,
    }
}

/// Parse a list of grid tracks: `200px 1fr 2fr auto`.
///
/// `repeat(n, ...)` is expanded here rather than passed along, because the
/// layout engine only takes repetitions whose every track has a fixed size and
/// this way `repeat(3, 1fr)` works, which is most of what anybody writes.
fn parse_tracks(tokens: &[Token]) -> Option<std::sync::Arc<[TrackSize]>> {
    let mut out: Vec<TrackSize> = Vec::new();
    for part in split_on_whitespace(tokens) {
        let sig = significant(&part);
        if sig.is_empty() {
            continue;
        }
        // Given the part as it was written, not the significant tokens: the
        // tracks inside a repetition are separated by the whitespace that
        // stripping them would remove.
        if let Some(expanded) = parse_repeat(&part) {
            out.extend(expanded);
            continue;
        }
        out.push(parse_track(&sig)?);
    }
    if out.is_empty() {
        return None;
    }
    Some(out.into())
}

/// `repeat(3, 1fr)`, expanded into three tracks.
fn parse_repeat(part: &[Token]) -> Option<Vec<TrackSize>> {
    let TokenKind::Function(name) = &part.first()?.kind else {
        return None;
    };
    if !name.eq_ignore_ascii_case("repeat") {
        return None;
    }
    // Between the function's own parentheses, whitespace and all.
    let inner: Vec<Token> = part[1..part.len().saturating_sub(1)].to_vec();
    let args = split_on_commas(&inner);
    if args.len() != 2 {
        return None;
    }
    let count = parse_number(&args[0])?;
    if !(1.0..=1024.0).contains(&count) {
        return None;
    }
    let mut tracks = Vec::new();
    for part in split_on_whitespace(&args[1]) {
        let sig = significant(&part);
        if !sig.is_empty() {
            tracks.push(parse_track(&sig)?);
        }
    }
    if tracks.is_empty() {
        return None;
    }
    Some(
        std::iter::repeat_n(tracks, count as usize)
            .flatten()
            .collect(),
    )
}

fn parse_track(sig: &[&Token]) -> Option<TrackSize> {
    if sig.len() != 1 {
        return None;
    }
    Some(match &sig[0].kind {
        TokenKind::Dimension(value, Unit::Fr) => TrackSize::Fr(*value),
        TokenKind::Dimension(value, Unit::Px) => TrackSize::Px(Px(*value)),
        TokenKind::Percentage(value) => TrackSize::Percent(*value),
        TokenKind::Number(value) if *value == 0.0 => TrackSize::Px(Px::ZERO),
        TokenKind::Ident(name) => match name.to_ascii_lowercase().as_str() {
            "auto" => TrackSize::Auto,
            "min-content" => TrackSize::MinContent,
            "max-content" => TrackSize::MaxContent,
            _ => return None,
        },
        _ => return None,
    })
}

/// Parse `grid-column` and `grid-row`: `2`, `2 / 5`, `span 3`, `1 / span 2`.
fn parse_grid_line(tokens: &[Token]) -> Option<GridLine> {
    let halves = split_on_slash(tokens);
    let start = parse_placement(&halves[0])?;
    let end = match halves.get(1) {
        Some(part) => parse_placement(part)?,
        None => GridPlacement::Auto,
    };
    Some(GridLine { start, end })
}

fn parse_placement(tokens: &[Token]) -> Option<GridPlacement> {
    let sig = significant(tokens);
    match sig.len() {
        0 => Some(GridPlacement::Auto),
        1 => match &sig[0].kind {
            TokenKind::Number(value) => Some(GridPlacement::Line(*value as i16)),
            TokenKind::Ident(name) if name.eq_ignore_ascii_case("auto") => {
                Some(GridPlacement::Auto)
            }
            _ => None,
        },
        2 => match (&sig[0].kind, &sig[1].kind) {
            (TokenKind::Ident(name), TokenKind::Number(count))
                if name.eq_ignore_ascii_case("span") =>
            {
                Some(GridPlacement::Span((*count).max(1.0) as u16))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Split on `/`, which separates the two ends of a grid placement.
fn split_on_slash(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![Vec::new()];
    for token in tokens {
        if matches!(&token.kind, TokenKind::Delim('/')) {
            out.push(Vec::new());
        } else {
            out.last_mut().expect("always one part").push(token.clone());
        }
    }
    out
}

/// Parse a duration in either seconds or milliseconds, returning milliseconds.
fn parse_duration_ms(tokens: &[Token]) -> Option<f32> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Dimension(v, Unit::Ms) => Some(*v),
        TokenKind::Dimension(v, Unit::S) => Some(*v * 1000.0),
        TokenKind::Number(v) if *v == 0.0 => Some(0.0),
        _ => None,
    }
}

fn parse_angle_deg(tokens: &[Token]) -> Option<f32> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Dimension(v, Unit::Deg) => Some(*v),
        TokenKind::Number(v) => Some(*v),
        _ => None,
    }
}

/// Parse a color: hex, keyword, `rgb`, `rgba`, `hsl`, `hsla`, or one of the
/// derivation helpers `lighten`, `darken`, `alpha` and `mix`.
pub fn parse_color(tokens: &[Token]) -> Option<Rgba> {
    let sig = significant(tokens);
    match sig.first().map(|t| &t.kind)? {
        TokenKind::Hash(digits) if sig.len() == 1 => Rgba::parse(&format!("#{digits}")),
        TokenKind::Ident(name) if sig.len() == 1 => Rgba::parse(name),
        TokenKind::Function(name) => {
            let args_end = find_matching_paren(tokens, function_body_start(tokens)?);
            let args = split_on_commas(&tokens[function_body_start(tokens)?..args_end]);
            parse_color_function(&name.to_ascii_lowercase(), &args)
        }
        _ => None,
    }
}

/// Index just past the leading `Function` token in a value.
fn function_body_start(tokens: &[Token]) -> Option<usize> {
    tokens
        .iter()
        .position(|t| matches!(t.kind, TokenKind::Function(_)))
        .map(|i| i + 1)
}

fn parse_color_function(name: &str, args: &[Vec<Token>]) -> Option<Rgba> {
    /// Accepts `0..255` numbers or percentages.
    fn channel(tokens: &[Token]) -> Option<f32> {
        let sig = significant(tokens);
        match sig.first().map(|t| &t.kind)? {
            TokenKind::Number(v) => Some(v / 255.0),
            TokenKind::Percentage(v) => Some(*v),
            _ => None,
        }
    }
    /// Accepts `0..1` numbers or percentages.
    fn unit(tokens: &[Token]) -> Option<f32> {
        let sig = significant(tokens);
        match sig.first().map(|t| &t.kind)? {
            TokenKind::Number(v) => Some(*v),
            TokenKind::Percentage(v) => Some(*v),
            _ => None,
        }
    }

    match name {
        "rgb" | "rgba" => {
            // `rgb(r g b / a)` is not supported; the comma form is.
            let r = channel(args.first()?)?;
            let g = channel(args.get(1)?)?;
            let b = channel(args.get(2)?)?;
            let a = match args.get(3) {
                Some(t) => unit(t)?,
                None => 1.0,
            };
            Some(Rgba::new(r, g, b, a))
        }
        "hsl" | "hsla" => {
            let h = match significant(args.first()?).first().map(|t| &t.kind)? {
                TokenKind::Number(v) => *v,
                TokenKind::Dimension(v, Unit::Deg) => *v,
                _ => return None,
            };
            let s = unit(args.get(1)?)?;
            let l = unit(args.get(2)?)?;
            let a = match args.get(3) {
                Some(t) => unit(t)?,
                None => 1.0,
            };
            Some(Hsla::deg(h, s, l, a).into())
        }
        "lighten" => Some(parse_color(args.first()?)?.lighten(unit(args.get(1)?)?)),
        "darken" => Some(parse_color(args.first()?)?.darken(unit(args.get(1)?)?)),
        "alpha" | "fade" => Some(parse_color(args.first()?)?.with_alpha(unit(args.get(1)?)?)),
        "mix" => {
            let a = parse_color(args.first()?)?;
            let b = parse_color(args.get(1)?)?;
            let t = match args.get(2) {
                Some(t) => unit(t)?,
                None => 0.5,
            };
            Some(a.lerp(b, t))
        }
        _ => None,
    }
}

use guirs_core::Hsla;

/// Parse a background: a color, a gradient, or `none`.
pub fn parse_paint(tokens: &[Token]) -> Option<Paint> {
    if let Some(ident) = single_ident(tokens) {
        if ident == "none" || ident == "transparent" {
            return Some(Paint::None);
        }
    }
    if let Some(color) = parse_color(tokens) {
        return Some(Paint::Solid(color));
    }

    let sig = significant(tokens);
    let TokenKind::Function(name) = sig.first().map(|t| &t.kind)? else {
        return None;
    };
    let name = name.to_ascii_lowercase();
    let body = function_body_start(tokens)?;
    let end = find_matching_paren(tokens, body);
    let args = split_on_commas(&tokens[body..end]);

    match name.as_str() {
        "linear-gradient" => {
            let (angle, stop_args) = match args.first() {
                Some(first) if parse_angle_deg(first).is_some() => {
                    (parse_angle_deg(first).unwrap(), &args[1..])
                }
                Some(first) if is_to_side(first).is_some() => {
                    (is_to_side(first).unwrap(), &args[1..])
                }
                _ => (180.0, &args[..]),
            };
            let stops = parse_gradient_stops(stop_args)?;
            Some(Paint::Gradient(Arc::new(Gradient::linear(angle, stops))))
        }
        "radial-gradient" => {
            let stops = parse_gradient_stops(&args)?;
            Some(Paint::Gradient(Arc::new(Gradient::radial(stops))))
        }
        _ => None,
    }
}

/// Interpret `to bottom`, `to right` and the diagonal forms as an angle.
fn is_to_side(tokens: &[Token]) -> Option<f32> {
    let sig = significant(tokens);
    let words: Vec<String> = sig
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::Ident(n) => Some(n.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if words.first().map(|w| w.as_str()) != Some("to") {
        return None;
    }
    let sides = &words[1..];
    let has = |name: &str| sides.iter().any(|s| s == name);
    Some(match (has("top"), has("right"), has("bottom"), has("left")) {
        (true, true, _, _) => 45.0,
        (_, true, true, _) => 135.0,
        (_, _, true, true) => 225.0,
        (true, _, _, true) => 315.0,
        (true, _, _, _) => 0.0,
        (_, true, _, _) => 90.0,
        (_, _, true, _) => 180.0,
        (_, _, _, true) => 270.0,
        _ => return None,
    })
}

fn parse_gradient_stops(args: &[Vec<Token>]) -> Option<Vec<GradientStop>> {
    if args.is_empty() {
        return None;
    }
    let mut parsed: Vec<(Rgba, Option<f32>)> = Vec::with_capacity(args.len());
    for arg in args {
        let parts = split_on_whitespace(arg);
        let color = parse_color(parts.first()?)?;
        let offset = match parts.get(1) {
            Some(p) => match significant(p).first().map(|t| &t.kind) {
                Some(TokenKind::Percentage(v)) => Some(*v),
                Some(TokenKind::Number(v)) => Some(*v),
                _ => None,
            },
            None => None,
        };
        parsed.push((color, offset));
    }

    // Distribute any stop that did not name a position evenly across the run.
    let count = parsed.len();
    let mut stops = Vec::with_capacity(count);
    for (i, (color, offset)) in parsed.iter().enumerate() {
        let position = offset.unwrap_or_else(|| {
            if count <= 1 {
                0.0
            } else {
                i as f32 / (count - 1) as f32
            }
        });
        stops.push(GradientStop::new(position, *color));
    }
    Some(stops)
}

/// Expand the one to four value CSS shorthand into top, right, bottom, left.
fn expand_shorthand<T: Copy>(values: &[T]) -> Option<(T, T, T, T)> {
    match values {
        [a] => Some((*a, *a, *a, *a)),
        [a, b] => Some((*a, *b, *a, *b)),
        [a, b, c] => Some((*a, *b, *c, *b)),
        [a, b, c, d] => Some((*a, *b, *c, *d)),
        _ => None,
    }
}

/// `translate(10px, 4px) rotate(45deg) scale(1.2)`, in any order.
///
/// The result is decomposed rather than multiplied out. Writing the functions
/// in a different order therefore does not change the outcome, which differs
/// from CSS and is the deliberate trade for transforms that interpolate the
/// way a reader expects when they animate.
fn parse_transform(tokens: &[Token]) -> Option<Transform2D> {
    if single_ident(tokens).as_deref() == Some("none") {
        return Some(Transform2D::IDENTITY);
    }

    let mut transform = Transform2D::IDENTITY;
    let mut index = 0usize;
    let mut saw_one = false;
    let significant: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Whitespace)
        .collect();

    while index < significant.len() {
        let TokenKind::Function(name) = &significant[index].kind else {
            return None;
        };
        // Arguments run to the matching close paren, which for these functions
        // is never nested.
        let start = index + 1;
        let mut end = start;
        let mut depth = 1usize;
        while end < significant.len() {
            match significant[end].kind {
                TokenKind::LParen | TokenKind::Function(_) => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        if depth != 0 {
            return None;
        }

        let args: Vec<Token> = significant[start..end].iter().map(|t| (*t).clone()).collect();
        let parts = split_on_commas(&args);
        let numbers: Vec<f32> = parts.iter().filter_map(|part| number_or_px(part)).collect();
        if numbers.len() != parts.len() {
            return None;
        }

        // `translateX` is as legitimate as `translatex`, the same as every
        // other identifier in the language.
        match (name.to_ascii_lowercase().as_str(), numbers.len()) {
            ("translate", 1) => transform.translate_x = Px(numbers[0]),
            ("translate", 2) => {
                transform.translate_x = Px(numbers[0]);
                transform.translate_y = Px(numbers[1]);
            }
            ("translatex", 1) => transform.translate_x = Px(numbers[0]),
            ("translatey", 1) => transform.translate_y = Px(numbers[0]),
            ("scale", 1) => {
                transform.scale_x = numbers[0];
                transform.scale_y = numbers[0];
            }
            ("scale", 2) => {
                transform.scale_x = numbers[0];
                transform.scale_y = numbers[1];
            }
            ("scalex", 1) => transform.scale_x = numbers[0],
            ("scaley", 1) => transform.scale_y = numbers[0],
            ("rotate", 1) => transform.rotate = numbers[0],
            _ => return None,
        }

        saw_one = true;
        index = end + 1;
    }

    if saw_one {
        Some(transform)
    } else {
        None
    }
}

/// A bare number, a pixel length or an angle, whichever the argument is.
fn number_or_px(tokens: &[Token]) -> Option<f32> {
    let sig = significant(tokens);
    if sig.len() != 1 {
        return None;
    }
    match &sig[0].kind {
        TokenKind::Number(v) => Some(*v),
        TokenKind::Dimension(v, Unit::Px) => Some(*v),
        TokenKind::Dimension(v, Unit::Deg) => Some(*v),
        _ => None,
    }
}

/// `50% 50%`, `left top`, or a single value applied to both axes.
fn parse_transform_origin(tokens: &[Token]) -> Option<(f32, f32)> {
    let parts = split_on_whitespace(tokens);
    let axis = |part: &[Token], horizontal: bool| -> Option<f32> {
        let sig = significant(part);
        if sig.len() != 1 {
            return None;
        }
        match &sig[0].kind {
            TokenKind::Percentage(fraction) => Some(*fraction),
            TokenKind::Number(v) if *v == 0.0 => Some(0.0),
            TokenKind::Ident(name) => match (name.to_ascii_lowercase().as_str(), horizontal) {
                ("left", true) | ("top", false) => Some(0.0),
                ("center", _) => Some(0.5),
                ("right", true) | ("bottom", false) => Some(1.0),
                _ => None,
            },
            _ => None,
        }
    };

    match parts.len() {
        1 => {
            let value = axis(&parts[0], true)?;
            Some((value, value))
        }
        2 => Some((axis(&parts[0], true)?, axis(&parts[1], false)?)),
        _ => None,
    }
}

fn parse_shadow_list(tokens: &[Token]) -> Option<ShadowList> {
    if single_ident(tokens).as_deref() == Some("none") {
        return Some(SmallVec::new());
    }
    let mut out: ShadowList = SmallVec::new();
    for part in split_on_commas(tokens) {
        out.push(parse_one_shadow(&part)?);
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

fn parse_one_shadow(tokens: &[Token]) -> Option<BoxShadow> {
    let mut shadow = BoxShadow {
        color: Rgba::BLACK,
        offset: Point::new(Px::ZERO, Px::ZERO),
        blur: Px::ZERO,
        spread: Px::ZERO,
        inset: false,
    };
    let mut lengths: Vec<Px> = Vec::new();
    let mut saw_color = false;

    for part in split_on_whitespace(tokens) {
        if single_ident(&part).as_deref() == Some("inset") {
            shadow.inset = true;
            continue;
        }
        if let Some(v) = parse_px(&part) {
            lengths.push(v);
            continue;
        }
        if let Some(c) = parse_color(&part) {
            shadow.color = c;
            saw_color = true;
            continue;
        }
        return None;
    }

    // A shadow with no color at all is almost certainly a mistake, but CSS
    // defaults it to the text color; black is the closer analogue here.
    let _ = saw_color;

    match lengths.len() {
        2 => shadow.offset = Point::new(lengths[0], lengths[1]),
        3 => {
            shadow.offset = Point::new(lengths[0], lengths[1]);
            shadow.blur = lengths[2];
        }
        4 => {
            shadow.offset = Point::new(lengths[0], lengths[1]);
            shadow.blur = lengths[2];
            shadow.spread = lengths[3];
        }
        _ => return None,
    }
    Some(shadow)
}

fn parse_transition_list(tokens: &[Token]) -> Option<TransitionList> {
    if single_ident(tokens).as_deref() == Some("none") {
        return Some(SmallVec::new());
    }
    let mut out: TransitionList = SmallVec::new();
    for part in split_on_commas(tokens) {
        out.push(parse_one_transition(&part)?);
    }
    Some(out)
}

fn parse_one_transition(tokens: &[Token]) -> Option<Transition> {
    let mut transition = Transition::default();
    let mut durations: Vec<f32> = Vec::new();
    let mut saw_target = false;

    for part in split_on_whitespace(tokens) {
        if let Some(ms) = parse_duration_ms(&part) {
            durations.push(ms);
            continue;
        }
        if let Some(easing) = parse_easing(&part) {
            transition.easing = easing;
            continue;
        }
        if let Some(ident) = single_ident(&part) {
            if !saw_target {
                transition.target = if ident == "all" {
                    TransitionTarget::All
                } else {
                    TransitionTarget::One(AnimatableProperty::from_name(&ident)?)
                };
                saw_target = true;
                continue;
            }
        }
        return None;
    }

    // The first duration is the duration, the second is the delay.
    if let Some(d) = durations.first() {
        transition.duration_ms = OrderedF32(*d);
    }
    if let Some(d) = durations.get(1) {
        transition.delay_ms = OrderedF32(*d);
    }
    Some(transition)
}

fn parse_easing(tokens: &[Token]) -> Option<Easing> {
    if let Some(ident) = single_ident(tokens) {
        return Some(match ident.as_str() {
            "linear" => Easing::Linear,
            "ease" => Easing::EASE,
            "ease-in" => Easing::EaseIn,
            "ease-out" => Easing::EaseOut,
            "ease-in-out" => Easing::EaseInOut,
            _ => return None,
        });
    }
    let sig = significant(tokens);
    let TokenKind::Function(name) = sig.first().map(|t| &t.kind)? else {
        return None;
    };
    let body = function_body_start(tokens)?;
    let end = find_matching_paren(tokens, body);
    let args = split_on_commas(&tokens[body..end]);
    match name.to_ascii_lowercase().as_str() {
        "cubic-bezier" => Some(Easing::CubicBezier(
            OrderedF32(parse_number(args.first()?)?),
            OrderedF32(parse_number(args.get(1)?)?),
            OrderedF32(parse_number(args.get(2)?)?),
            OrderedF32(parse_number(args.get(3)?)?),
        )),
        "steps" => Some(Easing::Steps(parse_number(args.first()?)? as u16)),
        _ => None,
    }
}

fn parse_font_family(tokens: &[Token]) -> Option<FontFamily> {
    let mut families = Vec::new();
    for part in split_on_commas(tokens) {
        let sig = significant(&part);
        match sig.first().map(|t| &t.kind) {
            Some(TokenKind::Str(s)) => families.push(SharedString::from(s.as_str())),
            Some(TokenKind::Ident(_)) => {
                // An unquoted family name may be several idents, as in
                // `Segoe UI`. Join them back together.
                let joined: Vec<String> = sig
                    .iter()
                    .filter_map(|t| match &t.kind {
                        TokenKind::Ident(n) => Some(n.clone()),
                        _ => None,
                    })
                    .collect();
                if joined.is_empty() {
                    return None;
                }
                families.push(SharedString::from(joined.join(" ")));
            }
            _ => return None,
        }
    }
    if families.is_empty() {
        None
    } else {
        Some(FontFamily::new(families))
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// Apply one declaration to a style, reporting anything unusable.
pub fn apply_declaration(
    style: &mut Style,
    name: &str,
    value: &[Token],
    line: u32,
    errors: &mut Vec<ParseError>,
) {
    let mut bad = |what: &str| {
        errors.push(ParseError {
            line,
            message: format!("`{name}` does not accept `{what}`"),
        });
    };
    let rendered = || {
        value
            .iter()
            .map(|t| t.kind.to_string())
            .collect::<String>()
            .trim()
            .to_string()
    };

    macro_rules! set {
        ($field:ident, $parser:expr) => {
            match $parser {
                Some(v) => style.$field = Some(v),
                None => bad(&rendered()),
            }
        };
    }

    match name {
        // -- display and flow --
        "display" => set!(
            display,
            match single_ident(value).as_deref() {
                Some("flex") => Some(Display::Flex),
                Some("block") => Some(Display::Block),
                Some("grid") => Some(Display::Grid),
                Some("none") => Some(Display::None),
                _ => None,
            }
        ),
        "position" => set!(
            position,
            match single_ident(value).as_deref() {
                Some("relative") => Some(Position::Relative),
                Some("absolute") => Some(Position::Absolute),
                _ => None,
            }
        ),
        "flex-direction" => set!(
            flex_direction,
            match single_ident(value).as_deref() {
                Some("row") => Some(FlexDirection::Row),
                Some("column") => Some(FlexDirection::Column),
                Some("row-reverse") => Some(FlexDirection::RowReverse),
                Some("column-reverse") => Some(FlexDirection::ColumnReverse),
                _ => None,
            }
        ),
        "flex-wrap" => set!(
            flex_wrap,
            match single_ident(value).as_deref() {
                Some("nowrap") => Some(FlexWrap::NoWrap),
                Some("wrap") => Some(FlexWrap::Wrap),
                Some("wrap-reverse") => Some(FlexWrap::WrapReverse),
                _ => None,
            }
        ),
        "justify-content" => set!(justify_content, parse_justify(value)),
        "align-content" => set!(align_content, parse_justify(value)),
        "align-items" => set!(align_items, parse_align(value)),
        "align-self" => set!(align_self, parse_align(value)),
        "grid-template-columns" => set!(grid_template_columns, parse_tracks(value)),
        "grid-template-rows" => set!(grid_template_rows, parse_tracks(value)),
        "grid-column" => set!(grid_column, parse_grid_line(value)),
        "grid-row" => set!(grid_row, parse_grid_line(value)),
        "flex-grow" => set!(flex_grow, parse_number(value)),
        "flex-shrink" => set!(flex_shrink, parse_number(value)),
        "flex-basis" => set!(flex_basis, parse_length(value)),
        "flex" => {
            let parts = split_on_whitespace(value);
            match parts.len() {
                1 => match parse_number(&parts[0]) {
                    Some(grow) => {
                        style.flex_grow = Some(grow);
                        style.flex_shrink = Some(1.0);
                        style.flex_basis = Some(Length::Px(Px::ZERO));
                    }
                    None => match parse_length(&parts[0]) {
                        Some(basis) => style.flex_basis = Some(basis),
                        None => bad(&rendered()),
                    },
                },
                2 => {
                    style.flex_grow = parse_number(&parts[0]);
                    if let Some(shrink) = parse_number(&parts[1]) {
                        style.flex_shrink = Some(shrink);
                    } else {
                        style.flex_basis = parse_length(&parts[1]);
                        style.flex_shrink = Some(1.0);
                    }
                }
                3 => {
                    style.flex_grow = parse_number(&parts[0]);
                    style.flex_shrink = parse_number(&parts[1]);
                    style.flex_basis = parse_length(&parts[2]);
                }
                _ => bad(&rendered()),
            }
        }

        // -- box metrics --
        "width" => set!(width, parse_length(value)),
        "height" => set!(height, parse_length(value)),
        "min-width" => set!(min_width, parse_length(value)),
        "min-height" => set!(min_height, parse_length(value)),
        "max-width" => set!(max_width, parse_length(value)),
        "max-height" => set!(max_height, parse_length(value)),
        "aspect-ratio" => set!(aspect_ratio, parse_number(value)),

        "padding" | "margin" | "inset" => {
            let parts = split_on_whitespace(value);
            let lengths: Option<Vec<Length>> = parts.iter().map(|p| parse_length(p)).collect();
            match lengths.and_then(|l| expand_shorthand(&l)) {
                Some((t, r, b, l)) => match name {
                    "padding" => {
                        style.padding_top = Some(t);
                        style.padding_right = Some(r);
                        style.padding_bottom = Some(b);
                        style.padding_left = Some(l);
                    }
                    "margin" => {
                        style.margin_top = Some(t);
                        style.margin_right = Some(r);
                        style.margin_bottom = Some(b);
                        style.margin_left = Some(l);
                    }
                    _ => {
                        style.top = Some(t);
                        style.right = Some(r);
                        style.bottom = Some(b);
                        style.left = Some(l);
                    }
                },
                None => bad(&rendered()),
            }
        }
        "padding-top" => set!(padding_top, parse_length(value)),
        "padding-right" => set!(padding_right, parse_length(value)),
        "padding-bottom" => set!(padding_bottom, parse_length(value)),
        "padding-left" => set!(padding_left, parse_length(value)),
        "margin-top" => set!(margin_top, parse_length(value)),
        "margin-right" => set!(margin_right, parse_length(value)),
        "margin-bottom" => set!(margin_bottom, parse_length(value)),
        "margin-left" => set!(margin_left, parse_length(value)),
        "top" => set!(top, parse_length(value)),
        "right" => set!(right, parse_length(value)),
        "bottom" => set!(bottom, parse_length(value)),
        "left" => set!(left, parse_length(value)),

        "gap" => {
            let parts = split_on_whitespace(value);
            match parts.len() {
                1 => match parse_length(&parts[0]) {
                    Some(v) => {
                        style.row_gap = Some(v);
                        style.column_gap = Some(v);
                    }
                    None => bad(&rendered()),
                },
                2 => match (parse_length(&parts[0]), parse_length(&parts[1])) {
                    (Some(row), Some(column)) => {
                        style.row_gap = Some(row);
                        style.column_gap = Some(column);
                    }
                    _ => bad(&rendered()),
                },
                _ => bad(&rendered()),
            }
        }
        "row-gap" => set!(row_gap, parse_length(value)),
        "column-gap" => set!(column_gap, parse_length(value)),

        "overflow" => {
            let parts = split_on_whitespace(value);
            match parts.len() {
                1 => match parse_overflow(&parts[0]) {
                    Some(v) => {
                        style.overflow_x = Some(v);
                        style.overflow_y = Some(v);
                    }
                    None => bad(&rendered()),
                },
                2 => match (parse_overflow(&parts[0]), parse_overflow(&parts[1])) {
                    (Some(x), Some(y)) => {
                        style.overflow_x = Some(x);
                        style.overflow_y = Some(y);
                    }
                    _ => bad(&rendered()),
                },
                _ => bad(&rendered()),
            }
        }
        "overflow-x" => set!(overflow_x, parse_overflow(value)),
        "overflow-y" => set!(overflow_y, parse_overflow(value)),

        // -- paint --
        "background" | "background-color" => set!(background, parse_paint(value)),
        "border" => {
            let mut width = None;
            let mut border_style = None;
            let mut color = None;
            let mut ok = true;
            for part in split_on_whitespace(value) {
                if let Some(v) = parse_px(&part) {
                    width = Some(v);
                } else if let Some(s) = parse_border_style(&part) {
                    border_style = Some(s);
                } else if let Some(c) = parse_color(&part) {
                    color = Some(c);
                } else {
                    ok = false;
                }
            }
            if !ok {
                bad(&rendered());
            } else {
                let width = width.unwrap_or(Px(1.0));
                style.border_top_width = Some(width);
                style.border_right_width = Some(width);
                style.border_bottom_width = Some(width);
                style.border_left_width = Some(width);
                style.border_style = Some(border_style.unwrap_or(BorderStyle::Solid));
                if let Some(c) = color {
                    style.border_color = Some(c);
                }
            }
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            // The same grammar as `border`, applied to one side only. Border
            // color is a single value for the whole box, so a per side color
            // sets all four; only sides with a non zero width actually draw.
            let mut width = None;
            let mut border_style = None;
            let mut color = None;
            let mut ok = true;
            for part in split_on_whitespace(value) {
                if let Some(v) = parse_px(&part) {
                    width = Some(v);
                } else if let Some(s) = parse_border_style(&part) {
                    border_style = Some(s);
                } else if let Some(c) = parse_color(&part) {
                    color = Some(c);
                } else {
                    ok = false;
                }
            }
            if !ok {
                bad(&rendered());
            } else {
                let width = width.unwrap_or(Px(1.0));
                match name {
                    "border-top" => style.border_top_width = Some(width),
                    "border-right" => style.border_right_width = Some(width),
                    "border-bottom" => style.border_bottom_width = Some(width),
                    _ => style.border_left_width = Some(width),
                }
                style.border_style = Some(border_style.unwrap_or(BorderStyle::Solid));
                if let Some(c) = color {
                    style.border_color = Some(c);
                }
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color"
        | "border-left-color" => set!(border_color, parse_color(value)),
        "border-width" => {
            let parts = split_on_whitespace(value);
            let widths: Option<Vec<Px>> = parts.iter().map(|p| parse_px(p)).collect();
            match widths.and_then(|w| expand_shorthand(&w)) {
                Some((t, r, b, l)) => {
                    style.border_top_width = Some(t);
                    style.border_right_width = Some(r);
                    style.border_bottom_width = Some(b);
                    style.border_left_width = Some(l);
                }
                None => bad(&rendered()),
            }
        }
        "border-top-width" => set!(border_top_width, parse_px(value)),
        "border-right-width" => set!(border_right_width, parse_px(value)),
        "border-bottom-width" => set!(border_bottom_width, parse_px(value)),
        "border-left-width" => set!(border_left_width, parse_px(value)),
        "border-color" => set!(border_color, parse_color(value)),
        "border-style" => set!(border_style, parse_border_style(value)),

        "border-radius" => {
            let parts = split_on_whitespace(value);
            let radii: Option<Vec<Px>> = parts.iter().map(|p| parse_px(p)).collect();
            // Radius shorthand runs top-left, top-right, bottom-right,
            // bottom-left, which is a different order from the edge shorthand.
            match radii.as_deref() {
                Some([a]) => {
                    style.radius_top_left = Some(*a);
                    style.radius_top_right = Some(*a);
                    style.radius_bottom_right = Some(*a);
                    style.radius_bottom_left = Some(*a);
                }
                Some([a, b]) => {
                    style.radius_top_left = Some(*a);
                    style.radius_bottom_right = Some(*a);
                    style.radius_top_right = Some(*b);
                    style.radius_bottom_left = Some(*b);
                }
                Some([a, b, c]) => {
                    style.radius_top_left = Some(*a);
                    style.radius_top_right = Some(*b);
                    style.radius_bottom_left = Some(*b);
                    style.radius_bottom_right = Some(*c);
                }
                Some([a, b, c, d]) => {
                    style.radius_top_left = Some(*a);
                    style.radius_top_right = Some(*b);
                    style.radius_bottom_right = Some(*c);
                    style.radius_bottom_left = Some(*d);
                }
                _ => bad(&rendered()),
            }
        }
        "border-top-left-radius" => set!(radius_top_left, parse_px(value)),
        "border-top-right-radius" => set!(radius_top_right, parse_px(value)),
        "border-bottom-right-radius" => set!(radius_bottom_right, parse_px(value)),
        "border-bottom-left-radius" => set!(radius_bottom_left, parse_px(value)),

        "box-shadow" => set!(box_shadows, parse_shadow_list(value)),
        "opacity" => set!(opacity, parse_number(value)),
        "transform" => match parse_transform(value) {
            // The origin is a separate property, so a later `transform` must
            // not undo one an earlier declaration already set.
            Some(parsed) => {
                let current = style.transform.get_or_insert(Transform2D::IDENTITY);
                current.translate_x = parsed.translate_x;
                current.translate_y = parsed.translate_y;
                current.scale_x = parsed.scale_x;
                current.scale_y = parsed.scale_y;
                current.rotate = parsed.rotate;
            }
            None => bad(&rendered()),
        },
        "transform-origin" => match parse_transform_origin(value) {
            Some((x, y)) => {
                let current = style.transform.get_or_insert(Transform2D::IDENTITY);
                current.origin_x = x;
                current.origin_y = y;
            }
            None => bad(&rendered()),
        },
        "cursor" => set!(cursor, parse_cursor(value)),
        "pointer-events" => set!(
            pointer_events,
            match single_ident(value).as_deref() {
                Some("none") => Some(false),
                Some("auto") => Some(true),
                _ => None,
            }
        ),

        // -- text --
        "color" => set!(color, parse_color(value)),
        "font-family" => set!(font_family, parse_font_family(value)),
        "font-size" => set!(font_size, parse_length(value)),
        "font-weight" => set!(font_weight, parse_font_weight(value)),
        "font-style" => set!(
            font_style,
            match single_ident(value).as_deref() {
                Some("normal") => Some(FontStyle::Normal),
                Some("italic") => Some(FontStyle::Italic),
                Some("oblique") => Some(FontStyle::Oblique),
                _ => None,
            }
        ),
        "line-height" => set!(
            line_height,
            match significant(value).first().map(|t| &t.kind) {
                Some(TokenKind::Ident(n)) if n.eq_ignore_ascii_case("normal") =>
                    Some(LineHeight::Normal),
                Some(TokenKind::Number(v)) => Some(LineHeight::Multiple(OrderedF32(*v))),
                Some(TokenKind::Percentage(v)) => Some(LineHeight::Multiple(OrderedF32(*v))),
                Some(TokenKind::Dimension(v, Unit::Px)) => Some(LineHeight::Px(Px(*v))),
                _ => None,
            }
        ),
        "letter-spacing" => set!(letter_spacing, parse_length(value)),
        "text-align" => set!(
            text_align,
            match single_ident(value).as_deref() {
                Some("left") | Some("start") => Some(TextAlign::Start),
                Some("center") => Some(TextAlign::Center),
                Some("right") | Some("end") => Some(TextAlign::End),
                Some("justify") => Some(TextAlign::Justify),
                _ => None,
            }
        ),
        "white-space" => set!(
            white_space,
            match single_ident(value).as_deref() {
                Some("normal") => Some(WhiteSpace::Normal),
                Some("nowrap") => Some(WhiteSpace::NoWrap),
                Some("pre") => Some(WhiteSpace::Pre),
                Some("pre-wrap") => Some(WhiteSpace::PreWrap),
                _ => None,
            }
        ),
        "text-decoration" | "text-decoration-line" => set!(
            text_decoration,
            match single_ident(value).as_deref() {
                Some("none") => Some(TextDecoration::NONE),
                Some("underline") => Some(TextDecoration {
                    underline: true,
                    strikethrough: false,
                }),
                Some("line-through") => Some(TextDecoration {
                    underline: false,
                    strikethrough: true,
                }),
                _ => None,
            }
        ),
        "text-overflow" => set!(
            text_overflow,
            match single_ident(value).as_deref() {
                Some("clip") => Some(TextOverflow::Clip),
                Some("ellipsis") => Some(TextOverflow::Ellipsis),
                _ => None,
            }
        ),

        // -- behaviour --
        "transition" => set!(transitions, parse_transition_list(value)),
        "transition-duration" => {
            let ms = parse_duration_ms(value);
            match ms {
                Some(ms) => {
                    let list = style.transitions.get_or_insert_with(|| smallvec![Transition::default()]);
                    for t in list.iter_mut() {
                        t.duration_ms = OrderedF32(ms);
                    }
                }
                None => bad(&rendered()),
            }
        }
        "transition-delay" => match parse_duration_ms(value) {
            Some(ms) => {
                let list = style.transitions.get_or_insert_with(|| smallvec![Transition::default()]);
                for t in list.iter_mut() {
                    t.delay_ms = OrderedF32(ms);
                }
            }
            None => bad(&rendered()),
        },
        "transition-timing-function" => match parse_easing(value) {
            Some(easing) => {
                let list = style.transitions.get_or_insert_with(|| smallvec![Transition::default()]);
                for t in list.iter_mut() {
                    t.easing = easing;
                }
            }
            None => bad(&rendered()),
        },

        _ => errors.push(ParseError {
            line,
            message: format!("unknown property `{name}`"),
        }),
    }
}

fn parse_justify(tokens: &[Token]) -> Option<JustifyContent> {
    Some(match single_ident(tokens).as_deref()? {
        "flex-start" | "start" => JustifyContent::Start,
        "center" => JustifyContent::Center,
        "flex-end" | "end" => JustifyContent::End,
        "space-between" => JustifyContent::SpaceBetween,
        "space-around" => JustifyContent::SpaceAround,
        "space-evenly" => JustifyContent::SpaceEvenly,
        "stretch" => JustifyContent::Stretch,
        _ => return None,
    })
}

fn parse_align(tokens: &[Token]) -> Option<AlignItems> {
    Some(match single_ident(tokens).as_deref()? {
        "flex-start" | "start" => AlignItems::Start,
        "center" => AlignItems::Center,
        "flex-end" | "end" => AlignItems::End,
        "stretch" => AlignItems::Stretch,
        "baseline" => AlignItems::Baseline,
        _ => return None,
    })
}

fn parse_overflow(tokens: &[Token]) -> Option<Overflow> {
    Some(match single_ident(tokens).as_deref()? {
        "visible" => Overflow::Visible,
        "hidden" | "clip" => Overflow::Hidden,
        "scroll" => Overflow::Scroll,
        "auto" => Overflow::Auto,
        _ => return None,
    })
}

fn parse_border_style(tokens: &[Token]) -> Option<BorderStyle> {
    Some(match single_ident(tokens).as_deref()? {
        "none" => BorderStyle::None,
        "solid" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        _ => return None,
    })
}

fn parse_cursor(tokens: &[Token]) -> Option<CursorStyle> {
    Some(match single_ident(tokens).as_deref()? {
        "default" | "arrow" => CursorStyle::Default,
        "pointer" => CursorStyle::Pointer,
        "text" => CursorStyle::Text,
        "crosshair" => CursorStyle::Crosshair,
        "move" => CursorStyle::Move,
        "grab" => CursorStyle::Grab,
        "grabbing" => CursorStyle::Grabbing,
        "not-allowed" => CursorStyle::NotAllowed,
        "wait" => CursorStyle::Wait,
        "ew-resize" => CursorStyle::ResizeHorizontal,
        "ns-resize" => CursorStyle::ResizeVertical,
        "nwse-resize" => CursorStyle::ResizeNwSe,
        "nesw-resize" => CursorStyle::ResizeNeSw,
        "col-resize" => CursorStyle::ColResize,
        "row-resize" => CursorStyle::RowResize,
        _ => return None,
    })
}

fn parse_font_weight(tokens: &[Token]) -> Option<FontWeight> {
    if let Some(n) = parse_number(tokens) {
        return Some(FontWeight(n.clamp(1.0, 1000.0) as u16));
    }
    Some(match single_ident(tokens).as_deref()? {
        "thin" => FontWeight::THIN,
        "extralight" | "extra-light" => FontWeight::EXTRA_LIGHT,
        "light" => FontWeight::LIGHT,
        "normal" | "regular" => FontWeight::NORMAL,
        "medium" => FontWeight::MEDIUM,
        "semibold" | "semi-bold" => FontWeight::SEMIBOLD,
        "bold" => FontWeight::BOLD,
        "extrabold" | "extra-bold" => FontWeight::EXTRA_BOLD,
        "black" | "heavy" => FontWeight::BLACK,
        _ => return None,
    })
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    /// The style one declaration produces.
    fn style_of(source: &str) -> Style {
        let sheet = Stylesheet::parse(&format!("x {{ {source} }}"));
        assert!(sheet.errors.is_empty(), "{:?}", sheet.errors);
        assert_eq!(sheet.rules.len(), 1, "expected one rule");
        (*sheet.rules[0].style).clone()
    }

    fn tracks(source: &str) -> Vec<TrackSize> {
        style_of(source)
            .grid_template_columns
            .expect("no columns parsed")
            .to_vec()
    }

    #[test]
    fn a_row_of_tracks_reads_left_to_right() {
        assert_eq!(
            tracks("grid-template-columns: 200px 1fr 2fr auto;"),
            [
                TrackSize::Px(Px(200.0)),
                TrackSize::Fr(1.0),
                TrackSize::Fr(2.0),
                TrackSize::Auto,
            ]
        );
    }

    #[test]
    fn percentages_and_content_sizes_are_understood() {
        assert_eq!(
            tracks("grid-template-columns: 25% min-content max-content;"),
            [
                TrackSize::Percent(0.25),
                TrackSize::MinContent,
                TrackSize::MaxContent,
            ]
        );
    }

    #[test]
    fn repeat_is_expanded_rather_than_passed_along() {
        // The layout engine only takes repetitions whose every track has a
        // fixed size, and `repeat(3, 1fr)` is most of what anybody writes.
        assert_eq!(
            tracks("grid-template-columns: repeat(3, 1fr);"),
            [TrackSize::Fr(1.0), TrackSize::Fr(1.0), TrackSize::Fr(1.0)]
        );
    }

    #[test]
    fn repeat_can_hold_more_than_one_track() {
        assert_eq!(
            tracks("grid-template-columns: repeat(2, 100px 1fr);"),
            [
                TrackSize::Px(Px(100.0)),
                TrackSize::Fr(1.0),
                TrackSize::Px(Px(100.0)),
                TrackSize::Fr(1.0),
            ]
        );
    }

    #[test]
    fn repeat_sits_alongside_ordinary_tracks() {
        assert_eq!(
            tracks("grid-template-columns: 200px repeat(2, 1fr) auto;"),
            [
                TrackSize::Px(Px(200.0)),
                TrackSize::Fr(1.0),
                TrackSize::Fr(1.0),
                TrackSize::Auto,
            ]
        );
    }

    #[test]
    fn a_placement_can_be_a_line_a_range_or_a_span() {
        assert_eq!(
            style_of("grid-column: 2;").grid_column,
            Some(GridLine {
                start: GridPlacement::Line(2),
                end: GridPlacement::Auto
            })
        );
        assert_eq!(
            style_of("grid-column: 2 / 5;").grid_column,
            Some(GridLine::between(2, 5))
        );
        assert_eq!(
            style_of("grid-column: span 3;").grid_column,
            Some(GridLine::span(3))
        );
        assert_eq!(
            style_of("grid-row: 1 / span 2;").grid_row,
            Some(GridLine {
                start: GridPlacement::Line(1),
                end: GridPlacement::Span(2)
            })
        );
    }

    #[test]
    fn a_line_counted_from_the_end_is_negative() {
        assert_eq!(
            style_of("grid-column: 1 / -1;").grid_column,
            Some(GridLine::between(1, -1))
        );
    }

    #[test]
    fn display_grid_is_understood() {
        assert_eq!(style_of("display: grid;").display, Some(Display::Grid));
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        for source in [
            "grid-template-columns: 1flr;",
            "grid-template-columns: sideways;",
            "grid-column: sideways;",
            "grid-column: span;",
        ] {
            let sheet = Stylesheet::parse(&format!("x {{ {source} }}"));
            assert!(
                !sheet.errors.is_empty(),
                "{source:?} was accepted without complaint"
            );
        }
    }
}
