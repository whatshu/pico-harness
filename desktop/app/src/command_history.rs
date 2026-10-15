//! Per-console command history, including the draft before browsing starts.
#[derive(Default)]
pub struct CommandHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl CommandHistory {
    pub fn remember(&mut self, command: &str) {
        if !command.is_empty() && self.entries.last().is_none_or(|last| last != command) {
            self.entries.push(command.into());
            if self.entries.len() > 100 {
                self.entries.remove(0);
            }
        }
        self.cursor = None;
        self.draft.clear();
    }

    pub fn previous(&mut self, input: &mut String) {
        if self.entries.is_empty() {
            return;
        }
        let cursor = match self.cursor {
            None => {
                self.draft = input.clone();
                self.entries.len() - 1
            }
            Some(cursor) => cursor.saturating_sub(1),
        };
        self.cursor = Some(cursor);
        *input = self.entries[cursor].clone();
    }

    pub fn next(&mut self, input: &mut String) {
        let Some(cursor) = self.cursor else { return };
        if cursor + 1 < self.entries.len() {
            self.cursor = Some(cursor + 1);
            *input = self.entries[cursor + 1].clone();
        } else {
            self.cursor = None;
            *input = self.draft.clone();
        }
    }
}
