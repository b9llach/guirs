//! Just enough markdown to make a reply read like one.
//!
//! This lives in the application rather than the framework on purpose. The
//! framework's job is rich text: a string with styled spans over it. Deciding
//! that two asterisks mean bold is a decision about a document format, and a
//! different application would make it differently.
//!
//! What is recognised: `**bold**`, `*italic*`, `` `code` ``, `[label](target)`
//! and bare `http` links. Anything unrecognised is left exactly as typed, which
//! matters while a reply is still arriving: half of `**bold` is not a broken
//! parse, it is text that has not finished being written yet.

use guirs::{rich, Rgba, RichText, RichTextBuilder, SpanStyle};

/// The colors the styled spans use.
///
/// Read from the stylesheet rather than hardcoded, so a change of theme reaches
/// inline code and links the same way it reaches everything else.
#[derive(Clone, Copy, Debug)]
pub struct MarkdownTheme {
    pub code_text: Rgba,
    pub code_background: Rgba,
    pub link: Rgba,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        MarkdownTheme {
            code_text: Rgba::rgba8(226, 232, 240, 255),
            code_background: Rgba::rgba8(255, 255, 255, 20),
            link: Rgba::rgba8(129, 161, 255, 255),
        }
    }
}

/// Convert a reply into rich text.
pub fn to_rich(source: &str, theme: &MarkdownTheme) -> RichText {
    let mut builder = rich("");
    let bytes = source.as_bytes();
    let mut plain_from = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        // A marker only counts if its closing partner is on the same line. That
        // is what stops a lone asterisk in prose from swallowing a paragraph.
        let found = match bytes[index] {
            b'*' if starts_with(bytes, index, b"**") => close(bytes, index + 2, b"**")
                .filter(|end| hugs_its_text(bytes, index + 2, *end))
                .map(|end| (Mark::Bold, index + 2, end, end + 2)),
            b'*' => close(bytes, index + 1, b"*")
                .filter(|end| hugs_its_text(bytes, index + 1, *end))
                .map(|end| (Mark::Italic, index + 1, end, end + 1)),
            b'`' => close(bytes, index + 1, b"`").map(|end| (Mark::Code, index + 1, end, end + 1)),
            b'[' => link_at(source, index),
            b'h' if starts_with(bytes, index, b"http") => bare_link_at(source, index),
            _ => None,
        };

        let Some((mark, inner_start, inner_end, after)) = found else {
            index += 1;
            continue;
        };

        builder = builder.push(&source[plain_from..index]);
        let inner = &source[inner_start..inner_end];
        builder = match mark {
            Mark::Bold => builder.bold(inner),
            Mark::Italic => builder.italic(inner),
            Mark::Code => builder.styled(
                inner,
                SpanStyle::default()
                    .with_family(["monospace", "Consolas", "Cascadia Mono"])
                    .with_scale(0.92)
                    .with_color(theme.code_text)
                    .with_background(theme.code_background),
            ),
            Mark::Link { target } => link_span(builder, inner, &source[target], theme),
        };

        plain_from = after;
        index = after;
    }

    builder.push(&source[plain_from..]).build()
}

fn link_span(
    builder: RichTextBuilder,
    label: &str,
    target: &str,
    theme: &MarkdownTheme,
) -> RichTextBuilder {
    builder.styled(
        label,
        SpanStyle::underline()
            .with_color(theme.link)
            .with_link(target),
    )
}

enum Mark {
    Bold,
    Italic,
    Code,
    Link { target: std::ops::Range<usize> },
}

/// Whether an emphasis marker sits directly against the text it marks.
///
/// Without this rule, arithmetic becomes italics: the two asterisks in
/// "2 * 3 * 4" pair up and swallow the three. Requiring the marker to touch
/// its content is what tells the two apart, and it is the rule markdown itself
/// uses.
fn hugs_its_text(bytes: &[u8], start: usize, end: usize) -> bool {
    start < end
        && !bytes[start].is_ascii_whitespace()
        && !bytes[end - 1].is_ascii_whitespace()
}

fn starts_with(bytes: &[u8], at: usize, prefix: &[u8]) -> bool {
    bytes.len() >= at + prefix.len() && &bytes[at..at + prefix.len()] == prefix
}

/// Where the closing marker is, if it is on this line and encloses something.
fn close(bytes: &[u8], from: usize, marker: &[u8]) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            return None;
        }
        if starts_with(bytes, index, marker) {
            return if index > from { Some(index) } else { None };
        }
        index += 1;
    }
    None
}

/// A `[label](target)` starting at `at`.
fn link_at(source: &str, at: usize) -> Option<(Mark, usize, usize, usize)> {
    let bytes = source.as_bytes();
    let label_end = close(bytes, at + 1, b"]")?;
    if !starts_with(bytes, label_end + 1, b"(") {
        return None;
    }
    let target_start = label_end + 2;
    let target_end = close(bytes, target_start, b")")?;
    Some((
        Mark::Link {
            target: target_start..target_end,
        },
        at + 1,
        label_end,
        target_end + 1,
    ))
}

