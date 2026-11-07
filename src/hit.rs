//! Parser for the subset of the HIT input syntax used by MOOSE test
//! specification (`tests`) files.
//!
//! Supported constructs:
//!
//! - sections: `[name]`, `[./name]`, `[../sibling]`, `[../../nested/path]`,
//!   and `[]` to close back to the root. Names without a leading dot are
//!   children of the current section (matching how the reference parser
//!   resolves the paths used in MOOSE spec files); a leading `/` makes the
//!   path absolute from the root.
//! - parameters: `key = value`
//! - values: bare tokens (`1e-10`, `file.i`), double-quoted strings, and
//!   `'''`-quoted block strings
//! - `#` comments running to end of line
//!
//! Constructs outside test-spec usage (substitutions, `!include`, indexed
//! parameter arrays) are intentionally not handled; they surface as parse
//! errors with line numbers rather than being silently misparsed.

use std::fmt;

/// A parameter value in a HIT file.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// An unquoted token such as a number, path, or bare word.
    Bare(String),
    /// A single- or double-quoted string. Adjacent string literals are
    /// concatenated by the lexer, matching the reference implementation.
    Quoted(String),
}

impl Value {
    /// The raw content of the value without quote delimiters.
    pub fn as_str(&self) -> &str {
        match self {
            Value::Bare(s) | Value::Quoted(s) => s,
        }
    }
}

/// One entry inside a [`Node`]: either a parameter or a nested section.
#[derive(Debug, Clone)]
pub enum Entry {
    /// A `key = value` parameter with the line it starts on.
    Param {
        /// Parameter name.
        key: String,
        /// Parameter value.
        value: Value,
        /// 1-based line number.
        line: usize,
    },
    /// A nested section.
    Section(Node),
}

/// A HIT section and its contents.
#[derive(Debug, Clone, Default)]
pub struct Node {
    /// Section name (empty for the root).
    pub name: String,
    /// 1-based line number where the section opens.
    pub line: usize,
    /// Parameters and sub-sections in file order.
    pub entries: Vec<Entry>,
}

impl Node {
    /// First value assigned to `key`, if present.
    pub fn param(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find_map(|e| match e {
            Entry::Param { key: k, value, .. } if k == key => Some(value),
            _ => None,
        })
    }

    /// All values assigned to `key` in order (duplicates are legal to detect).
    pub fn params<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.entries.iter().filter_map(move |e| match e {
            Entry::Param { key: k, value, .. } if k == key => Some(value),
            _ => None,
        })
    }

    /// Iterator over sub-sections.
    pub fn sections(&self) -> impl Iterator<Item = &Node> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Section(n) => Some(n),
            _ => None,
        })
    }
}

/// A syntax error with enough context to point at the problem.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    /// Description of what went wrong.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
            if c == Some(b'\n') {
                self.line += 1;
            }
        }
        c
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.bump();
                }
                Some(b'#') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            message: message.into(),
        }
    }

    /// Parse a bracketed path, positioned just after `[`. An empty path
    /// (`[]`) returns an empty segment list. The returned flag is true when
    /// the path was absolute (leading `/`).
    fn section_path(&mut self) -> Result<(Vec<String>, bool), ParseError> {
        let mut raw = String::new();
        loop {
            match self.bump() {
                Some(b']') => break,
                Some(b'\n') | None => return Err(self.error("unterminated '['")),
                Some(c) => raw.push(c as char),
            }
        }
        if raw.trim().is_empty() {
            return Ok((Vec::new(), false));
        }
        let absolute = raw.trim_start().starts_with('/');
        let mut parts: Vec<&str> = raw.split('/').collect();
        // Tolerate a trailing slash: `[../]` is common in the wild.
        if parts.last().is_some_and(|p| p.trim().is_empty()) {
            parts.pop();
        }
        if absolute && parts.first().is_some_and(|p| p.trim().is_empty()) {
            parts.remove(0);
        }
        let mut segments = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                return Err(self.error(format!("empty path segment in '[{raw}]'")));
            }
            segments.push(part.to_string());
        }
        Ok((segments, absolute))
    }

    /// Parse a parameter value, positioned at the first non-blank byte.
    ///
    /// Both single and double quotes open a string; a backslash before the
    /// quote character prevents termination. Adjacent string literals
    /// concatenate, so `'''x'''` (three quotes on each side) is the string
    /// `x` via two empty literals, exactly as the reference lexer produces.
    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"' | b'\'') => {
                let mut s = String::new();
                while let Some(quote @ (b'"' | b'\'')) = self.peek() {
                    self.bump();
                    let mut prev = None;
                    loop {
                        match self.bump() {
                            Some(c) if c == quote && prev != Some(b'\\') => break,
                            None => {
                                return Err(self.error("unterminated quoted string"));
                            }
                            Some(c) => {
                                prev = Some(c);
                                s.push(c as char);
                            }
                        }
                    }
                    // An adjacent string literal concatenates with this one;
                    // the reference lexer allows the continuation on the
                    // next line (a comment stops the run).
                    while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                        self.bump();
                    }
                }
                Ok(Value::Quoted(s))
            }
            _ => {
                let mut s = String::new();
                loop {
                    match self.peek() {
                        Some(b' ' | b'\t' | b'\r' | b'\n' | b'#') | None => break,
                        Some(c) => {
                            s.push(c as char);
                            self.bump();
                        }
                    }
                }
                if s.is_empty() {
                    return Err(self.error("expected a value after '='"));
                }
                Ok(Value::Bare(s))
            }
        }
    }
}

/// Pop open sections down to `depth`, attaching each completed section to
/// its parent's entries.
fn close_to(stack: &mut Vec<Node>, depth: usize) {
    while stack.len() > depth {
        let node = stack.pop().expect("stack longer than depth");
        stack
            .last_mut()
            .expect("parent below depth always exists")
            .entries
            .push(Entry::Section(node));
    }
}

