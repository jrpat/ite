//! Transforms JSON input into source-neutral tree data.
//!
//! The complete JSON boundary: the input document is read to the end, a
//! structural scan indexes every value's byte span, and the tree retains the
//! raw bytes. Labels, previews, pointers, and outputs are derived from the
//! spans on demand through the crate-private helpers at the bottom, so no
//! second representation of the document is stored. No JSON values escape
//! this module.

use std::fmt;
use std::io::{self, Read, Write};
use std::ops::Range;

use serde_json::Value;

use crate::tree::{JsonKey, NodeId, Tree};

/// Enough text to fill an unusually wide terminal without retaining an
/// unbounded second representation of every object.
const MAX_OBJECT_PREVIEW_BYTES: usize = 512;
/// Bounds the search for previewable scalars when early members are containers.
const MAX_OBJECT_PREVIEW_MEMBERS: usize = 32;
/// Matches serde_json's default recursion limit, which the scanner replaced.
const MAX_DEPTH: usize = 128;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid JSON input: {}", self.0)
    }
}

impl std::error::Error for Error {}

/// Read the input, detect JSON vs JSONL from the content (one uniform rule,
/// however the input arrived), and build the tree.
pub fn from_reader(mut reader: impl Read) -> Result<Tree, Error> {
    let bytes = slurp(reader.by_ref())?;
    if detect_jsonl(&bytes) {
        jsonl_from_bytes(bytes)
    } else {
        json_from_bytes(bytes)
    }
}

/// Read the input as JSONL regardless of what detection would say — the
/// `--jsonl` escape hatch for content that is also valid JSON (e.g. a single
/// record).
pub fn jsonl_from_reader(mut reader: impl Read) -> Result<Tree, Error> {
    let bytes = slurp(reader.by_ref())?;
    jsonl_from_bytes(bytes)
}

fn slurp(reader: &mut impl Read) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| Error(error.to_string()))?;
    if u32::try_from(bytes.len()).is_err() {
        return Err(Error(
            "input exceeds the 4 GiB the span index addresses".to_owned(),
        ));
    }
    if let Err(error) = std::str::from_utf8(&bytes) {
        return Err(Error(error.to_string()));
    }
    Ok(bytes)
}

/// Content detection: JSONL iff non-whitespace content follows the first
/// newline AND the first line is exactly one complete JSON value. Pretty
/// JSON's first line is incomplete; a minified document has nothing after its
/// only newline; the genuinely ambiguous single-record case reads as JSON,
/// which `--jsonl` overrides.
fn detect_jsonl(bytes: &[u8]) -> bool {
    let Some(start) = bytes
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    else {
        return false;
    };
    let content = &bytes[start..];
    let Some(newline) = content.iter().position(|&b| b == b'\n') else {
        return false;
    };
    let (first_line, rest) = content.split_at(newline);
    if rest
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return false;
    }
    is_complete_value(first_line)
}

/// Whether `bytes` holds exactly one complete JSON value (plus whitespace).
fn is_complete_value(bytes: &[u8]) -> bool {
    let mut throwaway = Tree::new();
    let mut scanner = Scanner::new(bytes);
    if scanner
        .scan_value(&mut throwaway, None, JsonKey::Root)
        .is_err()
    {
        return false;
    }
    scanner.skip_ws();
    scanner.pos == bytes.len()
}

fn json_from_bytes(bytes: Vec<u8>) -> Result<Tree, Error> {
    let mut tree = Tree::new();
    let mut scanner = Scanner::new(&bytes);
    scan_document(&mut scanner, &mut tree)?;
    scanner.skip_ws();
    if scanner.pos != bytes.len() {
        return Err(scanner.syntax_error("trailing characters"));
    }
    tree.set_json_source(bytes);
    Ok(tree)
}

