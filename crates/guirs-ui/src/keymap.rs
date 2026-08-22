//! What a key press means, and where it means it.
//!
//! Two things sit between a key and something happening. A **command** is what
//! an application can be asked to do, named rather than called: `"file:save"`
//! is a string, and whoever knows how to save decides to answer it. A
//! **keymap** says which keys ask for which command, and where.
//!
//! Naming the action rather than calling it is what makes the rest possible. A
//! menu item and a shortcut are then the same thing said twice, a menu can show
//! the shortcut without knowing what it does, and somebody can rebind a key
//! without touching the code that carries the action out.
//!
//! # Where a binding applies
//!
//! Bindings are scoped by context. A context is a name an element claims while
//! it or anything inside it has focus, so `Ctrl+F` can mean one thing in a text
//! editor and another in a file tree, and the same keymap holds both:
//!
//! ```
//! use guirs_ui::keymap::Keymap;
//!
//! let keymap = Keymap::new()
//!     .bind("cmd-s", "file:save")
//!     .bind_in("editor", "cmd-f", "editor:find")
//!     .bind_in("tree", "cmd-f", "tree:filter");
//! ```
//!
//! The innermost context that has something to say wins. A binding with no
//! context applies everywhere, and is only reached when nothing nearer claims
//! the key.
//!
//! # Sequences
//!
//! A binding can be more than one press. `"cmd-k cmd-d"` is two, and after the
//! first the keymap holds still and waits, which is what
//! [`Match::Pending`] reports.

use guirs_core::SharedString;

use crate::event::{Key, KeyEvent, Modifiers};

/// One press, as a binding describes it.
///
/// The accelerator is stored as itself rather than resolved to Control or
/// Command, so one keymap is right on every platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub key: Key,
    pub shift: bool,
    pub alt: bool,
    /// Control on Windows and Linux, Command on macOS. What a shortcut in a
    /// menu is written with.
    pub accelerator: bool,
    /// Control specifically, on a platform where that is not the accelerator.
    /// Rare, and only useful for a binding that means Control even on a Mac.
    pub control: bool,
}

impl Keystroke {
    /// Whether an actual press is this keystroke.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        if self.key != event.key {
            return false;
        }
        let modifiers = event.modifiers;
        if self.accelerator != modifiers.accelerator() {
            return false;
        }
        // Control is only checked separately where the platform draws a
        // distinction. Where Control *is* the accelerator, asking for both
        // would be asking for the same key twice.
        if !Modifiers::accelerator_is_control() && self.control != modifiers.control {
            return false;
        }
        if self.alt != modifiers.alt {
            return false;
        }
        // Shift is part of a keystroke for a named key, but for a character
        // the platform has already folded it into which character arrived, so
        // asking again would reject every capital letter.
        if matches!(self.key, Key::Character(_)) {
            return true;
        }
        self.shift == modifiers.shift
    }

    /// How this reads in a menu.
    ///
    /// Written the way the platform writes it, because a shortcut spelled the
    /// wrong way is worse than one not shown at all.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.accelerator {
            out.push_str(if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" });
        }
        if self.control && !Modifiers::accelerator_is_control() {
            out.push_str("Ctrl+");
        }
        if self.alt {
            out.push_str(if cfg!(target_os = "macos") { "Opt+" } else { "Alt+" });
        }
        if self.shift {
            out.push_str("Shift+");
        }
        out.push_str(&key_name(self.key));
        out
    }
}

fn key_name(key: Key) -> String {
    match key {
        Key::Escape => "Esc".into(),
        Key::Enter => "Enter".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Del".into(),
        Key::Insert => "Ins".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PgUp".into(),
        Key::PageDown => "PgDn".into(),
        Key::Left => "Left".into(),
        Key::Right => "Right".into(),
        Key::Up => "Up".into(),
        Key::Down => "Down".into(),
        Key::Space => "Space".into(),
        Key::Function(n) => format!("F{n}"),
        Key::Character(c) => c.to_uppercase().to_string(),
        Key::Unknown => "?".into(),
    }
}

