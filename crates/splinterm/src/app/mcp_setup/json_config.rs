//! A span-keeping JSON and JSONC reader that edits one member of an MCP host
//! configuration in place.
//!
//! The host's file is never re-serialized. An edit splices text into the
//! original, so unrelated members keep their order, spacing, and (in JSONC)
//! comments. The reader rejects duplicate keys anywhere in the document, which
//! a host's own parser would silently resolve to one of them. Every edit is
//! checked by parsing the result again and comparing it with the intended
//! document; a mismatch writes nothing.

use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Number, Value};

use super::{Action, Plan, SERVER_NAME};

/// Deeper documents are refused rather than risk the reader's stack.
const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialect {
    /// Strict JSON (Claude Code's `~/.claude.json`).
    Json,
    /// JSON with comments and trailing commas (VS Code's `mcp.json`).
    Jsonc,
}

/// An ordered JSON value for rendering new text. `serde_json::Value` would
/// sort object keys; hosts write theirs in a fixed order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ordered {
    Str(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Str(text) => Value::String(text.clone()),
            Self::Array(items) => Value::Array(items.iter().map(Self::to_value).collect()),
            Self::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }

    /// Renders the value as it would appear on a line indented by `indent`.
    fn render(&self, indent: &str, unit: &str) -> String {
        let mut out = String::new();
        self.render_into(indent, unit, &mut out);
        out
    }

    fn render_into(&self, indent: &str, unit: &str, out: &mut String) {
        let inner = format!("{indent}{unit}");
        match self {
            Self::Str(text) => out.push_str(&quote(text)),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Object(members) if members.is_empty() => out.push_str("{}"),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&inner);
                    item.render_into(&inner, unit, out);
                }
                out.push('\n');
                out.push_str(indent);
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&inner);
                    out.push_str(&quote(key));
                    out.push_str(": ");
                    value.render_into(&inner, unit, out);
                }
                out.push('\n');
                out.push_str(indent);
                out.push('}');
            }
        }
    }
}

