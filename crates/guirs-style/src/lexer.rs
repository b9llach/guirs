//! Tokenizer for the guirs stylesheet language.
//!
//! The token model follows CSS closely enough that the syntax feels familiar,
//! but it is deliberately a fixed grammar rather than a general CSS engine:
//! there is no `@media`, no attribute selector namespacing and no unicode range
//! syntax. What is here is what the cascade actually consumes.

use std::fmt;

/// A lexical token together with the line it started on.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
}

impl Token {
    #[inline]
    pub fn is(&self, kind: &TokenKind) -> bool {
        &self.kind == kind
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// A bare identifier such as `flex` or `border-radius`.
    Ident(String),
    /// A custom property name, stored without the leading dashes.
    CustomProperty(String),
    /// `@import` and friends, stored without the `@`.
    AtKeyword(String),
    /// `#6c5ce7` or `#sidebar`. Disambiguated by the parser from context.
    Hash(String),
    /// A quoted string, with quotes and escapes removed.
    Str(String),
    /// A number with no unit.
    Number(f32),
    /// A number with a unit, such as `12px` or `150ms`.
    Dimension(f32, Unit),
    /// A percentage, stored as a fraction so `50%` is `0.5`.
    Percentage(f32),
    /// An identifier immediately followed by `(`. The paren is consumed.
    Function(String),
    /// Any punctuation that is not otherwise special.
    Delim(char),
    Comma,
    Colon,
    Semicolon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    /// One or more whitespace characters. Kept because a space between two
    /// compound selectors is the descendant combinator.
    Whitespace,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Ident(s) => write!(f, "{s}"),
            TokenKind::CustomProperty(s) => write!(f, "--{s}"),
            TokenKind::AtKeyword(s) => write!(f, "@{s}"),
            TokenKind::Hash(s) => write!(f, "#{s}"),
            TokenKind::Str(s) => write!(f, "\"{s}\""),
            TokenKind::Number(v) => write!(f, "{v}"),
            TokenKind::Dimension(v, u) => write!(f, "{v}{u}"),
            TokenKind::Percentage(v) => write!(f, "{}%", v * 100.0),
            TokenKind::Function(s) => write!(f, "{s}("),
            TokenKind::Delim(c) => write!(f, "{c}"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::LParen => write!(f, "("),
            TokenKind::RParen => write!(f, ")"),
            TokenKind::LBrace => write!(f, "{{"),
            TokenKind::RBrace => write!(f, "}}"),
            TokenKind::LBracket => write!(f, "["),
            TokenKind::RBracket => write!(f, "]"),
            TokenKind::Whitespace => write!(f, " "),
        }
    }
}

/// The unit attached to a dimension token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Logical pixels.
    Px,
    /// Multiples of the root font size.
    Rem,
    /// Degrees, for gradient angles.
    Deg,
    /// Milliseconds.
    Ms,
    /// Seconds.
    S,
    /// A share of the free space, as in `flex: 1fr`.
    Fr,
    /// A unit the lexer did not recognize. Kept so the parser can report it.
    Unknown,
}

impl Unit {
    fn from_str(s: &str) -> Unit {
        match s {
            "px" => Unit::Px,
            "rem" => Unit::Rem,
            "deg" => Unit::Deg,
            "ms" => Unit::Ms,
            "s" => Unit::S,
            "fr" => Unit::Fr,
            _ => Unit::Unknown,
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Unit::Px => "px",
            Unit::Rem => "rem",
            Unit::Deg => "deg",
            Unit::Ms => "ms",
            Unit::S => "s",
            Unit::Fr => "fr",
            Unit::Unknown => "?",
        };
        f.write_str(s)
    }
}

/// Turn stylesheet source into a token stream.
///
/// Lexing never fails. Anything unrecognized becomes a `Delim`, which the
/// parser reports with a line number and then recovers from.
pub fn tokenize(source: &str) -> Vec<Token> {
    Lexer::new(source).run()
}