/// What went wrong reading a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing but whitespace.
    Empty,
    /// A word that is not a modifier and not a key.
    UnknownKey(String),
    /// Modifiers with no key after them, as in `"ctrl-"`.
    NoKey(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => f.write_str("a binding cannot be empty"),
            ParseError::UnknownKey(word) => write!(f, "no key called {word:?}"),
            ParseError::NoKey(word) => write!(f, "{word:?} names modifiers but no key"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Read one press, as written in a keymap.
///
/// Modifiers are joined to the key with `-`: `"cmd-shift-p"`, `"alt-f4"`,
/// `"ctrl-k"`. `cmd`, `super` and `win` all mean the platform's own
/// accelerator, so a keymap written once is right everywhere.
pub fn keystroke(text: &str) -> Result<Keystroke, ParseError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ParseError::Empty);
    }

    let mut stroke = Keystroke {
        key: Key::Unknown,
        shift: false,
        alt: false,
        accelerator: false,
        control: false,
    };

    // The last part is the key and everything before it is a modifier, which
    // is why `-` on its own has to be spelled `minus`: splitting on it would
    // otherwise leave an empty key.
    let parts: Vec<&str> = text.split('-').collect();
    let (key, modifiers) = parts.split_last().expect("split always yields one part");

    for modifier in modifiers {
        match modifier.to_ascii_lowercase().as_str() {
            "shift" => stroke.shift = true,
            "alt" | "opt" | "option" => stroke.alt = true,
            "cmd" | "command" | "super" | "win" | "meta" => stroke.accelerator = true,
            // Control means the accelerator where Control is the accelerator,
            // so that "ctrl-c" written by hand does the expected thing on
            // Windows and Linux without a second binding for macOS.
            "ctrl" | "control" => {
                if Modifiers::accelerator_is_control() {
                    stroke.accelerator = true;
                } else {
                    stroke.control = true;
                }
            }
            other => return Err(ParseError::UnknownKey(other.to_string())),
        }
    }

    if key.is_empty() {
        return Err(ParseError::NoKey(text.to_string()));
    }
    stroke.key = named_key(key).ok_or_else(|| ParseError::UnknownKey((*key).to_string()))?;
    Ok(stroke)
}

fn named_key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "escape" | "esc" => Key::Escape,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "space" => Key::Space,
        // Spelled out, because the separator between modifiers and key is
        // itself `-` and splitting on it would leave nothing behind.
        "minus" => Key::Character('-'),
        "plus" => Key::Character('+'),
        "equals" | "equal" => Key::Character('='),
        "comma" => Key::Character(','),
        "period" | "dot" => Key::Character('.'),
        "slash" => Key::Character('/'),
        "backslash" => Key::Character('\\'),
        "semicolon" => Key::Character(';'),
        "quote" => Key::Character('\''),
        "backtick" | "grave" => Key::Character('`'),
        "leftbracket" => Key::Character('['),
        "rightbracket" => Key::Character(']'),
        _ => {
            if let Some(number) = lower.strip_prefix('f') {
                if let Ok(n) = number.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(Key::Function(n));
                    }
                }
            }
            let mut characters = lower.chars();
            let first = characters.next()?;
            if characters.next().is_some() {
                return None;
            }
            Key::Character(first)
        }
    })
}

/// One binding: a sequence of presses, a command, and where it applies.
#[derive(Clone, Debug)]
pub struct Binding {
    pub strokes: Vec<Keystroke>,
    pub command: SharedString,
    /// The context this belongs to, or `None` for everywhere.
    pub context: Option<SharedString>,
}

/// What a keymap made of a press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    /// Nothing here wants this press. It belongs to whatever would have had it
    /// anyway, which for a printable key is usually the text under the caret.
    None,
    /// The start of a longer binding. The press is spoken for, and the next
    /// one decides what happens.
    Pending,
    /// Run this.
    Command(SharedString),
}

