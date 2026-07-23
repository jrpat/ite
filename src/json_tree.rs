//! Transforms JSON input into source-neutral tree data.

use std::fmt;
use std::io::{self, Read, Write};

use serde_json::Value;

use crate::tree::{ActionValues, NodeId, Tree};

/// Enough text to fill an unusually wide terminal without retaining an
/// unbounded second representation of every object.
const MAX_OBJECT_PREVIEW_BYTES: usize = 512;
/// Bounds the search for previewable scalars when early members are containers.
const MAX_OBJECT_PREVIEW_MEMBERS: usize = 32;

#[derive(Debug)]
pub struct Error(serde_json::Error);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid JSON input: {}", self.0)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub fn from_reader(reader: impl Read) -> Result<Tree, Error> {
    let value = serde_json::from_reader(reader).map_err(Error)?;
    Ok(transform(&value))
}

fn transform(value: &Value) -> Tree {
    let mut tree = Tree::new();
    match value {
        Value::Object(members) if !members.is_empty() => {
            for (key, value) in members {
                push_value(&mut tree, None, key, &append_pointer("", key), value);
            }
        }
        _ => {
            push_value(&mut tree, None, "$", "", value);
        }
    }
    tree
}

fn push_value(
    tree: &mut Tree,
    parent: Option<NodeId>,
    prefix: &str,
    pointer: &str,
    value: &Value,
) -> NodeId {
    let (name, detail) = label(prefix, value);
    let alternate_output =
        serde_json::to_string(value).expect("serializing a JSON value cannot fail");
    let action = ActionValues::new(pointer, pointer, pointer)
        .with_alternate_output(alternate_output);
    let id = tree.push_with_detail(
        parent,
        name,
        detail,
        matches!(value, Value::Array(_) | Value::Object(_)),
        action,
    );

    match value {
        Value::Array(elements) => {
            for (index, value) in elements.iter().enumerate() {
                let index = index.to_string();
                push_value(
                    tree,
                    Some(id),
                    &format!("[{index}]"),
                    &append_pointer(pointer, &index),
                    value,
                );
            }
        }
        Value::Object(members) => {
            for (key, value) in members {
                push_value(tree, Some(id), key, &append_pointer(pointer, key), value);
            }
        }
        _ => {}
    }
    id
}

fn label(prefix: &str, value: &Value) -> (String, Option<String>) {
    match value {
        Value::Array(elements) if elements.is_empty() => (format!("{prefix} []"), None),
        Value::Array(elements) => (format!("{prefix} [{}]", elements.len()), None),
        Value::Object(members) if members.is_empty() => (format!("{prefix} {{}}"), None),
        Value::Object(members) => (
            format!("{prefix} {{{}}}", members.len()),
            object_preview(members),
        ),
        _ => (
            format!(
                "{prefix}: {}",
                scalar(value).expect("non-container JSON values are scalar")
            ),
            None,
        ),
    }
}