struct Lexer<'a> {
    input: &'a [u8],
    source: &'a str,
    pos: usize,
    line: u32,
    out: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            input: source.as_bytes(),
            source,
            pos: 0,
            line: 1,
            out: Vec::new(),
        }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.input.get(self.pos + offset).copied()
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
        }
        Some(b)
    }

    fn push(&mut self, kind: TokenKind, line: u32) {
        self.out.push(Token { kind, line });
    }

    fn run(mut self) -> Vec<Token> {
        while let Some(b) = self.peek() {
            let line = self.line;
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.skip_whitespace();
                    self.push(TokenKind::Whitespace, line);
                }
                b'/' if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                b'/' if self.peek_at(1) == Some(b'/') => self.skip_line_comment(),
                b'"' | b'\'' => {
                    let s = self.read_string(b);
                    self.push(TokenKind::Str(s), line);
                }
                b'#' => {
                    self.bump();
                    let name = self.read_name();
                    self.push(TokenKind::Hash(name), line);
                }
                b'@' => {
                    self.bump();
                    let name = self.read_name();
                    self.push(TokenKind::AtKeyword(name), line);
                }
                b'-' if self.peek_at(1) == Some(b'-') => {
                    self.bump();
                    self.bump();
                    let name = self.read_name();
                    self.push(TokenKind::CustomProperty(name), line);
                }
                b'0'..=b'9' => self.read_numeric(line),
                b'.' | b'-' | b'+' if self.starts_number() => self.read_numeric(line),
                // A hyphen only begins an identifier when something follows it.
                // On its own it is punctuation.
                b'-' if !self.peek_at(1).is_some_and(is_name_continue) => {
                    self.bump();
                    self.push(TokenKind::Delim('-'), line);
                }
                b',' => {
                    self.bump();
                    self.push(TokenKind::Comma, line);
                }
                b':' => {
                    self.bump();
                    self.push(TokenKind::Colon, line);
                }
                b';' => {
                    self.bump();
                    self.push(TokenKind::Semicolon, line);
                }
                b'(' => {
                    self.bump();
                    self.push(TokenKind::LParen, line);
                }
                b')' => {
                    self.bump();
                    self.push(TokenKind::RParen, line);
                }
                b'{' => {
                    self.bump();
                    self.push(TokenKind::LBrace, line);
                }
                b'}' => {
                    self.bump();
                    self.push(TokenKind::RBrace, line);
                }
                b'[' => {
                    self.bump();
                    self.push(TokenKind::LBracket, line);
                }
                b']' => {
                    self.bump();
                    self.push(TokenKind::RBracket, line);
                }
                _ if is_name_start(b) => {
                    let name = self.read_name();
                    if self.peek() == Some(b'(') {
                        self.bump();
                        self.push(TokenKind::Function(name), line);
                    } else {
                        self.push(TokenKind::Ident(name), line);
                    }
                }
                _ => {
                    self.bump();
                    self.push(TokenKind::Delim(b as char), line);
                }
            }
        }
        self.out
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
        }
    }

    fn skip_block_comment(&mut self) {
        self.bump();
        self.bump();
        while let Some(b) = self.bump() {
            if b == b'*' && self.peek() == Some(b'/') {
                self.bump();
                return;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'\n' {
                return;
            }
            self.bump();
        }
    }

    fn read_string(&mut self, quote: u8) -> String {
        self.bump();
        let mut s = String::new();
        while let Some(b) = self.bump() {
            match b {
                b if b == quote => break,
                b'\\' => {
                    if let Some(escaped) = self.bump() {
                        s.push(escaped as char);
                    }
                }
                _ => {
                    // Rebuild multi byte characters from the original source.
                    if b < 0x80 {
                        s.push(b as char);
                    } else {
                        let start = self.pos - 1;
                        let end = next_char_boundary(self.source, start);
                        s.push_str(&self.source[start..end]);
                        self.pos = end;
                    }
                }
            }
        }
        s
    }

    fn read_name(&mut self) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_name_continue(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.source[start..self.pos].to_string()
    }

    /// Whether the sequence at the cursor begins a number rather than a delim.
    fn starts_number(&self) -> bool {
        match self.peek() {
            Some(b'.') => matches!(self.peek_at(1), Some(b'0'..=b'9')),
            Some(b'-') | Some(b'+') => match self.peek_at(1) {
                Some(b'0'..=b'9') => true,
                Some(b'.') => matches!(self.peek_at(2), Some(b'0'..=b'9')),
                _ => false,
            },
            Some(b'0'..=b'9') => true,
            _ => false,
        }
    }

    fn read_numeric(&mut self, line: u32) {
        let start = self.pos;
        if matches!(self.peek(), Some(b'-') | Some(b'+')) {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b'0'..=b'9')) {
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let value: f32 = self.source[start..self.pos].parse().unwrap_or(0.0);

        if self.peek() == Some(b'%') {
            self.pos += 1;
            self.push(TokenKind::Percentage(value / 100.0), line);
            return;
        }

        if self.peek().is_some_and(is_name_start) {
            let unit = self.read_name();
            self.push(
                TokenKind::Dimension(value, Unit::from_str(&unit.to_ascii_lowercase())),
                line,
            );
            return;
        }

        self.push(TokenKind::Number(value), line);
    }
}