/// Which keys ask for which commands.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    pub fn new() -> Self {
        Keymap::default()
    }

    /// Bind a key, or a sequence of them, everywhere.
    ///
    /// Panics if the binding cannot be read, because a keymap is written by
    /// hand at startup and a typo in one is a mistake to fix rather than a
    /// condition to handle. Use [`Keymap::try_bind`] for a keymap read from a
    /// file, where the text is not the author's.
    pub fn bind(self, keys: &str, command: impl Into<SharedString>) -> Self {
        self.try_bind(None, keys, command)
            .unwrap_or_else(|error| panic!("{keys:?}: {error}"))
    }

    /// Bind a key inside one context.
    pub fn bind_in(self, context: &str, keys: &str, command: impl Into<SharedString>) -> Self {
        self.try_bind(Some(context), keys, command)
            .unwrap_or_else(|error| panic!("{keys:?}: {error}"))
    }

    /// Bind a key, reporting anything wrong with it rather than panicking.
    pub fn try_bind(
        mut self,
        context: Option<&str>,
        keys: &str,
        command: impl Into<SharedString>,
    ) -> Result<Self, ParseError> {
        let strokes = keys
            .split_whitespace()
            .map(keystroke)
            .collect::<Result<Vec<_>, _>>()?;
        if strokes.is_empty() {
            return Err(ParseError::Empty);
        }
        self.bindings.push(Binding {
            strokes,
            command: command.into(),
            context: context.map(SharedString::from),
        });
        Ok(self)
    }

    /// Take everything from another keymap, letting it win where they disagree.
    ///
    /// This is how somebody's own bindings sit on top of the defaults: both
    /// keymaps are complete, and the later one is consulted first.
    pub fn extend(mut self, other: Keymap) -> Self {
        self.bindings.extend(other.bindings);
        self
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// What a press means, given where focus is and what came before it.
    ///
    /// `contexts` runs outermost first, the way the tree does, so the last
    /// entry is the innermost thing focused. `pending` is whatever earlier
    /// presses have not yet resolved.
    pub fn resolve(&self, contexts: &[SharedString], pending: &[Keystroke], event: &KeyEvent) -> Match {
        let sequence: &[Keystroke] = pending;

        let mut best: Option<(usize, &Binding)> = None;
        let mut pending_found = false;

        for binding in &self.bindings {
            let depth = match &binding.context {
                None => 0,
                Some(context) => match contexts.iter().position(|c| c == context) {
                    // Innermost wins, so a deeper context scores higher. One
                    // is added so that any context beats no context at all.
                    Some(index) => index + 1,
                    None => continue,
                },
            };

            if binding.strokes.len() <= sequence.len() {
                continue;
            }
            // Everything already pressed has to line up, or this binding is
            // not the one being typed.
            if !binding
                .strokes
                .iter()
                .zip(sequence)
                .all(|(stroke, done)| stroke == done)
            {
                continue;
            }
            if !binding.strokes[sequence.len()].matches(event) {
                continue;
            }

            if binding.strokes.len() > sequence.len() + 1 {
                // Longer than what has been typed: this press is spoken for,
                // but nothing runs yet.
                pending_found = true;
                continue;
            }

            // A complete match. Deeper context wins, and among equals the one
            // bound later, so somebody's own keymap overrides the defaults.
            match best {
                Some((best_depth, _)) if best_depth > depth => {}
                _ => best = Some((depth, binding)),
            }
        }

        // A longer binding that this press could be the start of wins over a
        // shorter one it completes. Binding both `cmd-k` and `cmd-k cmd-d`
        // means the first has to wait, because running it immediately makes
        // the second unreachable, and silently losing a binding is worse than
        // making one wait for a key that never comes.
        if pending_found {
            return Match::Pending;
        }
        if let Some((_, binding)) = best {
            return Match::Command(binding.command.clone());
        }
        Match::None
    }
}

/// The keystroke an actual press amounts to.
///
/// Used to remember what has been typed while a sequence is part way through.
pub fn strokes_of(event: &KeyEvent) -> Keystroke {
    Keystroke {
        key: event.key,
        shift: event.modifiers.shift && !matches!(event.key, Key::Character(_)),
        alt: event.modifiers.alt,
        accelerator: event.modifiers.accelerator(),
        control: if Modifiers::accelerator_is_control() {
            false
        } else {
            event.modifiers.control
        },
    }
}

/// The stack of contexts focus is currently inside, and any half typed
/// sequence.
///
/// Held by the window across frames, because a sequence is by definition not
/// finished within one.
#[derive(Clone, Debug, Default)]
pub struct KeymapState {
    pending: Vec<Keystroke>,
}

impl KeymapState {
    /// Feed one press through a keymap.
    pub fn press(
        &mut self,
        keymap: &Keymap,
        contexts: &[SharedString],
        event: &KeyEvent,
    ) -> Match {
        let outcome = keymap.resolve(contexts, &self.pending, event);
        match &outcome {
            Match::Pending => self.pending.push(strokes_of(event)),
            // Anything that resolves, one way or the other, ends the sequence.
            // A half typed sequence that is abandoned must not silently change
            // what the next unrelated press means.
            Match::Command(_) | Match::None => self.pending.clear(),
        }
        outcome
    }

