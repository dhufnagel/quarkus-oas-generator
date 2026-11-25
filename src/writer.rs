//! Serializes the model like SmallRye's Jackson based writer: JSON with Jackson's default pretty printer and
//! YAML with jackson-dataformat-yaml (`MINIMIZE_QUOTES`, `ALWAYS_QUOTE_NUMBERS_AS_STRINGS`) on SnakeYAML.

use crate::model::{self, M, MK, V};

pub enum Node {
    Map(Vec<(String, Node)>),
    List(Vec<Node>),
    Str(String),
    Bool(bool),
    /// Number as written.
    Num(String),
}

/// Converts the model into the tree that gets written.
pub fn to_node(m: &M) -> Node {
    let kind = model::kind(m);
    let mut entries: Vec<(String, Node)> = Vec::new();
    for (k, v) in model::public_props(m) {
        if k.starts_with("x-smallrye-profile-") || k == "x-smallrye-directives" {
            continue;
        }
        if kind == MK::Schema
            && k == "type"
            && let V::List(l) = &v
            && l.len() == 1
        {
            entries.push((k, value_node(&l[0])));
            continue;
        }
        entries.push((k, value_node(&v)));
    }
    if kind == MK::RequestBody && model::get_ref(m).is_none() {
        let required = if model::private_ext(m, "is-required-set") == Some(V::Bool(true)) {
            model::get_bool(m, "required")
        } else {
            match model::private_ext(m, "required-default") {
                Some(V::Bool(b)) => Some(b),
                _ => None,
            }
        };
        if let Some(r) = required {
            match entries.iter_mut().find(|(k, _)| k == "required") {
                Some(e) => e.1 = Node::Bool(r),
                None => entries.push(("required".into(), Node::Bool(r))),
            }
        }
    }
    Node::Map(entries)
}

fn value_node(v: &V) -> Node {
    match v {
        V::Str(s) => Node::Str(s.clone()),
        V::Bool(b) => Node::Bool(*b),
        V::Int(i) => Node::Num(i.to_string()),
        V::Dec(d) => Node::Num(d.clone()),
        V::List(l) => Node::List(l.iter().map(value_node).collect()),
        V::Map(m) => Node::Map(m.iter().map(|(k, v)| (k.clone(), value_node(v))).collect()),
        V::Obj(o) => to_node(o),
    }
}

// ---------------------------------------------------------------- JSON

pub fn to_json(n: &Node) -> String {
    let mut out = String::new();
    json(n, 0, &mut out);
    out
}

fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json(n: &Node, indent: usize, out: &mut String) {
    match n {
        Node::Str(s) => json_str(s, out),
        Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Node::Num(s) => out.push_str(s),
        Node::List(items) => {
            if items.is_empty() {
                out.push_str("[ ]");
                return;
            }
            out.push_str("[ ");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                json(item, indent, out);
            }
            out.push_str(" ]");
        }
        Node::Map(entries) => {
            if entries.is_empty() {
                out.push_str("{ }");
                return;
            }
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&" ".repeat(indent + 2));
                json_str(k, out);
                out.push_str(" : ");
                json(v, indent + 2, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------- YAML

const BEST_WIDTH: usize = 80;

struct Yaml {
    out: String,
    column: usize,
    whitespace: bool,
    indention: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Plain,
    Single,
    Double,
}

struct Analysis {
    empty: bool,
    multiline: bool,
    allow_block_plain: bool,
    allow_single: bool,
}

fn is_linebreak(c: char) -> bool {
    matches!(c, '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// SnakeYAML `Emitter.analyzeScalar` (block context only).
fn analyze(s: &str) -> Analysis {
    if s.is_empty() {
        return Analysis {
            empty: true,
            multiline: false,
            allow_block_plain: true,
            allow_single: true,
        };
    }
    let chars: Vec<char> = s.chars().collect();
    let mut block_indicators = false;
    let mut line_breaks = false;
    let mut special = false;
    let (mut leading_space, mut leading_break, mut trailing_space, mut trailing_break) =
        (false, false, false, false);
    let (mut break_space, mut space_break) = (false, false);
    if s.starts_with("---") || s.starts_with("...") {
        block_indicators = true;
    }
    let mut preceded_by_ws = true;
    let blank_t = |c: char| c == '\0' || c == ' ' || c == '\t';
    let mut followed_by_ws = chars.len() == 1 || blank_t(chars[1]) || is_linebreak(chars[1]);
    let mut previous_space = false;
    let mut previous_break = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if index == 0 {
            if "#,[]{}&*!|>'\"%@`".contains(c) {
                block_indicators = true;
            }
            if (c == '?' || c == ':') && followed_by_ws {
                block_indicators = true;
            }
            if c == '-' && followed_by_ws {
                block_indicators = true;
            }
        } else {
            if c == ':' && followed_by_ws {
                block_indicators = true;
            }
            if c == '#' && preceded_by_ws {
                block_indicators = true;
            }
        }
        let lb = is_linebreak(c);
        if lb {
            line_breaks = true;
        }
        if !(c == '\n' || (' '..='~').contains(&c)) {
            let cp = c as u32;
            let unicode = cp == 0x85
                || (0xA0..=0xD7FF).contains(&cp)
                || (0xE000..=0xFFFD).contains(&cp)
                || cp >= 0x10000;
            if !unicode {
                special = true;
            }
        }
        if c == ' ' {
            if index == 0 {
                leading_space = true;
            }
            if index == chars.len() - 1 {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if lb {
            if index == 0 {
                leading_break = true;
            }
            if index == chars.len() - 1 {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        index += 1;
        preceded_by_ws = blank_t(c) || lb;
        followed_by_ws = true;
        if index + 1 < chars.len() {
            let next = index + 1;
            if next < chars.len() {
                followed_by_ws = blank_t(chars[next]) || lb;
            }
        }
    }
    let mut allow_block_plain = true;
    let mut allow_single = true;
    if leading_space || leading_break || trailing_space || trailing_break {
        allow_block_plain = false;
    }
    if break_space {
        allow_block_plain = false;
        allow_single = false;
    }
    if space_break || special {
        allow_block_plain = false;
        allow_single = false;
    }
    if block_indicators {
        allow_block_plain = false;
    }
    Analysis {
        empty: false,
        multiline: line_breaks,
        allow_block_plain,
        allow_single,
    }
}

const RESERVED: &[&str] = &[
    "false", "False", "FALSE", "no", "No", "NO", "null", "Null", "NULL", "on", "On", "ON", "off",
    "Off", "OFF", "true", "True", "TRUE", "y", "Y", "yes", "Yes", "YES", "n", "N", "~",
];

fn is_reserved(s: &str) -> bool {
    s.is_empty() || RESERVED.contains(&s)
}

fn looks_like_number(s: &str) -> bool {
    matches!(s.chars().next(), Some('0'..='9' | '-' | '+' | '.'))
}

fn has_quotable_char(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        match c {
            '[' | ']' | '{' | '}' | ',' => return true,
            '#' if i == 0 || matches!(chars[i - 1], ' ' | '\t') => return true,
            ':' if i == chars.len() - 1 || matches!(chars[i + 1], ' ' | '\t') => return true,
            _ => {}
        }
    }
    false
}

fn plain_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let mut parts = s.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    int.chars().all(|c| c.is_ascii_digit())
        && frac.is_none_or(|f| f.chars().all(|c| c.is_ascii_digit()))
}

impl Yaml {
    fn write(&mut self, s: &str) {
        self.out.push_str(s);
        self.column += s.chars().count();
    }

    fn line_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
        self.whitespace = true;
        self.indention = true;
    }

    fn write_indent(&mut self, indent: usize) {
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.line_break();
        }
        if indent > self.column {
            let n = indent - self.column;
            self.out.push_str(&" ".repeat(n));
            self.column = indent;
        }
        self.whitespace = true;
        // indention stays as it was (writeWhitespace)
    }

    fn indicator(&mut self, ind: &str, need_ws: bool, whitespace: bool, indention: bool) {
        if !(self.whitespace || !need_ws) {
            self.write(" ");
        }
        self.whitespace = whitespace;
        self.indention = self.indention && indention;
        self.write(ind);
    }

    /// Chooses the style like Jackson (requested style) + SnakeYAML (`chooseScalarStyle`).
    fn style(&self, s: &str, requested_plain: bool, simple_key: bool) -> Style {
        let a = analyze(s);
        if !requested_plain {
            return Style::Double;
        }
        if !(simple_key && (a.empty || a.multiline)) && a.allow_block_plain {
            return Style::Plain;
        }
        if a.allow_single && !(simple_key && a.multiline) {
            return Style::Single;
        }
        Style::Double
    }

    fn scalar(&mut self, s: &str, style: Style, indent: usize, split: bool) {
        match style {
            Style::Plain => self.plain(s, indent, split),
            Style::Single => self.single(s, indent, split),
            Style::Double => self.double(s, indent, split),
        }
    }

    fn plain(&mut self, text: &str, indent: usize, split: bool) {
        if text.is_empty() {
            return;
        }
        if !self.whitespace {
            self.write(" ");
        }
        self.whitespace = false;
        self.indention = false;
        let chars: Vec<char> = text.chars().collect();
        let (mut spaces, mut start, mut end) = (false, 0usize, 0usize);
        while end <= chars.len() {
            let ch = chars.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end && self.column > BEST_WIDTH && split {
                        self.write_indent(indent);
                        self.whitespace = false;
                        self.indention = false;
                    } else {
                        let s: String = chars[start..end].iter().collect();
                        self.write(&s);
                    }
                    start = end;
                }
            } else if ch.is_none() || ch == Some(' ') {
                let s: String = chars[start..end].iter().collect();
                self.write(&s);
                start = end;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
            }
            end += 1;
        }
    }

    fn single(&mut self, text: &str, indent: usize, split: bool) {
        self.indicator("'", true, false, false);
        let chars: Vec<char> = text.chars().collect();
        let (mut spaces, mut start, mut end) = (false, 0usize, 0usize);
        while end <= chars.len() {
            let ch = chars.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end
                        && self.column > BEST_WIDTH
                        && split
                        && start != 0
                        && end != chars.len()
                    {
                        self.write_indent(indent);
                    } else {
                        let s: String = chars[start..end].iter().collect();
                        self.write(&s);
                    }
                    start = end;
                }
            } else if (ch.is_none() || ch == Some(' ') || ch == Some('\'')) && start < end {
                let s: String = chars[start..end].iter().collect();
                self.write(&s);
                start = end;
            }
            if ch == Some('\'') {
                self.write("''");
                start = end + 1;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
            }
            end += 1;
        }
        self.indicator("'", false, false, false);
    }

    fn double(&mut self, text: &str, indent: usize, split: bool) {
        self.indicator("\"", true, false, false);
        let chars: Vec<char> = text.chars().collect();
        let (mut start, mut end) = (0usize, 0usize);
        while end <= chars.len() {
            let ch = chars.get(end).copied();
            let needs_escape = match ch {
                None => true,
                Some(c) => {
                    "\"\\\u{85}\u{2028}\u{2029}\u{feff}".contains(c) || !(' '..='~').contains(&c)
                }
            };
            if needs_escape {
                if start < end {
                    let s: String = chars[start..end].iter().collect();
                    self.write(&s);
                    start = end;
                }
                if let Some(c) = ch {
                    let data = match c {
                        '\0' => "\\0".to_string(),
                        '\u{7}' => "\\a".to_string(),
                        '\u{8}' => "\\b".to_string(),
                        '\t' => "\\t".to_string(),
                        '\n' => "\\n".to_string(),
                        '\u{b}' => "\\v".to_string(),
                        '\u{c}' => "\\f".to_string(),
                        '\r' => "\\r".to_string(),
                        '\u{1b}' => "\\e".to_string(),
                        '"' => "\\\"".to_string(),
                        '\\' => "\\\\".to_string(),
                        '\u{85}' => "\\N".to_string(),
                        '\u{a0}' => "\\_".to_string(),
                        '\u{2028}' => "\\L".to_string(),
                        '\u{2029}' => "\\P".to_string(),
                        c if (c as u32) >= 0xA0 => c.to_string(),
                        c => format!("\\x{:02X}", c as u32),
                    };
                    self.write(&data);
                    start = end + 1;
                }
            }
            if 0 < end
                && end + 1 < chars.len()
                && (ch == Some(' ') || start >= end)
                && self.column + end.saturating_sub(start) > BEST_WIDTH
                && split
            {
                let mut data: String = if start >= end {
                    String::new()
                } else {
                    chars[start..end].iter().collect()
                };
                data.push('\\');
                if start < end {
                    start = end;
                }
                self.write(&data);
                self.write_indent(indent);
                self.whitespace = false;
                self.indention = false;
                if chars.get(start) == Some(&' ') {
                    self.write("\\");
                }
            }
            end += 1;
        }
        self.indicator("\"", false, false, false);
    }

    fn value_scalar(&mut self, n: &Node, indent: usize) {
        match n {
            Node::Str(s) => {
                let requested_plain = if s.contains('\n') {
                    false
                } else {
                    !(is_reserved(s) || has_quotable_char(s) || plain_number(s))
                };
                let style = self.style(s, requested_plain, false);
                self.scalar(s, style, indent, true);
            }
            Node::Bool(b) => self.plain(if *b { "true" } else { "false" }, indent, true),
            Node::Num(s) => self.plain(s, indent, true),
            _ => unreachable!(),
        }
    }

    fn key(&mut self, k: &str, indent: usize) {
        let requested_plain = !(is_reserved(k) || looks_like_number(k));
        let style = self.style(k, requested_plain, true);
        self.scalar(k, style, indent, false);
    }

    /// Block mapping at `indent`; `inline_first` when the first key follows a `- ` on the same line.
    fn mapping(&mut self, entries: &[(String, Node)], indent: usize, inline_first: bool) {
        for (i, (k, v)) in entries.iter().enumerate() {
            if !(i == 0 && inline_first) {
                self.write_indent(indent);
            }
            self.key(k, indent + 2);
            self.indicator(":", false, false, false);
            self.value(v, indent, true);
        }
    }

    /// Writes a value of a mapping entry (`in_mapping`) or a sequence item.
    fn value(&mut self, v: &Node, indent: usize, in_mapping: bool) {
        match v {
            Node::Map(e) if e.is_empty() => self.indicator("{}", true, false, false),
            Node::List(l) if l.is_empty() => self.indicator("[]", true, false, false),
            Node::Map(e) => {
                if in_mapping {
                    self.mapping(e, indent + 2, false);
                } else {
                    self.mapping(e, indent + 2, true);
                }
            }
            Node::List(l) => {
                // sequences in a mapping are indentless
                let seq_indent = if in_mapping { indent } else { indent + 2 };
                self.sequence(l, seq_indent, !in_mapping);
            }
            scalar => self.value_scalar(scalar, indent + 2),
        }
    }

    fn sequence(&mut self, items: &[Node], indent: usize, inline_first: bool) {
        for (i, item) in items.iter().enumerate() {
            if !(i == 0 && inline_first) {
                self.write_indent(indent);
            }
            self.indicator("-", true, false, true);
            self.value(item, indent, false);
        }
    }
}