/// Build the JSONL tree: a virtual array root over one record per non-blank
/// line. Indices are record ordinals, not line numbers (error messages carry
/// line numbers). A final record that fails to scan — the classic truncated
/// tail — is dropped; a malformed record anywhere else fails the load.
fn jsonl_from_bytes(bytes: Vec<u8>) -> Result<Tree, Error> {
    let mut tree = Tree::new();
    let root = tree.push_json(None, 0..bytes.len() as u32, JsonKey::JsonlRoot, true);
    let lines = non_blank_lines(&bytes);
    for (ordinal, line) in lines.iter().enumerate() {
        let mark = tree.len();
        // Bound the scanner to the record's line so inter-token whitespace
        // cannot swallow the next line's bytes.
        let mut scanner = Scanner::new(&bytes[..line.end]);
        scanner.pos = line.start;
        let scanned = scanner
            .scan_value(&mut tree, Some(root), JsonKey::Index(ordinal as u32))
            .and_then(|_| {
                scanner.skip_ws();
                if scanner.pos == line.end {
                    Ok(())
                } else {
                    Err(scanner.syntax_error("trailing characters"))
                }
            });
        if let Err(error) = scanned {
            if ordinal == lines.len() - 1 {
                tree.truncate(mark);
            } else {
                return Err(error);
            }
        }
    }
    tree.set_json_source(bytes);
    Ok(tree)
}

/// Byte ranges of the lines holding non-whitespace content, in order.
fn non_blank_lines(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (position, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            push_if_non_blank(bytes, start..position, &mut lines);
            start = position + 1;
        }
    }
    push_if_non_blank(bytes, start..bytes.len(), &mut lines);
    lines
}

fn push_if_non_blank(bytes: &[u8], range: Range<usize>, lines: &mut Vec<Range<usize>>) {
    if bytes[range.clone()]
        .iter()
        .any(|b| !matches!(b, b' ' | b'\t' | b'\r'))
    {
        lines.push(range);
    }
}

/// Scan the document, pushing nodes. A non-empty top-level object spreads its
/// members as forest roots (how the tree has always presented documents);
/// every other document gets a single `$` root.
fn scan_document(scanner: &mut Scanner, tree: &mut Tree) -> Result<(), Error> {
    scanner.skip_ws();
    if scanner.peek() == Some(b'{') {
        let probe = scanner.pos;
        scanner.pos += 1;
        scanner.skip_ws();
        let has_members = scanner.peek() != Some(b'}');
        scanner.pos = probe;
        if has_members {
            return scanner.scan_object_members(tree, None);
        }
    }
    scanner.scan_value(tree, None, JsonKey::Root).map(|_| ())
}