/// A JSON string literal for `text`, escaped as `serde_json` escapes it.
pub(super) fn quote(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

struct Node {
    start: usize,
    end: usize,
    kind: NodeKind,
}

enum NodeKind {
    Scalar(Value),
    Array(Vec<Node>),
    Object(ObjectSyntax),
}

struct ObjectSyntax {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

struct Member {
    key: String,
    key_start: usize,
    value: Node,
    /// The comma that follows the value, if any.
    comma: Option<usize>,
}

impl Node {
    fn to_value(&self) -> Value {
        match &self.kind {
            NodeKind::Scalar(value) => value.clone(),
            NodeKind::Array(items) => Value::Array(items.iter().map(Self::to_value).collect()),
            NodeKind::Object(object) => Value::Object(
                object
                    .members
                    .iter()
                    .map(|member| (member.key.clone(), member.value.to_value()))
                    .collect::<Map<_, _>>(),
            ),
        }
    }

    fn object(&self) -> Option<&ObjectSyntax> {
        match &self.kind {
            NodeKind::Object(object) => Some(object),
            _ => None,
        }
    }
}

impl ObjectSyntax {
    fn member(&self, key: &str) -> Option<(usize, &Member)> {
        self.members
            .iter()
            .enumerate()
            .find(|(_, member)| member.key == key)
    }
}

/// Line and column (both from 1) of a byte offset, for error messages.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |index| index + 1) + 1;
    (line, column)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    dialect: Dialect,
}

impl Parser<'_> {
    fn error_at(&self, offset: usize, message: &str) -> anyhow::Error {
        let (line, column) = position(self.text, offset);
        let syntax = match self.dialect {
            Dialect::Json => "JSON",
            Dialect::Jsonc => "JSON with comments",
        };
        anyhow!("invalid {syntax} at line {line}, column {column}: {message}")
    }

    fn error(&self, message: &str) -> anyhow::Error {
        self.error_at(self.pos, message)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.pos += 1,
                Some(b'/') if self.dialect == Dialect::Jsonc => {
                    match self.bytes.get(self.pos + 1) {
                        Some(b'/') => {
                            while self.peek().is_some_and(|byte| byte != b'\n') {
                                self.pos += 1;
                            }
                        }
                        Some(b'*') => {
                            let start = self.pos;
                            self.pos += 2;
                            loop {
                                match self.peek() {
                                    None => {
                                        return Err(self.error_at(start, "unterminated comment"));
                                    }
                                    Some(b'*') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                                        self.pos += 2;
                                        break;
                                    }
                                    Some(_) => self.pos += 1,
                                }
                            }
                        }
                        _ => return Err(self.error("unexpected `/`")),
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node> {
        if depth > MAX_DEPTH {
            return Err(self.error("the document is nested too deeply"));
        }
        let start = self.pos;
        let scalar = |parser: &Self, value| Node {
            start,
            end: parser.pos,
            kind: NodeKind::Scalar(value),
        };
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => {
                let text = self.string()?;
                Ok(scalar(self, Value::String(text)))
            }
            Some(b't') => {
                self.literal("true")?;
                Ok(scalar(self, Value::Bool(true)))
            }
            Some(b'f') => {
                self.literal("false")?;
                Ok(scalar(self, Value::Bool(false)))
            }
            Some(b'n') => {
                self.literal("null")?;
                Ok(scalar(self, Value::Null))
            }
            Some(b'-' | b'0'..=b'9') => {
                let number = self.number()?;
                Ok(scalar(self, Value::Number(number)))
            }
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("unexpected end of file")),
        }
    }

    fn literal(&mut self, word: &str) -> Result<()> {
        if !self.text[self.pos..].starts_with(word) {
            return Err(self.error("expected a JSON value"));
        }
        self.pos += word.len();
        Ok(())
    }

    fn digits(&mut self) -> Result<()> {
        if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return Err(self.error("malformed number"));
        }
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.pos += 1;
        }
        Ok(())
    }

    fn number(&mut self) -> Result<Number> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        if self.peek() == Some(b'0') {
            self.pos += 1;
        } else {
            self.digits()?;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.digits()?;
        }
        self.text[start..self.pos]
            .parse()
            .map_err(|_| self.error_at(start, "number out of range"))
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits = self
            .text
            .get(self.pos..self.pos + 4)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("malformed \\u escape"))?;
        self.pos += 4;
        u32::from_str_radix(digits, 16).map_err(|_| self.error("malformed \\u escape"))
    }

    fn escape(&mut self, out: &mut String) -> Result<()> {
        let at = self.pos;
        self.pos += 1;
        let escaped = self
            .peek()
            .ok_or_else(|| self.error("unterminated string"))?;
        self.pos += 1;
        let character = match escaped {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let high = self.hex4()?;
                let code = if (0xD800..0xDC00).contains(&high) {
                    if !self.text[self.pos..].starts_with("\\u") {
                        return Err(self.error_at(at, "unpaired surrogate escape"));
                    }
                    self.pos += 2;
                    let low = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&low) {
                        return Err(self.error_at(at, "unpaired surrogate escape"));
                    }
                    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                } else {
                    high
                };
                char::from_u32(code)
                    .ok_or_else(|| self.error_at(at, "unpaired surrogate escape"))?
            }
            _ => return Err(self.error_at(at, "invalid escape")),
        };
        out.push(character);
        Ok(())
    }

    fn string(&mut self) -> Result<String> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.pos..];
            let Some(index) = rest
                .find(|character: char| character == '"' || character == '\\' || character < ' ')
            else {
                return Err(self.error_at(start, "unterminated string"));
            };
            out.push_str(&rest[..index]);
            self.pos += index;
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => self.escape(&mut out)?,
                _ => return Err(self.error("control character in a string")),
            }
        }
    }

    fn closing_after_comma(&mut self, close: u8) -> Result<bool> {
        self.skip_trivia()?;
        if self.peek() != Some(close) {
            return Ok(false);
        }
        if self.dialect == Dialect::Json {
            return Err(self.error("trailing comma"));
        }
        Ok(true)
    }

    fn array(&mut self, depth: usize) -> Result<Node> {
        let start = self.pos;
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_trivia()?;
        if self.peek() != Some(b']') {
            loop {
                items.push(self.value(depth + 1)?);
                self.skip_trivia()?;
                match self.peek() {
                    Some(b',') => {
                        self.pos += 1;
                        if self.closing_after_comma(b']')? {
                            break;
                        }
                    }
                    Some(b']') => break,
                    _ => return Err(self.error("expected `,` or `]`")),
                }
            }
        }
        self.pos += 1;
        Ok(Node {
            start,
            end: self.pos,
            kind: NodeKind::Array(items),
        })
    }

    fn object(&mut self, depth: usize) -> Result<Node> {
        let open = self.pos;
        self.pos += 1;
        let mut members: Vec<Member> = Vec::new();
        let mut keys = HashSet::new();
        self.skip_trivia()?;
        if self.peek() != Some(b'}') {
            loop {
                if self.peek() != Some(b'"') {
                    return Err(self.error("expected a quoted object key"));
                }
                let key_start = self.pos;
                let key = self.string()?;
                if !keys.insert(key.clone()) {
                    return Err(self.error_at(key_start, &format!("duplicate key {}", quote(&key))));
                }
                self.skip_trivia()?;
                if self.peek() != Some(b':') {
                    return Err(self.error("expected `:`"));
                }
                self.pos += 1;
                self.skip_trivia()?;
                let value = self.value(depth + 1)?;
                self.skip_trivia()?;
                let comma = match self.peek() {
                    Some(b',') => Some(self.pos),
                    Some(b'}') => None,
                    _ => return Err(self.error("expected `,` or `}`")),
                };
                members.push(Member {
                    key,
                    key_start,
                    value,
                    comma,
                });
                if comma.is_none() {
                    break;
                }
                self.pos += 1;
                if self.closing_after_comma(b'}')? {
                    break;
                }
            }
        }
        let close = self.pos;
        self.pos += 1;
        Ok(Node {
            start: open,
            end: self.pos,
            kind: NodeKind::Object(ObjectSyntax {
                open,
                close,
                members,
            }),
        })
    }
}