#[inline]
fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'-' || b >= 0x80
}

#[inline]
fn is_name_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

fn next_char_boundary(s: &str, from: usize) -> usize {
    let mut i = from + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src)
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, TokenKind::Whitespace))
            .collect()
    }

    #[test]
    fn simple_rule_tokenizes() {
        assert_eq!(
            kinds("button { color: red; }"),
            vec![
                TokenKind::Ident("button".into()),
                TokenKind::LBrace,
                TokenKind::Ident("color".into()),
                TokenKind::Colon,
                TokenKind::Ident("red".into()),
                TokenKind::Semicolon,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn dimensions_carry_their_unit() {
        assert_eq!(
            kinds("8px 1.5rem 90deg 150ms 2s 1fr 50% 3"),
            vec![
                TokenKind::Dimension(8.0, Unit::Px),
                TokenKind::Dimension(1.5, Unit::Rem),
                TokenKind::Dimension(90.0, Unit::Deg),
                TokenKind::Dimension(150.0, Unit::Ms),
                TokenKind::Dimension(2.0, Unit::S),
                TokenKind::Dimension(1.0, Unit::Fr),
                TokenKind::Percentage(0.5),
                TokenKind::Number(3.0),
            ]
        );
    }

    #[test]
    fn negative_and_fractional_numbers_lex_as_numbers() {
        assert_eq!(
            kinds("-4px .5 -0.25rem"),
            vec![
                TokenKind::Dimension(-4.0, Unit::Px),
                TokenKind::Number(0.5),
                TokenKind::Dimension(-0.25, Unit::Rem),
            ]
        );
    }

    #[test]
    fn a_lone_hyphen_is_a_delim_not_a_number() {
        assert_eq!(kinds("-"), vec![TokenKind::Delim('-')]);
    }

    #[test]
    fn custom_properties_and_hashes_are_distinct() {
        assert_eq!(
            kinds("--primary: #6c5ce7;"),
            vec![
                TokenKind::CustomProperty("primary".into()),
                TokenKind::Colon,
                TokenKind::Hash("6c5ce7".into()),
                TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn functions_swallow_their_opening_paren() {
        assert_eq!(
            kinds("var(--x)"),
            vec![
                TokenKind::Function("var".into()),
                TokenKind::CustomProperty("x".into()),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn both_comment_styles_are_skipped() {
        assert_eq!(
            kinds("a /* block\ncomment */ b // line comment\nc"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Ident("b".into()),
                TokenKind::Ident("c".into()),
            ]
        );
    }

    #[test]
    fn unterminated_block_comment_does_not_hang() {
        assert_eq!(kinds("a /* never closed"), vec![TokenKind::Ident("a".into())]);
    }

    #[test]
    fn strings_handle_escapes_and_both_quote_styles() {
        assert_eq!(
            kinds(r#" "Fira Code" 'Segoe UI' "a\"b" "#),
            vec![
                TokenKind::Str("Fira Code".into()),
                TokenKind::Str("Segoe UI".into()),
                TokenKind::Str("a\"b".into()),
            ]
        );
    }

    #[test]
    fn non_ascii_survives_lexing() {
        assert_eq!(
            kinds("\"\u{65e5}\u{672c}\u{8a9e}\""),
            vec![TokenKind::Str("\u{65e5}\u{672c}\u{8a9e}".into())]
        );
    }

    #[test]
    fn line_numbers_track_newlines() {
        let tokens = tokenize("a\n\nb");
        let b = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Ident("b".into()))
            .unwrap();
        assert_eq!(b.line, 3);
    }

    #[test]
    fn whitespace_is_preserved_as_a_token() {
        let tokens = tokenize("a b");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[1].kind, TokenKind::Whitespace);
    }
}
