//! In-place editing of Codex's `config.toml` for the `[mcp_servers.splinterm]`
//! table.
//!
//! The file is never re-serialized. A new entry is appended as its own table,
//! the form `codex mcp add` writes; a replaced or removed entry is cut out by
//! its table headers. Every edit is checked by parsing the result and comparing
//! it with the intended document; anything the line-level edit cannot express
//! (an inline table, dotted keys, a header inside a multi-line string) fails
//! that check and writes nothing.

use std::fmt::Write as _;

use ::toml::{Table, Value};
use anyhow::{Result, bail};

use super::{Action, Plan, SERVER_NAME};

const CONTAINER: &str = "mcp_servers";

/// The Codex server entry: the adapter command and the Splinterm variables
/// Codex must forward, since it starts servers with a filtered environment.
pub(super) struct TomlEntry {
    pub(super) command: String,
    pub(super) env_vars: &'static [&'static str],
}

/// A TOML basic string for `text`.
pub(super) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            character if character.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

impl TomlEntry {
    pub(super) fn render(&self) -> String {
        let env_vars = self
            .env_vars
            .iter()
            .map(|name| quote(name))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "[{CONTAINER}.{SERVER_NAME}]\ncommand = {}\nenv_vars = [{env_vars}]\n",
            quote(&self.command)
        )
    }

    pub(super) fn to_value(&self) -> Value {
        let mut table = Table::new();
        table.insert("command".to_owned(), Value::String(self.command.clone()));
        table.insert(
            "env_vars".to_owned(),
            Value::Array(
                self.env_vars
                    .iter()
                    .map(|name| Value::String((*name).to_owned()))
                    .collect(),
            ),
        );
        Value::Table(table)
    }
}

fn parse(text: &str) -> Result<Table> {
    text.parse::<Table>().map_err(|error| {
        let message = error.message().trim_end().replace('\n', " ");
        match error.span() {
            Some(span) => {
                let before = text.get(..span.start).unwrap_or(text);
                let line = before.matches('\n').count() + 1;
                let column = before.len() - before.rfind('\n').map_or(0, |index| index + 1) + 1;
                anyhow::anyhow!("invalid TOML at line {line}, column {column}: {message}")
            }
            None => anyhow::anyhow!("invalid TOML: {message}"),
        }
    })
}

/// The existing `mcp_servers.splinterm` value, after checking the shape.
fn existing(document: &Table) -> Result<Option<&Value>> {
    match document.get(CONTAINER) {
        None => Ok(None),
        Some(Value::Table(servers)) => Ok(servers.get(SERVER_NAME)),
        Some(_) => bail!("`{CONTAINER}` is not a table"),
    }
}

/// Parses one table header line's dotted key, or `None` for any other line.
fn header_path(line: &str) -> Option<Vec<String>> {
    let line = line.trim_start();
    let rest = line.strip_prefix('[')?;
    if rest.starts_with('[') {
        // An array-of-tables header is a header too, but never ours.
        return Some(Vec::new());
    }
    let mut keys = Vec::new();
    let mut chars = rest.trim_start().chars().peekable();
    loop {
        let mut key = String::new();
        match chars.peek()? {
            '"' => {
                chars.next();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => return Some(Vec::new()),
                        character => key.push(character),
                    }
                }
            }
            '\'' => {
                chars.next();
                loop {
                    match chars.next()? {
                        '\'' => break,
                        character => key.push(character),
                    }
                }
            }
            _ => {
                while let Some(&character) = chars.peek() {
                    if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                        key.push(character);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if key.is_empty() {
                    return Some(Vec::new());
                }
            }
        }
        keys.push(key);
        while chars
            .peek()
            .is_some_and(|character| *character == ' ' || *character == '\t')
        {
            chars.next();
        }
        match chars.next()? {
            '.' => {
                while chars
                    .peek()
                    .is_some_and(|character| *character == ' ' || *character == '\t')
                {
                    chars.next();
                }
            }
            ']' => return Some(keys),
            _ => return Some(Vec::new()),
        }
    }
}

