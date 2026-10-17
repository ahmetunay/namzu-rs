use crate::editor::{self, Move};
use namzu_core::{Event, Stats};
use std::collections::VecDeque;
use std::time::Instant;

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
    pub tool_id: Option<String>,
    pub expanded: bool,
    pub diff: bool,
}

pub(crate) struct App {
    pub entries: VecDeque<Entry>,
    pub input: String,
    pub cursor: usize,
    pub context_used: Option<usize>,
    pub context_limit: usize,
    pub selected_tool: Option<String>,
    pub started: Option<Instant>,
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
            cursor: 0,
            context_used: Some(0),
            context_limit: 4 * 1024 * 1024,
            selected_tool: None,
            started: None,
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
        self.entries.push_back(Entry {
            label,
            text,
            tool_id: None,
            expanded: false,
            diff: false,
        });
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
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let remaining = INPUT_LIMIT.saturating_sub(self.input.len());
        let part = visible(&value, remaining);
        if part.len() < value.len() {
            self.status = "Input limit: 16 KiB".into();
        }
        self.cursor = self.cursor.min(self.input.len());
        self.input.insert_str(self.cursor, &part);
        self.cursor += part.len();
    }
    pub fn move_input(&mut self, direction: Move) {
        if !self.busy {
            self.cursor =
                editor::move_cursor(&self.input, self.cursor.min(self.input.len()), direction);
        }
    }
    pub fn backspace(&mut self) {
        if !self.busy {
            let end = self.cursor.min(self.input.len());
            let start = editor::move_cursor(&self.input, end, Move::Left);
            self.input.replace_range(start..end, "");
            self.cursor = start;
        }
    }
    pub fn delete(&mut self) {
        if !self.busy {
            self.cursor = self.cursor.min(self.input.len());
            let end = editor::move_cursor(&self.input, self.cursor, Move::Right);
            self.input.replace_range(self.cursor..end, "");
        }
    }
    pub fn select_tool(&mut self, previous: bool) {
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter_map(|e| e.tool_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let index = self
            .selected_tool
            .as_ref()
            .and_then(|id| ids.iter().position(|v| v == id))
            .unwrap_or(ids.len() - 1);
        self.selected_tool = Some(
            ids[if previous {
                index.saturating_sub(1)
            } else {
                (index + 1).min(ids.len() - 1)
            }]
            .clone(),
        );
    }
    pub fn toggle_tool(&mut self) {
        if self.selected_tool.is_none() {
            self.selected_tool = self.entries.iter().rev().find_map(|e| e.tool_id.clone());
        }
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.tool_id == self.selected_tool && e.tool_id.is_some())
        {
            e.expanded = !e.expanded;
        }
    }
    pub fn submit(&mut self) -> Option<String> {
        if self.busy || self.blocked || self.input.trim().is_empty() {
            return None;
        }
        let prompt = std::mem::take(&mut self.input);
        self.push("You", &prompt);
        self.draft.clear();
        self.cursor = 0;
        self.stats = None;
        self.started = Some(Instant::now());
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
            Event::ToolStarted { id, name } => {
                self.status = format!("Tool · {}", visible(&name, 128));
                self.push("Tool", &format!("{} · running", visible(&name, 128)));
                self.entries.back_mut().unwrap().tool_id = Some(id.clone());
                self.selected_tool = Some(id);
            }
            Event::ToolFinished { id, name, result } => {
                let text = visible(
                    &format!(
                        "{} · {}\n{}",
                        name,
                        if result.is_error { "error" } else { "OK" },
                        result.content
                    ),
                    ENTRY_LIMIT,
                );
                if let Some(e) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.tool_id.as_deref() == Some(&id))
                {
                    self.bytes -= e.text.len();
                    self.bytes += text.len();
                    e.text = text;
                    e.diff = name == "apply_patch" || name == "diff_file";
                } else {
                    self.push("Tool", &text);
                    let e = self.entries.back_mut().unwrap();
                    e.tool_id = Some(id.clone());
                    e.diff = name == "apply_patch" || name == "diff_file";
                }
                while self.bytes > HISTORY_LIMIT {
                    if let Some(e) = self.entries.pop_front() {
                        self.bytes -= e.text.len();
                    }
                }
                self.selected_tool = Some(id);
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
        self.context_used = outcome.context_bytes;
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
    pub fn load_history(&mut self, history: crate::History) {
        *self = Self::new(Some(history.id));
        for entry in history.entries {
            self.push(entry.label, &entry.content);
            if let Some(id) = entry.tool_id {
                self.entries.back_mut().unwrap().tool_id = Some(id);
            }
        }
        self.context_used = Some(history.context_bytes);
        self.blocked = !history.resumable;
        self.status = history.notice;
    }
    pub fn new_conversation(&mut self) {
        if !self.busy {
            *self = Self::new(None);
        }
    }
}
