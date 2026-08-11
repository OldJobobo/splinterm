//! Bounded text editing for trusted client-owned fields.

const MAX_UNDO_STATES: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditSnapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoundedTextEditor {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    undo: Vec<EditSnapshot>,
    max_bytes: usize,
    max_scalars: usize,
}

impl BoundedTextEditor {
    pub(crate) fn new(
        text: String,
        max_bytes: usize,
        max_scalars: usize,
        select_all: bool,
    ) -> Self {
        debug_assert!(text.len() <= max_bytes);
        debug_assert!(text.chars().count() <= max_scalars);
        let cursor = text.len();
        Self {
            text,
            cursor,
            anchor: select_all.then_some(0),
            undo: Vec::new(),
            max_bytes,
            max_scalars,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        let (start, end) = self.selection_range()?;
        (start != end).then(|| &self.text[start..end])
    }

    pub(crate) fn insert(&mut self, input: &str) -> bool {
        let (start, end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        let base_bytes = self.text.len().saturating_sub(end.saturating_sub(start));
        let base_scalars = self
            .text
            .chars()
            .count()
            .saturating_sub(self.text[start..end].chars().count());
        let mut accepted = String::new();
        for character in input
            .chars()
            .filter(|character| !character.is_control() && !is_bidi_formatting(*character))
        {
            if base_bytes
                .saturating_add(accepted.len())
                .saturating_add(character.len_utf8())
                > self.max_bytes
                || base_scalars
                    .saturating_add(accepted.chars().count())
                    .saturating_add(1)
                    > self.max_scalars
            {
                break;
            }
            accepted.push(character);
        }
        if accepted.is_empty() {
            return false;
        }
        self.push_undo();
        self.text.replace_range(start..end, &accepted);
        self.cursor = start.saturating_add(accepted.len());
        self.anchor = None;
        true
    }

    pub(crate) fn backspace(&mut self) -> bool {
        if let Some((start, end)) = self.selection_range()
            && start != end
        {
            self.push_undo();
            self.text.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let start = previous_boundary(&self.text, self.cursor);
        self.push_undo();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    pub(crate) fn move_left(&mut self, extend: bool) -> bool {
        let next = if extend {
            previous_boundary(&self.text, self.cursor)
        } else {
            self.selection_range()
                .filter(|(start, end)| start != end)
                .map_or_else(
                    || previous_boundary(&self.text, self.cursor),
                    |range| range.0,
                )
        };
        self.move_cursor(next, extend)
    }

    pub(crate) fn move_right(&mut self, extend: bool) -> bool {
        let next = if extend {
            next_boundary(&self.text, self.cursor)
        } else {
            self.selection_range()
                .filter(|(start, end)| start != end)
                .map_or_else(|| next_boundary(&self.text, self.cursor), |range| range.1)
        };
        self.move_cursor(next, extend)
    }

    pub(crate) fn move_home(&mut self, extend: bool) -> bool {
        self.move_cursor(0, extend)
    }

    pub(crate) fn move_end(&mut self, extend: bool) -> bool {
        self.move_cursor(self.text.len(), extend)
    }

    pub(crate) fn select_all(&mut self) -> bool {
        if self.text.is_empty() || (self.anchor == Some(0) && self.cursor == self.text.len()) {
            return false;
        }
        self.anchor = Some(0);
        self.cursor = self.text.len();
        true
    }

    pub(crate) fn cut(&mut self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        if start == end {
            return None;
        }
        let cut = self.text[start..end].to_owned();
        self.push_undo();
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        Some(cut)
    }

    pub(crate) fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.text = previous.text;
        self.cursor = previous.cursor;
        self.anchor = previous.anchor;
        true
    }

    #[cfg(test)]
    pub(crate) fn undo_len(&self) -> usize {
        self.undo.len()
    }

    fn move_cursor(&mut self, next: usize, extend: bool) -> bool {
        let next = next.min(self.text.len());
        if next == self.cursor && (extend || self.anchor.is_none()) {
            return false;
        }
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = next;
        true
    }

    fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        Some(if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        })
    }

    fn push_undo(&mut self) {
        if self.undo.len() == MAX_UNDO_STATES {
            self.undo.remove(0);
        }
        self.undo.push(EditSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        });
    }
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(index, _)| cursor.saturating_add(index))
}

fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_cut_paste_and_undo_are_unicode_safe_and_bounded() {
        let mut editor = BoundedTextEditor::new("a界c".into(), 16, 8, false);
        assert!(editor.move_left(true));
        assert_eq!(editor.selected_text(), Some("c"));
        assert_eq!(editor.cut().as_deref(), Some("c"));
        assert_eq!(editor.text(), "a界");
        assert!(editor.insert(" Z\n\u{202e}"));
        assert_eq!(editor.text(), "a界 Z");
        assert!(editor.undo());
        assert_eq!(editor.text(), "a界");
        assert!(editor.undo());
        assert_eq!(editor.text(), "a界c");
    }

    #[test]
    fn undo_history_and_input_size_have_hard_limits() {
        let mut editor = BoundedTextEditor::new(String::new(), 4, 4, false);
        for _ in 0..32 {
            editor.insert("x");
            editor.backspace();
        }
        assert_eq!(editor.undo_len(), MAX_UNDO_STATES);
        assert!(editor.insert("abcdef"));
        assert_eq!(editor.text(), "abcd");
        assert!(!editor.insert("z"));
    }

    #[test]
    fn initial_select_all_replaces_rename_text_and_empty_cut_is_inert() {
        let mut editor = BoundedTextEditor::new("old".into(), 8, 8, true);
        assert_eq!(editor.selected_text(), Some("old"));
        assert!(editor.insert("new"));
        assert_eq!(editor.text(), "new");
        assert_eq!(editor.cut(), None);
        assert!(editor.select_all());
        assert_eq!(editor.cut().as_deref(), Some("new"));
    }
}