/// A structural scanner: validates the document grammar while recording byte
/// spans, without building values. Scalar contents are re-parsed lazily at
/// display time, so validation here is strict about shape (strings, escapes,
/// number grammar, literals) and leaves range checks to derivation.
struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Scanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.syntax_error(&format!("expected `{}`", byte as char)))
        }
    }

    fn syntax_error(&self, message: &str) -> Error {
        let consumed = &self.bytes[..self.pos.min(self.bytes.len())];
        let line = 1 + consumed.iter().filter(|&&b| b == b'\n').count();
        let line_start = consumed.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let column = self.pos - line_start + 1;
        Error(format!("{message} at line {line} column {column}"))
    }

    /// Scan one complete value, pushing its node (and its descendants').
    fn scan_value(
        &mut self,
        tree: &mut Tree,
        parent: Option<NodeId>,
        key: JsonKey,
    ) -> Result<NodeId, Error> {
        self.skip_ws();
        let start = self.pos as u32;
        match self.peek() {
            Some(b'{') => {
                let id = tree.push_json(parent, start..start, key, true);
                self.descend()?;
                self.scan_object_members(tree, Some(id))?;
                self.depth -= 1;
                tree.set_json_span_end(id, self.pos as u32);
                Ok(id)
            }
            Some(b'[') => {
                let id = tree.push_json(parent, start..start, key, true);
                self.descend()?;
                self.scan_array_elements(tree, id)?;
                self.depth -= 1;
                tree.set_json_span_end(id, self.pos as u32);
                Ok(id)
            }
            _ => {
                self.scan_scalar()?;
                Ok(tree.push_json(parent, start..self.pos as u32, key, false))
            }
        }
    }

    fn descend(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.syntax_error("recursion limit exceeded"));
        }
        Ok(())
    }

    /// Scan `{ "key": value, ... }`, pushing each member under `parent`
    /// (`None` spreads the members as forest roots).
    fn scan_object_members(&mut self, tree: &mut Tree, parent: Option<NodeId>) -> Result<(), Error> {
        self.expect(b'{')?;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(());
        }
        loop {
            self.skip_ws();
            let key_start = self.pos as u32;
            self.scan_string()?;
            let key_span = key_start..self.pos as u32;
            self.skip_ws();
            self.expect(b':')?;
            self.scan_value(tree, parent, JsonKey::Member { key_span })?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err(self.syntax_error("expected `,` or `}`")),
            }
        }
    }

    fn scan_array_elements(&mut self, tree: &mut Tree, parent: NodeId) -> Result<(), Error> {
        self.expect(b'[')?;
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(());
        }
        let mut index = 0;
        loop {
            self.scan_value(tree, Some(parent), JsonKey::Index(index))?;
            index += 1;
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err(self.syntax_error("expected `,` or `]`")),
            }
        }
    }

    fn scan_scalar(&mut self) -> Result<(), Error> {
        match self.peek() {
            Some(b'"') => self.scan_string(),
            Some(b't') => self.scan_literal("true"),
            Some(b'f') => self.scan_literal("false"),
            Some(b'n') => self.scan_literal("null"),
            Some(b'-' | b'0'..=b'9') => self.scan_number(),
            Some(other) => Err(self.syntax_error(&format!("unexpected character `{}`", other as char))),
            None => Err(self.syntax_error("unexpected end of input")),
        }
    }

    fn scan_literal(&mut self, literal: &str) -> Result<(), Error> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(())
        } else {
            Err(self.syntax_error(&format!("expected `{literal}`")))
        }
    }

    fn scan_string(&mut self) -> Result<(), Error> {
        self.expect(b'"')?;
        loop {
            match self.peek() {
                None => return Err(self.syntax_error("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.pos += 1;
                        }
                        Some(b'u') => {
                            self.pos += 1;
                            for _ in 0..4 {
                                if !self.peek().is_some_and(|b| b.is_ascii_hexdigit()) {
                                    return Err(self.syntax_error("invalid \\u escape"));
                                }
                                self.pos += 1;
                            }
                        }
                        _ => return Err(self.syntax_error("invalid escape")),
                    }
                }
                Some(byte) if byte < 0x20 => {
                    return Err(self.syntax_error("control character in string"));
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn scan_number(&mut self) -> Result<(), Error> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.syntax_error("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.syntax_error("invalid number"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.syntax_error("invalid number"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        Ok(())
    }
}

// --- Span-derivation helpers used by `Tree`'s accessors ---

fn slice<'a>(bytes: &'a [u8], span: &Range<u32>) -> &'a [u8] {
    &bytes[span.start as usize..span.end as usize]
}

/// The unescaped text of a raw key token (quotes included in the span).
pub(crate) fn key_text(bytes: &[u8], span: &Range<u32>) -> String {
    let raw = slice(bytes, span);
    serde_json::from_slice(raw).unwrap_or_else(|_| String::from_utf8_lossy(raw).into_owned())
}

/// The compact serialization of the value at `span` — scalars for labels,
/// whole subtrees for the alternate output. Falls back to the raw text when
/// the span holds something serde rejects (e.g. an out-of-range number).
pub(crate) fn value_text(bytes: &[u8], span: &Range<u32>) -> String {
    let raw = slice(bytes, span);
    match serde_json::from_slice::<Value>(raw) {
        Ok(value) => {
            serde_json::to_string(&value).expect("serializing a parsed JSON value cannot fail")
        }
        Err(_) => String::from_utf8_lossy(raw).into_owned(),
    }
}

pub(crate) fn append_pointer(parent: &str, token: &str) -> String {
    let token = token.replace('~', "~0").replace('/', "~1");
    format!("{parent}/{token}")
}

/// Preview of an object's leading scalar members: `Some((key span, value
/// span))` per scalar member, `None` per container member — containers occupy
/// one of the inspected slots but contribute no text.
pub(crate) fn object_preview(
    bytes: &[u8],
    members: impl Iterator<Item = Option<(Range<u32>, Range<u32>)>>,
) -> Option<String> {
    let mut preview = Preview::new(MAX_OBJECT_PREVIEW_BYTES);
    for member in members.take(MAX_OBJECT_PREVIEW_MEMBERS) {
        let Some((key_span, value_span)) = member else {
            continue;
        };
        if !preview.is_empty() && !preview.push_str(" · ") {
            break;
        }
        if !preview.push_str(&key_text(bytes, &key_span)) || !preview.push_str(": ") {
            break;
        }
        preview.push_value(bytes, &value_span);
        if preview.is_exhausted() {
            break;
        }
    }
    preview.finish()
}

