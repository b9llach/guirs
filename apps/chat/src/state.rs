//! What the application knows.
//!
//! Kept apart from the views on purpose: the view functions read this and
//! return elements, and nothing in the element tree owns anything. That is what
//! makes the tree safe to throw away and rebuild every frame.

use guirs::SharedString;

/// Who said something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    You,
    Assistant,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::You => "You",
            Role::Assistant => "guirs",
        }
    }
}

/// One turn in a conversation.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// How much of `text` has been revealed so far.
    ///
    /// A reply arrives a piece at a time rather than all at once, which is
    /// both what people expect from a chat and a decent exercise of the
    /// animation path.
    pub revealed: usize,
}

impl Message {
    pub fn complete(role: Role, text: impl Into<String>) -> Self {
        let text = text.into();
        Message {
            role,
            revealed: text.len(),
            text,
        }
    }

    pub fn streaming(role: Role, text: impl Into<String>) -> Self {
        Message {
            role,
            text: text.into(),
            revealed: 0,
        }
    }

    #[inline]
    pub fn is_streaming(&self) -> bool {
        self.revealed < self.text.len()
    }

    /// The part of the message that should be on screen.
    pub fn visible(&self) -> &str {
        // Always land on a character boundary, or a reply containing anything
        // outside ASCII panics partway through being revealed.
        let mut end = self.revealed.min(self.text.len());
        while end > 0 && !self.text.is_char_boundary(end) {
            end -= 1;
        }
        &self.text[..end]
    }

    /// Reveal up to `target` bytes, snapped to a character boundary.
    pub fn reveal_to(&mut self, target: usize) {
        let mut target = target.min(self.text.len());
        while target > 0 && !self.text.is_char_boundary(target) {
            target += 1;
            if target >= self.text.len() {
                target = self.text.len();
                break;
            }
        }
        self.revealed = self.revealed.max(target);
    }
}

/// One conversation.
#[derive(Clone, Debug)]
pub struct Conversation {
    pub title: SharedString,
    pub messages: Vec<Message>,
    /// When the reply currently arriving started, in seconds.
    pub streaming_since: Option<f64>,
}

impl Conversation {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Conversation {
            title: title.into(),
            messages: Vec::new(),
            streaming_since: None,
        }
    }

    pub fn preview(&self) -> String {
        match self.messages.last() {
            Some(message) => {
                // The markup goes through the same reader the transcript uses,
                // so a preview shows the words rather than the asterisks
                // around them. Anything the reader does not recognise stays as
                // it was typed, which is what should happen to a message that
                // is genuinely about asterisks.
                let rendered = crate::markdown::to_rich(&message.text, &Default::default());
                let text = rendered.as_str().replace('\n', " ");
                let trimmed: String = text.chars().take(46).collect();
                if text.chars().count() > 46 {
                    format!("{trimmed}\u{2026}")
                } else {
                    trimmed
                }
            }
            None => "No messages yet".into(),
        }
    }
}

/// Everything the window shows.
#[derive(Debug)]
pub struct Chat {
    pub conversations: Vec<Conversation>,
    pub active: usize,
    /// Whether the sidebar is showing.
    pub sidebar_open: bool,
}

impl Default for Chat {
    fn default() -> Self {
        Chat::new()
    }
}

impl Chat {
    pub fn new() -> Self {
        // Opens on an empty conversation, which is the state the reference
        // this was laid out against shows.
        Chat {
            conversations: vec![Conversation::new("New chat")],
            active: 0,
            sidebar_open: true,
        }
    }

    pub fn current(&self) -> &Conversation {
        &self.conversations[self.active.min(self.conversations.len() - 1)]
    }

    pub fn current_mut(&mut self) -> &mut Conversation {
        let index = self.active.min(self.conversations.len() - 1);
        &mut self.conversations[index]
    }

    /// Start a new conversation and switch to it.
    pub fn new_conversation(&mut self) {
        self.conversations.push(Conversation::new("New chat"));
        self.active = self.conversations.len() - 1;
    }

    /// Record what was typed and queue a reply.
    pub fn send(&mut self, text: &str, now: f64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }

        let reply = reply_to(text);
        let conversation = self.current_mut();

        // The first thing said in a conversation names it.
        if conversation.messages.is_empty() {
            let title: String = text.chars().take(28).collect();
            conversation.title = SharedString::from(title.as_str());
        }

        conversation
            .messages
            .push(Message::complete(Role::You, text));
        conversation
            .messages
            .push(Message::streaming(Role::Assistant, reply));
        conversation.streaming_since = Some(now);
    }

    /// Advance whatever is currently arriving.
    ///
    /// Returns true while there is still more to reveal, which is the window's
    /// cue to keep drawing frames.
    pub fn advance_streaming(&mut self, now: f64) -> bool {
        // Roughly the speed of a fast typist, which reads as deliberate rather
        // than instant without being slow enough to annoy.
        const BYTES_PER_SECOND: f64 = 900.0;

        let conversation = self.current_mut();
        let Some(started) = conversation.streaming_since else {
            return false;
        };
        let budget = ((now - started) * BYTES_PER_SECOND) as usize;

        let Some(message) = conversation.messages.last_mut() else {
            conversation.streaming_since = None;
            return false;
        };
        message.reveal_to(budget);

        if message.is_streaming() {
            true
        } else {
            conversation.streaming_since = None;
            false
        }
    }
}