/// Byte ranges of every table section belonging to `[mcp_servers.splinterm]`
/// (the table and its subtables). A section runs from its header to the next
/// header, minus the blank and comment lines just before that header, which
/// belong to it.
fn sections(text: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    let mut in_multiline: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let header = if in_multiline.is_none() {
            header_path(line)
        } else {
            None
        };
        for delimiter in ["\"\"\"", "'''"] {
            let count = line.matches(delimiter).count();
            if count % 2 == 1 {
                in_multiline = match in_multiline {
                    None => Some(delimiter),
                    Some(open) if open == delimiter => None,
                    other => other,
                };
            }
        }
        lines.push((offset, line, header));
        offset += line.len();
    }
    let ours = |path: &[String]| path.len() >= 2 && path[0] == CONTAINER && path[1] == SERVER_NAME;
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (start, _, header) = &lines[index];
        if !header.as_deref().is_some_and(ours) {
            index += 1;
            continue;
        }
        let mut next = index + 1;
        while next < lines.len() && lines[next].2.is_none() {
            next += 1;
        }
        let mut last = next;
        if next < lines.len() {
            while last > index + 1 {
                let trimmed = lines[last - 1].1.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    last -= 1;
                } else {
                    break;
                }
            }
        }
        let end = lines.get(last).map_or(text.len(), |(offset, _, _)| *offset);
        ranges.push((*start, end));
        index = next;
    }
    ranges
}

/// Cuts `ranges` out of `text`, putting `replacement` at the first one. A
/// range cut out from between two blank lines takes one of them along.
fn cut(text: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    let mut out = text.to_owned();
    for (index, &(start, end)) in ranges.iter().enumerate().rev() {
        let insert = if index == 0 { replacement } else { "" };
        let mut end = end;
        if insert.is_empty()
            && (start == 0 || text[..start].ends_with("\n\n"))
            && text[end..].starts_with('\n')
        {
            end += 1;
        }
        out.replace_range(start..end, insert);
    }
    out
}

fn appended(text: &str, block: &str) -> String {
    if text.trim().is_empty() {
        return block.to_owned();
    }
    let mut out = text.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(block);
    out
}

/// Trailing blank lines left at the end of the file by a removal.
fn tidy_end(mut text: String) -> String {
    let trimmed = text.trim_end_matches(['\n', '\r', ' ', '\t']).len();
    if trimmed == 0 {
        return String::new();
    }
    text.truncate(trimmed);
    text.push('\n');
    text
}

/// An empty `mcp_servers` table means the same as none at all.
fn normalized(mut document: Table) -> Table {
    if document
        .get(CONTAINER)
        .and_then(Value::as_table)
        .is_some_and(Table::is_empty)
    {
        document.remove(CONTAINER);
    }
    document
}

fn verify(text: &str, expected: &Table) -> Result<()> {
    match parse(text).map(normalized) {
        Ok(actual) if actual == normalized(expected.clone()) => Ok(()),
        _ => Err(super::unsafe_edit()),
    }
}

/// Plans adding (or with `replace`, replacing) the `splinterm` table.
pub(super) fn plan_setup(text: Option<&str>, entry: &TomlEntry, replace: bool) -> Result<Plan> {
    let block = entry.render();
    let Some(text) = text else {
        return Ok(Plan::change(Action::Created, block));
    };
    let document = parse(text)?;
    let value = entry.to_value();
    let action = match existing(&document)? {
        None => Action::Added,
        Some(current) if *current == value => return Ok(Plan::unchanged(Action::Unchanged)),
        Some(_) if !replace => return Err(super::conflict()),
        Some(_) => Action::Replaced,
    };
    let new_text = if action == Action::Added {
        appended(text, &block)
    } else {
        let ranges = sections(text);
        if ranges.is_empty() {
            return Err(super::unsafe_edit());
        }
        cut(text, &ranges, &block)
    };
    let mut expected = document;
    expected
        .entry(CONTAINER.to_owned())
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .expect("the container is a table")
        .insert(SERVER_NAME.to_owned(), value);
    verify(&new_text, &expected)?;
    Ok(Plan::change(action, new_text))
}

/// Plans removing the `splinterm` table, leaving everything else.
pub(super) fn plan_remove(text: Option<&str>) -> Result<Plan> {
    let Some(text) = text else {
        return Ok(Plan::unchanged(Action::Absent));
    };
    let document = parse(text)?;
    if existing(&document)?.is_none() {
        return Ok(Plan::unchanged(Action::Absent));
    }
    let ranges = sections(text);
    if ranges.is_empty() {
        return Err(super::unsafe_edit());
    }
    let mut new_text = cut(text, &ranges, "");
    if ranges.last().is_some_and(|(_, end)| *end == text.len()) {
        new_text = tidy_end(new_text);
    }
    let mut expected = document;
    expected
        .get_mut(CONTAINER)
        .and_then(Value::as_table_mut)
        .expect("the container is a table")
        .remove(SERVER_NAME);
    verify(&new_text, &expected)?;
    Ok(Plan::change(Action::Removed, new_text))
}