struct Preview {
    text: String,
    limit: usize,
    exhausted: bool,
}

impl Preview {
    fn new(limit: usize) -> Self {
        Self {
            text: String::with_capacity(limit),
            limit,
            exhausted: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    fn push_str(&mut self, text: &str) -> bool {
        if self.exhausted {
            return false;
        }
        let remaining = self.limit.saturating_sub(self.text.len());
        if text.len() <= remaining {
            self.text.push_str(text);
            return true;
        }

        let mut end = remaining;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
        self.exhausted = true;
        false
    }

    fn push_value(&mut self, bytes: &[u8], span: &Range<u32>) {
        match serde_json::from_slice::<Value>(slice(bytes, span)) {
            Ok(value) => {
                if serde_json::to_writer(&mut *self, &value).is_err() {
                    self.exhausted = true;
                }
            }
            Err(_) => {
                self.push_str(&String::from_utf8_lossy(slice(bytes, span)));
            }
        }
    }

    fn finish(self) -> Option<String> {
        (!self.text.is_empty()).then_some(self.text)
    }
}

impl Write for Preview {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.exhausted {
            return Err(io::ErrorKind::WriteZero.into());
        }

        let remaining = self.limit.saturating_sub(self.text.len());
        let candidate = &bytes[..bytes.len().min(remaining)];
        let end = std::str::from_utf8(candidate).map_or_else(|error| error.valid_up_to(), str::len);
        if end == 0 {
            self.exhausted = true;
            return Err(io::ErrorKind::WriteZero.into());
        }

        self.text.push_str(
            std::str::from_utf8(&candidate[..end])
                .expect("a prefix ending on a UTF-8 boundary is valid"),
        );
        self.exhausted = end < bytes.len();
        Ok(end)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    const DEMO_JSON: &str = include_str!("../examples/sample.json");

    fn parse(json: &str) -> Tree {
        from_reader(json.as_bytes()).unwrap()
    }

    fn names(tree: &Tree, ids: &[usize]) -> Vec<String> {
        ids.iter().map(|&id| tree.name(id)).collect()
    }

    #[test]
    fn object_members_become_ordered_roots_with_container_sizes_and_scalar_values() {
        let tree = parse(
            r#"{
                "users": [{"id": 12, "name": "Ada"}, {"id": 27}, null],
                "empty": [],
                "settings": {},
                "enabled": true
            }"#,
        );

        assert_eq!(
            names(&tree, tree.root_ids()),
            ["users [3]", "empty []", "settings {}", "enabled: true"]
        );
        assert!(tree.is_container(tree.root_ids()[0]));
        assert!(tree.is_container(tree.root_ids()[1]));
        assert!(tree.is_leaf(tree.root_ids()[1]));
    }

    #[test]
    fn array_elements_keep_their_order_and_include_object_previews() {
        let tree = parse(r#"["rust", 7, {"id": 12, "name": "Ada"}, [null]]"#);
        let root = tree.root_ids()[0];
        let object = tree.children_of(root)[2];

        assert_eq!(tree.name(root), "$ [4]");
        assert_eq!(
            names(&tree, tree.children_of(root)),
            ["[0]: \"rust\"", "[1]: 7", "[2] {2}", "[3] [1]"]
        );
        assert_eq!(
            tree.detail(object).as_deref(),
            Some(r#"id: 12 · name: "Ada""#)
        );
    }

    #[test]
    fn object_previews_include_more_than_two_scalar_members() {
        let tree = parse(r#"{"item":{"a":1,"b":2,"c":3}}"#);
        let item = tree.root_ids()[0];

        assert_eq!(tree.detail(item).as_deref(), Some("a: 1 · b: 2 · c: 3"));
    }

    #[test]
    fn object_previews_are_bounded_and_remain_valid_utf8() {
        let json = format!(r#"{{"item":{{"huge":"{}"}}}}"#, "😀".repeat(1_000));
        let tree = parse(&json);
        let item = tree.root_ids()[0];
        let preview = tree.detail(item).unwrap();

        assert!(preview.len() <= 512, "preview used {} bytes", preview.len());
        assert!(preview.starts_with(r#"huge: ""#));
    }

    #[test]
    fn object_previews_inspect_at_most_the_first_32_members() {
        let mut object = serde_json::Map::new();
        for index in 0..32 {
            object.insert(format!("nested-{index}"), Value::Array(Vec::new()));
        }
        object.insert("too-late".to_owned(), Value::Bool(true));
        let document = Value::Object(
            [("item".to_owned(), Value::Object(object))]
                .into_iter()
                .collect(),
        );
        let tree = parse(&document.to_string());
        let item = tree.root_ids()[0];

        assert_eq!(tree.detail(item), None);
    }

    #[test]
    fn every_node_outputs_its_canonical_json_pointer() {
        let tree = parse(r#"["rust", {"a/b": {"~key": "value"}}]"#);
        let root = tree.root_ids()[0];
        let text = tree.children_of(root)[0];
        let object = tree.children_of(root)[1];
        let slash_key = tree.children_of(object)[0];
        let tilde_key = tree.children_of(slash_key)[0];

        assert_eq!(tree.output(root), OsStr::new(""));
        assert_eq!(
            tree.alternate_output(root),
            OsStr::new(r#"["rust",{"a/b":{"~key":"value"}}]"#)
        );
        assert_eq!(tree.path(root), OsStr::new(""));
        assert_eq!(tree.output(text), OsStr::new("/0"));
        assert_eq!(tree.alternate_output(text), OsStr::new(r#""rust""#));
        assert_eq!(tree.path(text), OsStr::new("/0"));
        assert_eq!(tree.output(slash_key), OsStr::new("/1/a~1b"));
        assert_eq!(tree.path(slash_key), OsStr::new("/1/a~1b"));
        assert_eq!(tree.path(tilde_key), OsStr::new("/1/a~1b/~0key"));
        assert_eq!(tree.relpath(tilde_key), tree.path(tilde_key));
        assert_eq!(tree.output(tilde_key), OsStr::new("/1/a~1b/~0key"));
        assert_eq!(tree.alternate_output(tilde_key), OsStr::new(r#""value""#));
    }

    #[test]
    fn scalar_and_empty_object_roots_remain_selectable() {
        let scalar = parse("null");
        assert_eq!(names(&scalar, scalar.root_ids()), ["$: null"]);
        assert_eq!(scalar.output(scalar.root_ids()[0]), OsStr::new(""));

        let empty = parse("{}");
        assert_eq!(names(&empty, empty.root_ids()), ["$ {}"]);
        assert_eq!(empty.output(empty.root_ids()[0]), OsStr::new(""));
        assert!(empty.is_container(empty.root_ids()[0]));
        assert!(empty.is_leaf(empty.root_ids()[0]));
    }

    #[test]
    fn invalid_json_is_reported() {
        let error = from_reader("{]".as_bytes()).unwrap_err();
        assert!(error.to_string().starts_with("invalid JSON input:"));
    }

    #[test]
    fn trailing_content_after_the_document_is_an_error() {
        assert!(from_reader(r#"{"a": 1} extra"#.as_bytes()).is_err());
        assert!(from_reader("[1, 2] [3]".as_bytes()).is_err());
        assert!(from_reader("null null".as_bytes()).is_err());
    }

    #[test]
    fn truncated_documents_are_an_error() {
        assert!(from_reader(r#"{"a": "#.as_bytes()).is_err());
        assert!(from_reader(r#"["unterminated"#.as_bytes()).is_err());
        assert!(from_reader("".as_bytes()).is_err());
    }

    #[test]
    fn escaped_keys_and_strings_render_unescaped_labels() {
        let tree = parse("{\"k\\u0065y\": \"va\\u0041lue\"}");
        assert_eq!(names(&tree, tree.root_ids()), [r#"key: "vaAlue""#]);
    }

    #[test]
    fn numbers_render_through_json_value_normalization() {
        let tree = parse(r#"[1e3, 0.5, -0]"#);
        let root = tree.root_ids()[0];
        assert_eq!(
            names(&tree, tree.children_of(root)),
            ["[0]: 1000.0", "[1]: 0.5", "[2]: -0.0"]
        );
    }

    #[test]
    fn jsonl_content_is_detected_and_presents_as_a_virtual_array() {
        let tree = parse("{\"a\": 1}\n{\"a\": 2}\n");
        let root = tree.root_ids()[0];

        assert_eq!(names(&tree, tree.root_ids()), ["$ [2]"]);
        assert!(tree.is_container(root));
        assert_eq!(names(&tree, tree.children_of(root)), ["[0] {1}", "[1] {1}"]);
    }

    #[test]
    fn jsonl_detection_accepts_scalar_records() {
        // Valid JSONL, invalid JSON — two number records.
        let tree = parse("1\n2\n");
        let root = tree.root_ids()[0];
        assert_eq!(names(&tree, tree.root_ids()), ["$ [2]"]);
        assert_eq!(names(&tree, tree.children_of(root)), ["[0]: 1", "[1]: 2"]);
    }

    #[test]
    fn a_single_record_with_trailing_newline_stays_json() {
        // Simultaneously valid JSON and one-record JSONL; detection picks
        // JSON, and --jsonl exists to force the other reading.
        let tree = parse("{\"a\": 1}\n");
        assert_eq!(names(&tree, tree.root_ids()), ["a: 1"]);
    }

    #[test]
    fn pretty_printed_json_is_not_mistaken_for_jsonl() {
        let tree = parse("{\n  \"a\": 1,\n  \"b\": 2\n}\n");
        assert_eq!(names(&tree, tree.root_ids()), ["a: 1", "b: 2"]);
    }

    #[test]
    fn jsonl_skips_blank_lines_and_indices_are_record_ordinals() {
        let tree = parse("1\n\n  \n2\n");
        let root = tree.root_ids()[0];
        assert_eq!(names(&tree, tree.root_ids()), ["$ [2]"]);
        assert_eq!(names(&tree, tree.children_of(root)), ["[0]: 1", "[1]: 2"]);
    }

    #[test]
    fn jsonl_drops_a_truncated_final_record() {
        let tree = parse("{\"a\": 1}\n{\"b\": tru");
        let root = tree.root_ids()[0];
        assert_eq!(names(&tree, tree.root_ids()), ["$ [1]"]);
        assert_eq!(names(&tree, tree.children_of(root)), ["[0] {1}"]);
    }

    #[test]
    fn jsonl_fails_loudly_on_a_malformed_middle_record() {
        let error = from_reader("{\"a\": 1}\nxxx\n{\"b\": 2}\n".as_bytes()).unwrap_err();
        assert!(error.to_string().starts_with("invalid JSON input:"));
    }

    #[test]
    fn jsonl_nodes_carry_global_and_record_relative_addresses() {
        let tree = parse("{\"user\": {\"name\": \"Ada\"}}\n{\"user\": {\"name\": \"Bo\"}}\n");
        let root = tree.root_ids()[0];
        let record = tree.children_of(root)[1];
        let user = tree.children_of(record)[0];
        let name = tree.children_of(user)[0];

        assert_eq!(tree.path(root), OsStr::new(""));
        assert_eq!(tree.path(record), OsStr::new("/1"));
        assert_eq!(tree.relpath(record), OsStr::new(""));
        assert_eq!(tree.output(record), OsStr::new("/1"));
        assert_eq!(
            tree.alternate_output(record),
            OsStr::new(r#"{"user":{"name":"Bo"}}"#)
        );
        assert_eq!(tree.path(name), OsStr::new("/1/user/name"));
        assert_eq!(tree.relpath(name), OsStr::new("/user/name"));
        assert_eq!(tree.jump_key(name), "/1/user/name");
        assert_eq!(tree.jump_key(record), "/1");
    }

    #[test]
    fn forced_jsonl_reads_a_single_record_as_one_element() {
        let tree = jsonl_from_reader("{\"a\": 1}\n".as_bytes()).unwrap();
        let root = tree.root_ids()[0];
        assert_eq!(names(&tree, tree.root_ids()), ["$ [1]"]);
        assert_eq!(names(&tree, tree.children_of(root)), ["[0] {1}"]);
    }

    #[test]
    fn demo_sample_exercises_the_json_tree_shapes() {
        let tree = parse(DEMO_JSON);

        assert_eq!(
            names(&tree, tree.root_ids()),
            ["project {4}", "users [3]", "settings {}", "version: 1"]
        );
        assert_eq!(
            tree.detail(tree.root_ids()[0]).as_deref(),
            Some(r#"name: "ite" · status: "experimental""#)
        );
        let users = tree.root_ids()[1];
        assert_eq!(tree.children_of(users).len(), 3);
        assert_eq!(tree.name(tree.children_of(users)[2]), "[2]: null");
    }
}