    /// Whether a sequence is part way through.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// What has been typed so far, for showing in a status bar.
    pub fn pending(&self) -> &[Keystroke] {
        &self.pending
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, modifiers: Modifiers) -> KeyEvent {
        KeyEvent { key, modifiers, repeat: false }
    }

    fn accel() -> Modifiers {
        let mut m = Modifiers::default();
        if Modifiers::accelerator_is_control() {
            m.control = true;
        } else {
            m.meta = true;
        }
        m
    }

    #[test]
    fn a_binding_reads_the_way_it_is_written() {
        let stroke = keystroke("cmd-shift-p").expect("did not parse");
        assert!(stroke.accelerator);
        assert!(stroke.shift);
        assert!(!stroke.alt);
        assert_eq!(stroke.key, Key::Character('p'));
    }

    #[test]
    fn the_accelerator_is_whatever_this_platform_uses() {
        // One keymap, right on every platform: `cmd` and `ctrl` both mean the
        // key a shortcut is actually taken with here.
        let cmd = keystroke("cmd-s").unwrap();
        let ctrl = keystroke("ctrl-s").unwrap();
        if Modifiers::accelerator_is_control() {
            assert_eq!(cmd, ctrl, "on this platform both mean Control");
        } else {
            assert!(cmd.accelerator && !cmd.control);
            assert!(ctrl.control && !ctrl.accelerator);
        }
    }

    #[test]
    fn named_keys_are_understood() {
        assert_eq!(keystroke("f11").unwrap().key, Key::Function(11));
        assert_eq!(keystroke("escape").unwrap().key, Key::Escape);
        assert_eq!(keystroke("pgdn").unwrap().key, Key::PageDown);
        // The separator is a hyphen, so a hyphen has to be spelled out or the
        // key would come out empty.
        assert_eq!(keystroke("minus").unwrap().key, Key::Character('-'));
        assert_eq!(keystroke("cmd-minus").unwrap().key, Key::Character('-'));
    }

    #[test]
    fn nonsense_is_reported_rather_than_guessed_at() {
        assert!(keystroke("").is_err());
        assert!(keystroke("ctrl-").is_err());
        assert!(keystroke("hyper-x").is_err());
        assert!(keystroke("notakey").is_err());
    }

    #[test]
    fn a_bound_key_resolves_to_its_command() {
        let keymap = Keymap::new().bind("cmd-s", "file:save");
        let outcome = keymap.resolve(&[], &[], &press(Key::Character('s'), accel()));
        assert_eq!(outcome, Match::Command("file:save".into()));
    }

    #[test]
    fn an_unbound_key_is_left_alone() {
        // Anything not claimed has to fall through, or typing into a field
        // would stop working the moment a keymap existed.
        let keymap = Keymap::new().bind("cmd-s", "file:save");
        let outcome = keymap.resolve(&[], &[], &press(Key::Character('q'), Modifiers::default()));
        assert_eq!(outcome, Match::None);
    }

    #[test]
    fn the_innermost_context_wins() {
        let keymap = Keymap::new()
            .bind("cmd-f", "window:find")
            .bind_in("editor", "cmd-f", "editor:find")
            .bind_in("tree", "cmd-f", "tree:filter");

        let event = press(Key::Character('f'), accel());
        let in_editor: Vec<SharedString> = vec!["workspace".into(), "editor".into()];
        assert_eq!(
            keymap.resolve(&in_editor, &[], &event),
            Match::Command("editor:find".into())
        );

        let in_tree: Vec<SharedString> = vec!["workspace".into(), "tree".into()];
        assert_eq!(
            keymap.resolve(&in_tree, &[], &event),
            Match::Command("tree:filter".into())
        );

        // Nowhere in particular falls back to the binding with no context.
        assert_eq!(keymap.resolve(&[], &[], &event), Match::Command("window:find".into()));
    }

    #[test]
    fn a_context_that_is_not_focused_does_not_apply() {
        let keymap = Keymap::new().bind_in("editor", "cmd-f", "editor:find");
        let event = press(Key::Character('f'), accel());
        let elsewhere: Vec<SharedString> = vec!["tree".into()];
        assert_eq!(keymap.resolve(&elsewhere, &[], &event), Match::None);
    }