pub fn to_yaml(n: &Node) -> String {
    let mut y = Yaml {
        out: String::new(),
        column: 0,
        whitespace: true,
        indention: true,
    };
    y.indicator("---", true, false, false);
    match n {
        Node::Map(e) if !e.is_empty() => {
            y.mapping(e, 0, false);
        }
        other => y.value(other, 0, false),
    }
    y.line_break();
    y.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(entries: Vec<(&str, Node)>) -> String {
        to_yaml(&Node::Map(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        ))
    }

    fn s(v: &str) -> Node {
        Node::Str(v.to_string())
    }

    #[test]
    fn quotes_scalars_like_jackson_minimize_quotes() {
        let out = yaml(vec![
            ("$ref", s("#/components/schemas/UUID")),
            ("pattern", s(".*\\S{2}.*")),
            ("null", s("null")),
            ("200", s("12")),
            ("*", s("As defined by 'partial-time' in RFC3339")),
        ]);
        assert_eq!(
            out,
            "---\n$ref: \"#/components/schemas/UUID\"\npattern: \".*\\\\S{2}.*\"\n\"null\": \"null\"\n\"200\": \"12\"\n\
             '*': As defined by 'partial-time' in RFC3339\n"
        );
    }

    #[test]
    fn folds_long_plain_scalars_at_80_columns() {
        // same nesting depth as in the backend's spec: /paths/<path>/<method>/responses/404/description
        let mut node = Node::Map(vec![(
            "description".into(),
            s(
                "A valid verification request with the given verificationToken is not found (any more)",
            ),
        )]);
        for key in ["404", "responses", "post", "/mail/verify"] {
            node = Node::Map(vec![(key.into(), node)]);
        }
        let out = yaml(vec![("paths", node)]);
        assert!(out.ends_with(
            "          description: A valid verification request with the given verificationToken\n            is not found (any more)\n"
        ));
    }

    #[test]
    fn writes_sequences_indentless_and_empty_collections_in_flow_style() {
        let security = Node::List(vec![Node::Map(vec![("auth0".into(), Node::List(vec![]))])]);
        let out = yaml(vec![
            ("required", Node::List(vec![s("count")])),
            ("security", security),
            ("value", Node::Map(vec![])),
        ]);
        assert_eq!(
            out,
            "---\nrequired:\n- count\nsecurity:\n- auth0: []\nvalue: {}\n"
        );
        assert_eq!(
            to_json(&Node::Map(vec![
                ("a".into(), Node::List(vec![])),
                ("b".into(), Node::List(vec![s("x"), s("y")]))
            ])),
            "{\n  \"a\" : [ ],\n  \"b\" : [ \"x\", \"y\" ]\n}"
        );
    }
}