/// Produce something to say back.
///
/// Canned, because the point of this application is the interface rather than
/// the answer, but varied enough that the transcript does not look fake.
fn reply_to(prompt: &str) -> String {
    let lower = prompt.to_lowercase();

    if lower.contains("select") || lower.contains("copy") {
        return "Selection works **within a single block of text**. Press and \
                sweep to select, double click for a word, triple click for a \
                line, and the platform copy shortcut puts it on the clipboard.\n\n\
                Selecting across separate messages would need a total ordering \
                over the whole element tree, which is a larger piece of work \
                than it first appears."
            .into();
    }
    if lower.contains("style") || lower.contains("theme") || lower.contains("css") {
        return "Styling is a real cascading stylesheet with its own parser. It \
                has selectors, combinators, specificity, pseudo classes, custom \
                properties and transitions.\n\n\
                The file next to this binary is watched while it runs, so \
                editing a color or a radius changes the window without a \
                rebuild. Try it: `apps/chat/assets/theme.gss`. The colors this \
                paragraph uses for code and links come from \
                `--code-surface` and `--link` in that file."
            .into();
    }
    if lower.contains("fast") || lower.contains("slow") || lower.contains("perf") {
        return "Every frame phase is measured separately, because \"the frame is \
                slow\" is not a diagnosis.\n\n\
                Building the tree, laying it out, painting it, preparing the \
                instance buffers and waiting on the swap chain all fail for \
                unrelated reasons, so they are timed apart. Two real bugs were \
                found that way: an inherited style field that allocated on every \
                clone, and blank glyphs being rasterized again on **every single \
                frame**.\n\n\
                Memory is measured the same way. The numbers in the title bar \
                are live: font data, text caches and atlas textures, counted \
                separately because the answer to \"why is this using memory\" is \
                almost always one of them rather than the total."
            .into();
    }
    if lower.contains("how") && lower.contains("draw") {
        return "**Two pipelines.** One draws rounded boxes, the other draws \
                sprites.\n\n\
                Borders, per corner radii, gradients and Gaussian shadows are all \
                evaluated analytically in the box shader, so there is no \
                tessellation and antialiasing is exact. Glyphs and images come \
                from array atlases. A whole interface collapses into a few dozen \
                draw calls.\n\n\
                The shaders are `quad.wgsl` and `sprite.wgsl`, and there are no \
                others."
            .into();
    }
    if lower.contains("scroll") {
        return "The transcript is a scrolling view that stays pinned to the end \
                while you are already at the end, and leaves you alone when you \
                have scrolled up to read something.\n\n\
                Its scrollbar is a real control: grab the thumb, or click the \
                track to jump."
            .into();
    }

    format!(
        "You said: \u{201c}{}\u{201d}\n\n\
         There is no model behind this window, only a handful of canned replies, \
         because the interesting part here is the interface rather than the \
         answer. Ask about **styling**, **selection**, **scrolling**, \
         **performance**, or how any of this is drawn.",
        prompt.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reveals_itself_over_time() {
        let mut chat = Chat::new();
        chat.new_conversation();
        chat.send("hello", 0.0);

        assert!(chat.current().messages.last().unwrap().is_streaming());
        assert_eq!(chat.current().messages.last().unwrap().visible(), "");

        assert!(chat.advance_streaming(0.05));
        assert!(!chat.current().messages.last().unwrap().visible().is_empty());

        // Long enough to finish.
        assert!(!chat.advance_streaming(100.0));
        let message = chat.current().messages.last().unwrap();
        assert!(!message.is_streaming());
        assert_eq!(message.visible(), message.text);
    }

    #[test]
    fn revealing_never_splits_a_character() {
        let mut message = Message::streaming(Role::Assistant, "caf\u{00e9} \u{65e5}\u{672c}");
        for target in 0..=message.text.len() {
            message.revealed = 0;
            message.reveal_to(target);
            // Both the stored offset and the slice must stay on a boundary.
            assert!(message.text.is_char_boundary(message.revealed));
            let _ = message.visible();
        }
    }

    #[test]
    fn the_first_message_names_the_conversation() {
        let mut chat = Chat::new();
        chat.new_conversation();
        assert_eq!(chat.current().title, "New chat");

        chat.send("How does layout work", 0.0);
        assert_eq!(chat.current().title, "How does layout work");

        // A later message leaves the title alone.
        chat.send("and what about text", 1.0);
        assert_eq!(chat.current().title, "How does layout work");
    }

    #[test]
    fn empty_input_sends_nothing() {
        let mut chat = Chat::new();
        chat.new_conversation();
        chat.send("   ", 0.0);
        assert!(chat.current().messages.is_empty());
    }

    #[test]
    fn a_send_produces_a_pair_of_turns() {
        let mut chat = Chat::new();
        chat.new_conversation();
        chat.send("hello", 0.0);
        let messages = &chat.current().messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::You);
        assert_eq!(messages[1].role, Role::Assistant);
    }

    #[test]
    fn the_preview_shows_words_rather_than_markup() {
        let chat = Chat::new();
        let preview = chat.conversations[0].preview();
        assert!(
            !preview.contains("**"),
            "the preview leaked its markup: {preview}"
        );
        assert!(!preview.contains('`'), "the preview leaked a code fence");
    }

    #[test]
    fn the_preview_is_short_and_single_line() {
        let mut chat = Chat::new();
        chat.new_conversation();
        assert_eq!(chat.current().preview(), "No messages yet");

        chat.send("a question that runs on and on and on and on and on", 0.0);
        let preview = chat.current().preview();
        assert!(!preview.contains('\n'));
        assert!(preview.chars().count() <= 47);
    }

    #[test]
    fn switching_conversations_keeps_them_apart() {
        let mut chat = Chat::new();
        chat.new_conversation();
        chat.send("first", 0.0);
        let first = chat.active;

        chat.new_conversation();
        assert!(chat.current().messages.is_empty());

        chat.active = first;
        assert_eq!(chat.current().messages.len(), 2);
    }
}