    #[test]
    fn a_later_binding_overrides_an_earlier_one() {
        // Somebody's own keymap sits on top of the defaults.
        let defaults = Keymap::new().bind("cmd-p", "file:open");
        let theirs = Keymap::new().bind("cmd-p", "command:palette");
        let keymap = defaults.extend(theirs);
        assert_eq!(
            keymap.resolve(&[], &[], &press(Key::Character('p'), accel())),
            Match::Command("command:palette".into())
        );
    }

    #[test]
    fn a_sequence_waits_for_the_rest_of_itself() {
        let keymap = Keymap::new().bind("cmd-k cmd-d", "editor:duplicate");
        let mut state = KeymapState::default();

        let first = state.press(&keymap, &[], &press(Key::Character('k'), accel()));
        assert_eq!(first, Match::Pending);
        assert!(state.is_pending(), "the first press was forgotten");

        let second = state.press(&keymap, &[], &press(Key::Character('d'), accel()));
        assert_eq!(second, Match::Command("editor:duplicate".into()));
        assert!(!state.is_pending(), "the sequence did not finish");
    }

    #[test]
    fn an_abandoned_sequence_does_not_change_the_next_press() {
        // Half of a sequence followed by something unrelated. The unrelated
        // press must mean what it always means, and the half must be dropped.
        let keymap = Keymap::new()
            .bind("cmd-k cmd-d", "editor:duplicate")
            .bind("cmd-s", "file:save");
        let mut state = KeymapState::default();

        assert_eq!(
            state.press(&keymap, &[], &press(Key::Character('k'), accel())),
            Match::Pending
        );
        assert_eq!(
            state.press(&keymap, &[], &press(Key::Character('x'), accel())),
            Match::None,
            "an unrelated press was swallowed"
        );
        assert!(!state.is_pending());
        assert_eq!(
            state.press(&keymap, &[], &press(Key::Character('s'), accel())),
            Match::Command("file:save".into()),
            "the next press was still being read as part of the sequence"
        );
    }

    #[test]
    fn a_shorter_binding_does_not_swallow_a_longer_one() {
        // Both are bound and the first press belongs to both. Running the
        // short one straight away would make the long one unreachable, so the
        // short one waits.
        let keymap = Keymap::new()
            .bind("cmd-k", "pane:close")
            .bind("cmd-k cmd-d", "editor:duplicate");
        let mut state = KeymapState::default();

        assert_eq!(
            state.press(&keymap, &[], &press(Key::Character('k'), accel())),
            Match::Pending
        );
        assert_eq!(
            state.press(&keymap, &[], &press(Key::Character('d'), accel())),
            Match::Command("editor:duplicate".into()),
            "the longer binding was unreachable"
        );
    }

    #[test]
    fn shift_is_not_asked_for_twice_on_a_letter() {
        // The platform reports a capital as a different character, so a
        // binding that also demanded shift would never match one.
        let keymap = Keymap::new().bind("cmd-shift-p", "command:palette");
        let mut modifiers = accel();
        modifiers.shift = true;
        assert_eq!(
            keymap.resolve(&[], &[], &press(Key::Character('p'), modifiers)),
            Match::Command("command:palette".into())
        );
    }

    #[test]
    fn shift_is_part_of_a_named_key() {
        let keymap = Keymap::new().bind("shift-tab", "focus:previous");
        let modifiers = Modifiers { shift: true, ..Default::default() };
        assert_eq!(
            keymap.resolve(&[], &[], &press(Key::Tab, modifiers)),
            Match::Command("focus:previous".into())
        );
        // And plain Tab is a different thing entirely.
        assert_eq!(
            keymap.resolve(&[], &[], &press(Key::Tab, Modifiers::default())),
            Match::None
        );
    }

    #[test]
    fn a_shortcut_reads_the_way_the_platform_writes_it() {
        let shown = keystroke("cmd-shift-p").unwrap().display();
        if cfg!(target_os = "macos") {
            assert_eq!(shown, "Cmd+Shift+P");
        } else {
            assert_eq!(shown, "Ctrl+Shift+P");
        }
        assert_eq!(keystroke("f5").unwrap().display(), "F5");
    }
}
