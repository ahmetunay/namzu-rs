use namzu_core::{Event, Stats};
use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) const INPUT_LIMIT: usize = 16 * 1024;
const ENTRY_LIMIT: usize = 64 * 1024;
const HISTORY_LIMIT: usize = 256 * 1024;

/// Never allow task/model/tool text to issue terminal control sequences.
pub(crate) fn visible(text: &str, limit: usize) -> String {
    let mut result = String::new();
    for c in text.chars() {
        let c = match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                '\u{fffd}'
            }
            c => c,
        };
        if result.len() + c.len_utf8() > limit {
            break;
        }
        result.push(c);
    }
    result
}

pub(crate) struct Entry {
    pub label: &'static str,
    pub text: String,
}

pub(crate) struct App {
    pub entries: VecDeque<Entry>,
    pub input: String,
    pub draft: String,
    pub busy: bool,
    pub blocked: bool,
    pub status: String,
    pub session_id: Option<String>,
    pub stats: Option<Stats>,
    pub scroll: usize,
    bytes: usize,
}
impl App {
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            entries: VecDeque::new(),
            input: String::new(),
            draft: String::new(),
            busy: false,
            blocked: false,
            status: "Ready".into(),
            session_id,
            stats: None,
            scroll: 0,
            bytes: 0,
        }
    }
    fn push(&mut self, label: &'static str, text: &str) {
        let truncated = text.len() > ENTRY_LIMIT;
        let mut text = visible(text, ENTRY_LIMIT);
        // The full conversation remains in private session storage; this view is bounded.
        if truncated {
            text.push_str("\n[display limit]");
        }
        self.bytes += text.len();
        self.entries.push_back(Entry { label, text });
        while self.bytes > HISTORY_LIMIT || self.entries.len() > 256 {
            if let Some(entry) = self.entries.pop_front() {
                self.bytes -= entry.text.len();
            }
        }
    }
    pub fn append_input(&mut self, value: &str) {
        if self.busy || self.blocked {
            return;
        }
        let value = value.replace(['\n', '\r'], " ");
        let remaining = INPUT_LIMIT.saturating_sub(self.input.len());
        let part = visible(&value, remaining);
        if part.len() < value.len() {
            self.status = "Input limit: 16 KiB".into();
        }
        self.input.push_str(&part);
    }
    pub fn backspace(&mut self) {
        if !self.busy
            && let Some((offset, _)) = self.input.grapheme_indices(true).next_back()
        {
            self.input.truncate(offset);
        }
    }
    pub fn submit(&mut self) -> Option<String> {
        if self.busy || self.blocked || self.input.trim().is_empty() {
            return None;
        }
        let prompt = std::mem::take(&mut self.input);
        self.push("You", &prompt);
        self.draft.clear();
        self.busy = true;
        self.scroll = 0;
        self.status = "Connecting…".into();
        Some(prompt)
    }
    pub fn event(&mut self, event: Event) {
        match event {
            Event::TurnStarted => self.status = "Starting…".into(),
            Event::ProviderStarted { step } => {
                self.draft.clear();
                self.status = format!("Model · step {step}");
            }
            Event::AssistantDelta { text } => {
                self.draft.push_str(&visible(
                    &text,
                    ENTRY_LIMIT.saturating_sub(self.draft.len()),
                ));
            }
            Event::AssistantMessage { text, .. } => {
                self.draft.clear();
                if !text.is_empty() {
                    self.push("Assistant", &text);
                }
            }
            Event::ToolStarted { name, .. } => {
                self.status = format!("Tool · {}", visible(&name, 128));
                self.push("Tool", &format!("{} · running", visible(&name, 128)));
            }
            Event::ToolFinished { name, result, .. } => {
                self.push(
                    "Tool",
                    &format!(
                        "{} · {}\n{}",
                        visible(&name, 128),
                        if result.is_error { "error" } else { "OK" },
                        visible(&result.content, 240)
                    ),
                );
            }
            Event::TurnCompleted { stats } => {
                self.stats = Some(stats);
                self.status = "Saving session…".into();
            }
            Event::TurnFailed { .. } => {
                self.draft.clear();
                self.status = "Failed".into();
            }
        }
    }
    pub fn finish(&mut self, outcome: crate::Outcome) {
        self.busy = false;
        self.draft.clear();
        if let Some(id) = outcome.session_id {
            self.session_id = Some(id);
        }
        if let Some(error) = outcome.error {
            self.blocked = true;
            self.status = "Turn failed · Ctrl-N starts a new conversation".into();
            self.push("Error", &error);
        } else {
            self.status = "Ready".into();
        }
    }
    pub fn new_conversation(&mut self) {
        if !self.busy {
            *self = Self::new(None);
        }
    }
}