/// Parse HIT text into a root [`Node`].
///
/// Returns the parsed tree together with any recoverable errors. Unrecoverable
/// syntax errors are returned as `Err`.
pub fn parse(src: &str) -> Result<Node, ParseError> {
    let mut p = Parser {
        src: src.as_bytes(),
        pos: 0,
        line: 1,
    };
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        p.skip_ws_and_comments();
        match p.peek() {
            None => {
                if stack.len() > 1 {
                    return Err(ParseError {
                        line: stack.last().map_or(1, |n| n.line),
                        message: format!(
                            "section '{}' never closed with '[]'",
                            stack.last().map_or("", |n| n.name.as_str())
                        ),
                    });
                }
                return Ok(stack.pop().expect("root node is always present"));
            }
            Some(b'[') => {
                p.bump();
                let (segments, absolute) = p.section_path()?;
                if segments.is_empty() {
                    close_to(&mut stack, 1);
                    continue;
                }
                // Resolve the path against the current stack. Paths whose
                // first segment is `.` or `..` are relative to the current
                // section; plain names are children of the current section
                // (as in `[1D]` inside `[Tests]`); a leading `/` is absolute
                // from the root.
                let mut depth = stack.len();
                if absolute {
                    depth = 1;
                }
                let mut name = None;
                for seg in &segments {
                    match seg.as_str() {
                        "." => {}
                        ".." => {
                            if depth > 1 {
                                depth -= 1;
                            } else {
                                return Err(p.error("'..' climbs above the root section"));
                            }
                        }
                        n => name = Some(n.to_string()),
                    }
                }
                close_to(&mut stack, depth);
                // An all-dot path (`[../]`) closes levels without opening a
                // new section.
                if let Some(name) = name {
                    stack.push(Node {
                        name,
                        line: p.line,
                        entries: Vec::new(),
                    });
                }
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' || c == b'-' => {
                let mut key = String::new();
                while let Some(c) = p.peek() {
                    if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.') {
                        key.push(c as char);
                        p.bump();
                    } else {
                        break;
                    }
                }
                p.skip_inline_ws();
                if p.peek() != Some(b'=') {
                    return Err(p.error(format!("expected '=' after parameter '{key}'")));
                }
                p.bump();
                p.skip_inline_ws();
                let value = p.value()?;
                let line = p.line;
                stack
                    .last_mut()
                    .expect("stack is never empty")
                    .entries
                    .push(Entry::Param { key, value, line });
            }
            Some(c) => {
                return Err(p.error(format!("unexpected character '{}'", c as char)));
            }
        }
    }
}

impl<'a> Parser<'a> {
    fn skip_inline_ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\r') = self.peek() {
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_relative_sections() {
        let src = "[Tests]\n  [./a]\n    type = RunApp\n    input = 'a.i'\n  [../b]\n    type = CSVDiff\n    csvdiff = 'b.csv'\n  []\n[]";
        let root = parse(src).expect("parses");
        let tests = &root.sections().next().expect("Tests");
        assert_eq!(tests.name, "Tests");
        let names: Vec<_> = tests.sections().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let a = &tests.sections().next().expect("a");
        assert_eq!(a.param("type").expect("type").as_str(), "RunApp");
        assert_eq!(a.param("input").expect("input").as_str(), "a.i");
    }

    #[test]
    fn parses_block_strings_and_comments() {
        let src = "[t]\n  requirement = '''line one\nline two'''\n  # trailing comment\n  issues = '#123'\n[]";
        let root = parse(src).expect("parses");
        let t = root.sections().next().expect("t");
        assert_eq!(
            t.param("requirement").expect("req").as_str(),
            "line one\nline two"
        );
        assert_eq!(t.param("issues").expect("issues").as_str(), "#123");
    }

    #[test]
    fn plain_names_nest_in_current_section() {
        // `[b]` is a child of the still-open `[a]`; `[]` closes everything,
        // and `[c]` lands at the root.
        let src = "[a]\n[b]\n[]\n[c]\n[]";
        let root = parse(src).expect("parses");
        let names: Vec<_> = root.sections().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        let nested: Vec<_> = root
            .sections()
            .next()
            .expect("a")
            .sections()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(nested, ["b"]);
    }

    #[test]
    fn reports_unclosed_section() {
        let err = parse("[a]\n  x = 1\n").expect_err("must fail");
        assert!(err.message.contains("never closed"), "{}", err.message);
        assert_eq!(err.line, 1);
    }

    #[test]
    fn reports_unterminated_quote() {
        let err = parse("[a]\nx = \"oops\n[]").expect_err("must fail");
        assert!(err.message.contains("unterminated"), "{}", err.message);
    }

    #[test]
    fn reports_parent_escape() {
        let err = parse("[../up]\nx = 1\n[]").expect_err("must fail");
        assert!(err.message.contains("above the root"), "{}", err.message);
    }

    #[test]
    fn tolerates_trailing_slash_in_paths() {
        let root = parse(
            "[Tests]\n  [./a]\n    type = RunApp\n  [../]\n  [./b]\n    type = RunApp\n  []\n[]",
        )
        .expect("parses");
        let tests = root.sections().next().expect("Tests");
        let names: Vec<_> = tests.sections().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn quoted_strings_continue_on_the_next_line() {
        let src =
            "[t]\n  requirement = 'part one '\n                'part two'\n  issues = '#1'\n[]";
        let root = parse(src).expect("parses");
        let t = root.sections().next().expect("t");
        assert_eq!(
            t.param("requirement").expect("req").as_str(),
            "part one part two"
        );
    }
}
