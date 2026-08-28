//! Bounded composed-character storage used by combining input and grid reflow.
//!
//! Foot 1.27.0 stores composed sequences in `composed.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Splinterm keeps the observable
//! semantics while using safe project-owned indexing; numeric keys are internal
//! and are never a protocol contract.

use std::{collections::HashMap, fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{SeqAccess, Visitor},
};
use unicode_width::UnicodeWidthChar;

const MAX_CHECKPOINT_COMPOSED_ENTRIES: usize = 1 << 16;
const MAX_CHECKPOINT_COMPOSED_CHARS: usize = 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(deserialize_with = "deserialize_composed_chars")]
    chars: Vec<char>,
    width: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComposedTable {
    #[serde(skip)]
    by_sequence: HashMap<Vec<char>, u32>,
    #[serde(deserialize_with = "deserialize_composed_entries")]
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

    pub(crate) fn checkpoint_entry_count(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn validate_and_rebuild_checkpoint(&mut self, expected_limit: usize) -> bool {
        if self.limit != expected_limit
            || self.entries.len() > self.limit
            || self.entries.len() > MAX_CHECKPOINT_COMPOSED_ENTRIES
        {
            return false;
        }
        let mut rebuilt = HashMap::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let Some(first) = entry.chars.first() else {
                return false;
            };
            if entry.chars.len() > MAX_CHECKPOINT_COMPOSED_CHARS {
                return false;
            }
            let width = UnicodeWidthChar::width(*first).unwrap_or(0).max(1);
            let Ok(key) = u32::try_from(index) else {
                return false;
            };
            if entry.width != width || rebuilt.insert(entry.chars.clone(), key).is_some() {
                return false;
            }
        }
        self.by_sequence = rebuilt;
        true
    }
}

fn deserialize_composed_entries<'de, D>(deserializer: D) -> Result<Vec<Entry>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(
        deserializer,
        MAX_CHECKPOINT_COMPOSED_ENTRIES,
        "composed entries",
    )
}

fn deserialize_composed_chars<'de, D>(deserializer: D) -> Result<Vec<char>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_sequence(
        deserializer,
        MAX_CHECKPOINT_COMPOSED_CHARS,
        "composed characters",
    )
}

fn deserialize_bounded_sequence<'de, D, T>(
    deserializer: D,
    maximum: usize,
    name: &'static str,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedVisitor<T> {
        maximum: usize,
        name: &'static str,
        marker: PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for BoundedVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "at most {} {}", self.maximum, self.name)
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            if sequence
                .size_hint()
                .is_some_and(|length| length > self.maximum)
            {
                return Err(serde::de::Error::custom(format_args!(
                    "{} exceed checkpoint bound",
                    self.name
                )));
            }
            let mut values =
                Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(self.maximum));
            while let Some(value) = sequence.next_element()? {
                if values.len() == self.maximum {
                    return Err(serde::de::Error::custom(format_args!(
                        "{} exceed checkpoint bound",
                        self.name
                    )));
                }
                values.push(value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(BoundedVisitor {
        maximum,
        name,
        marker: PhantomData,
    })
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