/// A bare link, which runs to the first space or trailing punctuation.
fn bare_link_at(source: &str, at: usize) -> Option<(Mark, usize, usize, usize)> {
    let bytes = source.as_bytes();
    if !starts_with(bytes, at, b"http://") && !starts_with(bytes, at, b"https://") {
        return None;
    }
    let mut end = at;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    // A sentence ending in a link should not swallow the full stop.
    while end > at && matches!(bytes[end - 1], b'.' | b',' | b')' | b']' | b'!' | b'?') {
        end -= 1;
    }
    if end <= at + 8 {
        return None;
    }
    Some((Mark::Link { target: at..end }, at, end, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_of(source: &str) -> RichText {
        to_rich(source, &MarkdownTheme::default())
    }

    #[test]
    fn markers_are_removed_and_the_text_between_them_is_styled() {
        let built = spans_of("a **bold** word");
        assert_eq!(built.as_str(), "a bold word");
        assert_eq!(built.spans.len(), 1);
        assert_eq!(&built.as_str()[built.spans[0].range.clone()], "bold");
    }

    #[test]
    fn italic_and_code_are_recognised_too() {
        let built = spans_of("*slanted* and `let x = 1;` here");
        assert_eq!(built.as_str(), "slanted and let x = 1; here");
        assert_eq!(built.spans.len(), 2);
        assert_eq!(&built.as_str()[built.spans[1].range.clone()], "let x = 1;");
        assert!(built.spans[1].style.background.is_some());
    }

    #[test]
    fn an_unclosed_marker_is_left_alone() {
        // This is what half an arriving reply looks like.
        assert_eq!(spans_of("a **bold").as_str(), "a **bold");
        assert_eq!(spans_of("2 * 3 * 4 is").as_str(), "2 * 3 * 4 is");
        assert!(spans_of("2 * 3 * 4 is").spans.is_empty());
        assert!(spans_of("a **bold").spans.is_empty());

        // A real pair later in the line is still found.
        let mixed = spans_of("a * b *c* d");
        assert_eq!(mixed.as_str(), "a * b c d");
        assert_eq!(&mixed.as_str()[mixed.spans[0].range.clone()], "c");
    }

    #[test]
    fn a_marker_does_not_reach_across_a_line() {
        let source = "one *start\nend* two";
        assert_eq!(spans_of(source).as_str(), source);
    }

    #[test]
    fn an_empty_pair_is_not_a_span() {
        assert_eq!(spans_of("a **** b").as_str(), "a **** b");
        assert!(spans_of("a `` b").spans.is_empty());
    }

    #[test]
    fn a_labelled_link_keeps_its_label_and_its_target() {
        let built = spans_of("see [the docs](https://example.com) now");
        assert_eq!(built.as_str(), "see the docs now");
        let span = &built.spans[0];
        assert_eq!(&built.as_str()[span.range.clone()], "the docs");
        assert_eq!(
            span.style.link.as_ref().map(|href| href.as_str()),
            Some("https://example.com")
        );
    }

    #[test]
    fn a_bare_link_becomes_its_own_target() {
        let built = spans_of("go to https://example.com/x for more");
        assert_eq!(built.as_str(), "go to https://example.com/x for more");
        assert_eq!(
            built.spans[0].style.link.as_ref().map(|href| href.as_str()),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn a_link_ending_a_sentence_does_not_swallow_the_full_stop() {
        let built = spans_of("read https://example.com/page.");
        assert_eq!(built.as_str(), "read https://example.com/page.");
        assert_eq!(
            &built.as_str()[built.spans[0].range.clone()],
            "https://example.com/page"
        );
    }

    #[test]
    fn something_that_only_looks_like_a_link_is_left_alone() {
        assert!(spans_of("http not a link").spans.is_empty());
        assert!(spans_of("[label] with no target").spans.is_empty());
    }

    #[test]
    fn plain_prose_produces_no_spans_at_all() {
        let built = spans_of("nothing marked up in here");
        assert!(built.is_plain());
        assert_eq!(built.as_str(), "nothing marked up in here");
    }

    #[test]
    fn every_prefix_of_a_reply_converts_without_panicking() {
        // A reply is revealed a byte at a time, so every prefix of it is asked
        // to render at some point, including the ones that cut a marker or a
        // multi byte character in half.
        let source = "**bold** and `code` and [a link](https://example.com) \u{2014} caf\u{00e9}";
        for end in 0..=source.len() {
            if !source.is_char_boundary(end) {
                continue;
            }
            let built = spans_of(&source[..end]);
            // Every span has to land on a character boundary of the result.
            for span in built.spans.iter() {
                assert!(built.as_str().is_char_boundary(span.range.start));
                assert!(built.as_str().is_char_boundary(span.range.end));
            }
        }
    }
}
