//! Bounded composed-character storage used by combining input and grid reflow.
//!
//! Foot 1.27.0 stores composed sequences in `composed.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Splinterm keeps the observable
//! semantics while using safe project-owned indexing; numeric keys are internal
//! and are never a protocol contract.

use std::collections::HashMap;

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    chars: Vec<char>,
    width: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposedTable {
    by_sequence: HashMap<Vec<char>, u32>,
    entries: Vec<Entry>,
    limit: usize,
}

impl ComposedTable {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            by_sequence: HashMap::new(),
            entries: Vec::new(),
            limit,
        }
    }

    pub(crate) fn intern(&mut self, chars: Vec<char>) -> Option<u32> {
        if let Some(key) = self.by_sequence.get(&chars) {
            return Some(*key);
        }
        if self.entries.len() >= self.limit || self.entries.len() > 0x3fff_ffff {
            return None;
        }
        let key = u32::try_from(self.entries.len()).ok()?;
        let width = chars
            .first()
            .and_then(|character| UnicodeWidthChar::width(*character))
            .unwrap_or(0)
            .max(1);
        self.entries.push(Entry {
            chars: chars.clone(),
            width,
        });
        self.by_sequence.insert(chars, key);
        Some(key)
    }

    pub(crate) fn chars(&self, key: u32) -> Option<&[char]> {
        self.entries
            .get(usize::try_from(key).ok()?)
            .map(|entry| entry.chars.as_slice())
    }

    pub(crate) fn width(&self, key: u32) -> usize {
        self.entries
            .get(usize::try_from(key).unwrap_or(usize::MAX))
            .map_or(1, |entry| entry.width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_and_reuses_sequences_with_a_hard_limit() {
        let mut table = ComposedTable::new(1);
        let sequence = vec!['a', '\u{301}'];
        let key = table.intern(sequence.clone()).unwrap();
        assert_eq!(table.intern(sequence.clone()), Some(key));
        assert_eq!(table.chars(key), Some(sequence.as_slice()));
        assert_eq!(table.intern(vec!['b', '\u{301}']), None);
    }
}