fn parse(text: &str, dialect: Dialect) -> Result<Node> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        dialect,
    };
    parser.skip_trivia()?;
    let node = parser.value(0)?;
    parser.skip_trivia()?;
    if parser.pos != text.len() {
        return Err(parser.error("unexpected text after the JSON document"));
    }
    Ok(node)
}

/// Parses `text` as a complete document of `dialect`, rejecting duplicate
/// keys at any depth.
pub(super) fn parse_value(text: &str, dialect: Dialect) -> Result<Value> {
    parse(text, dialect).map(|node| node.to_value())
}

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |index| index + 1)
}

fn indent_at(text: &str, pos: usize) -> &str {
    let line = &text[line_start(text, pos)..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn first_on_line(text: &str, pos: usize) -> bool {
    text[line_start(text, pos)..pos]
        .bytes()
        .all(|byte| byte == b' ' || byte == b'\t')
}

/// The document's indentation step, taken from its first top-level member.
fn indent_unit(text: &str, root: &ObjectSyntax) -> String {
    root.members
        .first()
        .filter(|member| first_on_line(text, member.key_start))
        .map(|member| indent_at(text, member.key_start).to_owned())
        .filter(|indent| !indent.is_empty())
        .unwrap_or_else(|| "  ".to_owned())
}

fn member_indent(text: &str, object: &ObjectSyntax, unit: &str) -> String {
    match object.members.first() {
        Some(member) if first_on_line(text, member.key_start) => {
            indent_at(text, member.key_start).to_owned()
        }
        _ => format!("{}{unit}", indent_at(text, object.open)),
    }
}

/// Applies non-overlapping `(start, end, replacement)` edits given in
/// ascending order. Edits at the same offset keep their order.
fn splice(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut out = text.to_owned();
    for (start, end, replacement) in edits.iter().rev() {
        out.replace_range(*start..*end, replacement);
    }
    out
}

fn insert_member(
    text: &str,
    object: &ObjectSyntax,
    key: &str,
    value: &Ordered,
    unit: &str,
) -> String {
    let indent = member_indent(text, object, unit);
    let member = format!("{}: {}", quote(key), value.render(&indent, unit));
    let close_indent = indent_at(text, object.open);
    let inner_end = object.open
        + 1
        + text[object.open + 1..object.close]
            .trim_end_matches([' ', '\t', '\r', '\n'])
            .len();
    let Some(last) = object.members.last() else {
        // Only whitespace or comments inside: keep the comments.
        return splice(
            text,
            &[(
                inner_end,
                object.close,
                format!("\n{indent}{member}\n{close_indent}"),
            )],
        );
    };
    let mut edits = Vec::new();
    if last.comma.is_none() {
        edits.push((last.value.end, last.value.end, ",".to_owned()));
    }
    edits.push((inner_end, inner_end, format!("\n{indent}{member}")));
    splice(text, &edits)
}

/// The end of a removed line: past trailing blanks and its newline when only
/// whitespace follows `pos` on that line.
fn through_line_end(text: &str, pos: usize) -> usize {
    let rest = &text[pos..];
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[blanks..];
    if after.starts_with("\r\n") {
        pos + blanks + 2
    } else if after.starts_with('\n') {
        pos + blanks + 1
    } else {
        pos + blanks
    }
}

fn remove_member(text: &str, object: &ObjectSyntax, index: usize) -> String {
    let member = &object.members[index];
    let own_line = first_on_line(text, member.key_start);
    if object.members.len() == 1 {
        let after = member.comma.map_or(member.value.end, |comma| comma + 1);
        // An object left with only whitespace collapses to `{}`.
        if text[object.open + 1..member.key_start].trim().is_empty()
            && text[after..object.close].trim().is_empty()
        {
            return splice(text, &[(object.open + 1, object.close, String::new())]);
        }
        // Comments stay; only the member's own text goes.
        let (start, end) = if own_line {
            (
                line_start(text, member.key_start),
                through_line_end(text, after),
            )
        } else {
            (member.key_start, after)
        };
        return splice(text, &[(start, end, String::new())]);
    }
    if let Some(comma) = member.comma {
        let start = if own_line {
            line_start(text, member.key_start)
        } else {
            member.key_start
        };
        let end = if own_line {
            through_line_end(text, comma + 1)
        } else {
            let rest = &text[comma + 1..];
            comma + 1 + rest.len() - rest.trim_start_matches([' ', '\t']).len()
        };
        return splice(text, &[(start, end, String::new())]);
    }
    // The last of several members: drop the comma before it and the line
    // break that leads to it.
    let previous = &object.members[index - 1];
    let previous_comma = previous
        .comma
        .expect("a member followed by another member has a comma");
    let start = if own_line {
        let line = line_start(text, member.key_start);
        let mut start = line.saturating_sub(1);
        if start > 0 && text.as_bytes()[start - 1] == b'\r' {
            start -= 1;
        }
        start.max(previous_comma + 1)
    } else {
        member.key_start
    };
    splice(
        text,
        &[
            (previous_comma, previous_comma + 1, String::new()),
            (start, member.value.end, String::new()),
        ],
    )
}

/// One JSON host's layout: the top-level member that maps server names to
/// entries, and the entry Splinterm writes.
pub(super) struct JsonTarget<'a> {
    pub(super) dialect: Dialect,
    pub(super) container: &'static str,
    pub(super) entry: &'a Ordered,
}

fn wrapped(target: &JsonTarget<'_>) -> Ordered {
    Ordered::Object(vec![(SERVER_NAME.to_owned(), target.entry.clone())])
}

fn fresh_document(target: &JsonTarget<'_>) -> String {
    let document = Ordered::Object(vec![(target.container.to_owned(), wrapped(target))]);
    format!("{}\n", document.render("", "  "))
}

/// Plans adding (or with `replace`, replacing) the `splinterm` entry.
pub(super) fn plan_setup(
    text: Option<&str>,
    target: &JsonTarget<'_>,
    replace: bool,
) -> Result<Plan> {
    let Some(text) = text else {
        return Ok(Plan::change(Action::Created, fresh_document(target)));
    };
    if text.trim().is_empty() {
        return Ok(Plan::change(Action::Added, fresh_document(target)));
    }
    let root = parse(text, target.dialect)?;
    let Some(root_object) = root.object() else {
        bail!("the top level is not a JSON object");
    };
    let unit = indent_unit(text, root_object);
    let mut expected = root.to_value();
    let entry = target.entry.to_value();
    let (action, new_text) = match root_object.member(target.container) {
        None => (
            Action::Added,
            insert_member(text, root_object, target.container, &wrapped(target), &unit),
        ),
        Some((_, container)) => {
            let Some(servers) = container.value.object() else {
                bail!("{} is not a JSON object", quote(target.container));
            };
            match servers.member(SERVER_NAME) {
                None => (
                    Action::Added,
                    insert_member(text, servers, SERVER_NAME, target.entry, &unit),
                ),
                Some((_, existing)) if existing.value.to_value() == entry => {
                    return Ok(Plan::unchanged(Action::Unchanged));
                }
                Some(_) if !replace => return Err(super::conflict()),
                Some((_, existing)) => {
                    let indent = if first_on_line(text, existing.key_start) {
                        indent_at(text, existing.key_start).to_owned()
                    } else {
                        member_indent(text, servers, &unit)
                    };
                    (
                        Action::Replaced,
                        splice(
                            text,
                            &[(
                                existing.value.start,
                                existing.value.end,
                                target.entry.render(&indent, &unit),
                            )],
                        ),
                    )
                }
            }
        }
    };
    let servers = expected
        .as_object_mut()
        .expect("the root is an object")
        .entry(target.container)
        .or_insert_with(|| Value::Object(Map::new()));
    servers
        .as_object_mut()
        .expect("the container is an object")
        .insert(SERVER_NAME.to_owned(), entry);
    verify(&new_text, target.dialect, &expected)?;
    Ok(Plan::change(action, new_text))
}

/// Plans removing the `splinterm` entry, leaving every other member.
pub(super) fn plan_remove(text: Option<&str>, target: &JsonTarget<'_>) -> Result<Plan> {
    let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
        return Ok(Plan::unchanged(Action::Absent));
    };
    let root = parse(text, target.dialect)?;
    let Some(root_object) = root.object() else {
        bail!("the top level is not a JSON object");
    };
    let Some((_, container)) = root_object.member(target.container) else {
        return Ok(Plan::unchanged(Action::Absent));
    };
    let Some(servers) = container.value.object() else {
        bail!("{} is not a JSON object", quote(target.container));
    };
    let Some((index, member)) = servers.member(SERVER_NAME) else {
        return Ok(Plan::unchanged(Action::Absent));
    };
    let command = member.value.to_value()["command"]
        .as_str()
        .map(str::to_owned);
    let new_text = remove_member(text, servers, index);
    let mut expected = root.to_value();
    expected[target.container]
        .as_object_mut()
        .expect("the container is an object")
        .remove(SERVER_NAME);
    verify(&new_text, target.dialect, &expected)?;
    Ok(Plan::removal(new_text, command))
}

fn verify(text: &str, dialect: Dialect, expected: &Value) -> Result<()> {
    match parse_value(text, dialect) {
        Ok(actual) if actual == *expected => Ok(()),
        _ => Err(super::unsafe_edit()),
    }
}