fn object_preview(members: &serde_json::Map<String, Value>) -> Option<String> {
    let mut preview = Preview::new(MAX_OBJECT_PREVIEW_BYTES);
    for (key, value) in members.iter().take(MAX_OBJECT_PREVIEW_MEMBERS) {
        if !matches!(
            value,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        ) {
            continue;
        }
        if !preview.is_empty() && !preview.push_str(" · ") {
            break;
        }
        if !preview.push_str(key) || !preview.push_str(": ") {
            break;
        }
        preview.push_json(value);
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

    fn push_json(&mut self, value: &Value) {
        if serde_json::to_writer(&mut *self, value).is_err() {
            self.exhausted = true;
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

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            Some(serde_json::to_string(value).expect("serializing a JSON scalar cannot fail"))
        }
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn append_pointer(parent: &str, token: &str) -> String {
    let token = token.replace('~', "~0").replace('/', "~1");
    format!("{parent}/{token}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    const DEMO_JSON: &str = include_str!("../examples/sample.json");

    fn parse(json: &str) -> Tree {
        from_reader(json.as_bytes()).unwrap()
    }

    fn names<'a>(tree: &'a Tree, ids: &[usize]) -> Vec<&'a str> {
        ids.iter().map(|&id| tree.node(id).name.as_str()).collect()
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
        assert!(tree.node(tree.root_ids()[0]).is_container);
        assert!(tree.node(tree.root_ids()[1]).is_container);
        assert!(tree.is_leaf(tree.root_ids()[1]));
    }

    #[test]
    fn array_elements_keep_their_order_and_include_object_previews() {
        let tree = parse(r#"["rust", 7, {"id": 12, "name": "Ada"}, [null]]"#);
        let root = tree.root_ids()[0];
        let object = tree.node(root).children[2];

        assert_eq!(tree.node(root).name, "$ [4]");
        assert_eq!(
            names(&tree, &tree.node(root).children),
            ["[0]: \"rust\"", "[1]: 7", "[2] {2}", "[3] [1]"]
        );
        assert_eq!(
            tree.node(object).detail.as_deref(),
            Some(r#"id: 12 · name: "Ada""#)
        );
    }

    #[test]
    fn object_previews_include_more_than_two_scalar_members() {
        let tree = parse(r#"{"item":{"a":1,"b":2,"c":3}}"#);
        let item = tree.root_ids()[0];

        assert_eq!(
            tree.node(item).detail.as_deref(),
            Some("a: 1 · b: 2 · c: 3")
        );
    }

    #[test]
    fn object_previews_are_bounded_and_remain_valid_utf8() {
        let json = format!(r#"{{"item":{{"huge":"{}"}}}}"#, "😀".repeat(1_000));
        let tree = parse(&json);
        let item = tree.root_ids()[0];
        let preview = tree.node(item).detail.as_deref().unwrap();

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
        let tree = transform(&document);
        let item = tree.root_ids()[0];

        assert_eq!(tree.node(item).detail, None);
    }

    #[test]
    fn every_node_outputs_its_canonical_json_pointer() {
        let tree = parse(r#"["rust", {"a/b": {"~key": "value"}}]"#);
        let root = tree.root_ids()[0];
        let text = tree.node(root).children[0];
        let object = tree.node(root).children[1];
        let slash_key = tree.node(object).children[0];
        let tilde_key = tree.node(slash_key).children[0];

        assert_eq!(tree.node(root).action.output, OsStr::new(""));
        assert_eq!(
            tree.node(root).action.alternate_output,
            OsStr::new(r#"["rust",{"a/b":{"~key":"value"}}]"#)
        );
        assert_eq!(tree.node(root).action.path, OsStr::new(""));
        assert_eq!(tree.node(text).action.output, OsStr::new("/0"));
        assert_eq!(
            tree.node(text).action.alternate_output,
            OsStr::new(r#""rust""#)
        );
        assert_eq!(tree.node(text).action.path, OsStr::new("/0"));
        assert_eq!(tree.node(slash_key).action.output, OsStr::new("/1/a~1b"));
        assert_eq!(tree.node(slash_key).action.path, OsStr::new("/1/a~1b"));
        assert_eq!(
            tree.node(tilde_key).action.path,
            OsStr::new("/1/a~1b/~0key")
        );
        assert_eq!(
            tree.node(tilde_key).action.relpath,
            tree.node(tilde_key).action.path
        );
        assert_eq!(
            tree.node(tilde_key).action.output,
            OsStr::new("/1/a~1b/~0key")
        );
        assert_eq!(
            tree.node(tilde_key).action.alternate_output,
            OsStr::new(r#""value""#)
        );
    }

    #[test]
    fn scalar_and_empty_object_roots_remain_selectable() {
        let scalar = parse("null");
        assert_eq!(names(&scalar, scalar.root_ids()), ["$: null"]);
        assert_eq!(
            scalar.node(scalar.root_ids()[0]).action.output,
            OsStr::new("")
        );

        let empty = parse("{}");
        assert_eq!(names(&empty, empty.root_ids()), ["$ {}"]);
        assert_eq!(
            empty.node(empty.root_ids()[0]).action.output,
            OsStr::new("")
        );
        assert!(empty.node(empty.root_ids()[0]).is_container);
        assert!(empty.is_leaf(empty.root_ids()[0]));
    }

    #[test]
    fn invalid_json_is_reported() {
        let error = from_reader("{]".as_bytes()).unwrap_err();
        assert!(error.to_string().starts_with("invalid JSON input:"));
    }

    #[test]
    fn demo_sample_exercises_the_json_tree_shapes() {
        let tree = parse(DEMO_JSON);

        assert_eq!(
            names(&tree, tree.root_ids()),
            ["project {4}", "users [3]", "settings {}", "version: 1"]
        );
        assert_eq!(
            tree.node(tree.root_ids()[0]).detail.as_deref(),
            Some(r#"name: "ite" · status: "experimental""#)
        );
        let users = tree.root_ids()[1];
        assert_eq!(tree.node(users).children.len(), 3);
        assert_eq!(tree.node(tree.node(users).children[2]).name, "[2]: null");
    }
}
