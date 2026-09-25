//! Classifies PowerShell commands for the Claude Code `PreToolUse` guard.
//!
//! The lexer splits a script into pipelines of commands and expressions, and
//! lexes script blocks, parentheses, and subexpressions as scripts of their
//! own. A pipeline is denied when text embedded in it (a string, here-string,
//! or `Write-Output` arguments) or `Get-Content` text rewritten by `-replace`,
//! `.Replace()`, or `ForEach-Object` reaches a writer such as `Set-Content`,
//! `Out-File`, or `>`; when it calls a .NET file-write API; or when an
//! external program fails the Bash checks. Anything else is allowed.

use std::collections::HashMap;
use std::ops::Range;

use super::{
    Command, Content, Context, Dialect, Finding, MAX_NESTING, MAX_SCRIPT_DEPTH, MAX_TOKENS,
    Pattern, Scope, Word, inspect, program_name,
};

/// Reserved words after which the next token decides whether a statement is
/// a command or an expression.
const KEYWORDS: &[&str] = &[
    "catch", "do", "else", "elseif", "exit", "finally", "if", "return", "switch", "throw", "trap",
    "try", "until", "while",
];

/// Commands that neither read, write, nor pass on content.
const INERT: &[&str] = &[
    "begin",
    "break",
    "class",
    "continue",
    "data",
    "dynamicparam",
    "end",
    "enum",
    "filter",
    "for",
    "function",
    "out-null",
    "param",
    "process",
    "using",
];

/// Commands that pass their input on unchanged or only reordered.
const PASS_THROUGH: &[&str] = &[
    "?",
    "convertto-csv",
    "convertto-html",
    "convertto-json",
    "convertto-xml",
    "get-unique",
    "gu",
    "join-string",
    "out-string",
    "select",
    "select-object",
    "sort",
    "sort-object",
    "where",
    "where-object",
];

/// `[IO.File]` methods that write files.
const FILE_WRITES: &[&str] = &[
    "appendalllines",
    "appendalllinesasync",
    "appendalltext",
    "appendalltextasync",
    "appendtext",
    "create",
    "createtext",
    "openwrite",
    "writeallbytes",
    "writeallbytesasync",
    "writealllines",
    "writealllinesasync",
    "writealltext",
    "writealltextasync",
];

/// A script's pipelines in source order. Once `;`, newlines, `&&`, and `||`
/// have split them, statement boundaries no longer matter.
type Script = Vec<Pipeline>;

/// The elements of one pipeline, joined by `|`.
type Pipeline = Vec<Element>;

#[derive(Debug, Default)]
struct Element {
    atoms: Vec<Atom>,
    redirects: Vec<Redirect>,
    mode: Mode,
}

impl Element {
    fn is_empty(&self) -> bool {
        self.atoms.is_empty() && self.redirects.is_empty()
    }
}

/// Whether an element parses as a command with arguments or as an
/// expression, which decides what `[` starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Undecided,
    Command,
    Expression,
}

#[derive(Debug)]
struct Redirect {
    /// The operator as written, such as `>` or `*>>`.
    operator: String,
    /// Whether the success stream goes to the target.
    output: bool,
    /// The target's atoms; empty for a merge such as `2>&1`.
    target: Vec<Atom>,
}

#[derive(Debug)]
struct Atom {
    kind: Kind,
    /// The atom's bytes in the script.
    span: Range<usize>,
    /// Whether whitespace separates it from the previous atom.
    spaced: bool,
}

#[derive(Debug)]
enum Kind {
    /// A bareword, number, parameter, or operator after escape and quote
    /// processing. `-Name:` keeps its colon, and its value is the next atom.
    Bare(String),
    /// A quoted string's value, with expansions verbatim.
    Quoted(String),
    /// A here-string's value.
    Here(String),
    /// A variable's name as written after `$`, such as `env:TEMP`.
    Variable(String),
    /// `@name` splatting.
    Splat,
    /// A bracketed script and the pipelines inside it.
    Group(Bracket, Script),
    /// A type literal or index such as `[IO.File]`, without brackets.
    Type(String),
    /// `.Name` or `::Name` member access, without the dot or colons.
    Member(String),
    /// `,`, `&`, or `=` for any assignment operator.
    Punct(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bracket {
    /// `(...)`, `$(...)`, or `@(...)`.
    Paren,
    /// `{...}`.
    Block,
    /// `@{...}`.
    Hash,
}

struct Lexer<'a> {
    source: &'a [u8],
    at: usize,
    /// Subexpressions inside strings, checked as scripts of their own.
    nested: Vec<Script>,
    nesting: usize,
    /// Atoms and redirections lexed so far, bounded by `MAX_TOKENS`.
    tokens: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            at: 0,
            nested: Vec::new(),
            nesting: 0,
            tokens: 0,
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.source.get(self.at + offset).copied()
    }

    /// Lexes pipelines up to the end of input or past the `close` bracket.
    fn script(&mut self, close: Option<u8>) -> Script {
        let mut script = vec![vec![Element::default()]];
        loop {
            if self.tokens > MAX_TOKENS {
                // Every enclosing script stops as well.
                self.at = self.source.len();
                break;
            }
            let spaced = self.blank();
            let Some(byte) = self.peek(0) else {
                break;
            };
            match byte {
                b'\n' => {
                    self.at += 1;
                    if !self.continues(&script) {
                        end(&mut script);
                    }
                }
                b'#' => {
                    while self.peek(0).is_some_and(|byte| byte != b'\n') {
                        self.at += 1;
                    }
                }
                b'<' if self.peek(1) == Some(b'#') => {
                    self.at = self.source[self.at..]
                        .windows(2)
                        .skip(2)
                        .position(|pair| pair == b"#>")
                        .map_or(self.source.len(), |offset| self.at + offset + 4);
                }
                b';' => {
                    self.at += 1;
                    end(&mut script);
                }
                // PowerShell 7 chains pipelines with `&&` and `||`.
                b'|' | b'&' if self.peek(1) == Some(byte) => {
                    self.at += 2;
                    end(&mut script);
                }
                b'|' => {
                    self.at += 1;
                    if let Some(pipeline) = script.last_mut() {
                        pipeline.push(Element::default());
                    }
                }
                b')' | b'}' => {
                    self.at += 1;
                    if close == Some(byte) {
                        break;
                    }
                }
                _ => {
                    if let Some(element) =
                        script.last_mut().and_then(|pipeline| pipeline.last_mut())
                    {
                        self.atom(element, spaced);
                    }
                }
            }
        }
        for pipeline in &mut script {
            pipeline.retain(|element| !element.is_empty());
        }
        script.retain(|pipeline| !pipeline.is_empty());
        script
    }

    /// Whether a newline continues the statement: after `|`, a comma, or an
    /// assignment, or before a line that starts with `|`, which it skips to.
    fn continues(&mut self, script: &Script) -> bool {
        let Some(element) = script.last().and_then(|pipeline| pipeline.last()) else {
            return false;
        };
        if element.is_empty()
            || matches!(
                element.atoms.last().map(|atom| &atom.kind),
                Some(Kind::Punct(b',' | b'='))
            )
        {
            return true;
        }
        let rest = &self.source[self.at..];
        let next = rest
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
        match next {
            Some(offset) if rest[offset] == b'|' && rest.get(offset + 1) != Some(&b'|') => {
                self.at += offset;
                true
            }
            _ => false,
        }
    }

    /// Skips spaces, tabs, and backtick line continuations; returns whether
    /// it skipped any.
    fn blank(&mut self) -> bool {
        let start = self.at;
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b' ' | b'\t' | b'\r' | 0x0c), _) => self.at += 1,
                (Some(b'`'), Some(b'\n')) => self.at += 2,
                _ => break,
            }
        }
        self.at > start
    }

    /// Lexes one atom or redirection into `element`.
    fn atom(&mut self, element: &mut Element, spaced: bool) {
        self.tokens += 1;
        let redirect = match (self.peek(0), self.peek(1)) {
            (Some(b'>'), _) => true,
            (Some(b'*' | b'0'..=b'9'), Some(b'>')) => spaced || element.atoms.is_empty(),
            _ => false,
        };
        if redirect {
            let redirect = self.redirect();
            element.redirects.push(redirect);
            return;
        }
        let start = self.at;
        let kind = self.operand(element, spaced);
        let spaced = spaced || element.atoms.is_empty();
        element.atoms.push(Atom {
            kind,
            span: start..self.at,
            spaced,
        });
        decide(element);
    }

    /// Lexes a redirection such as `>`, `2>>`, `*>`, or `2>&1` and its target.
    fn redirect(&mut self) -> Redirect {
        let start = self.at;
        let stream = self.peek(0).filter(|byte| *byte != b'>');
        self.at += usize::from(stream.is_some()) + 1;
        if self.peek(0) == Some(b'>') {
            self.at += 1;
        }
        let operator = lossy(&self.source[start..self.at]);
        if self.peek(0) == Some(b'&') && self.peek(1).is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 2;
            return Redirect {
                operator,
                output: false,
                target: Vec::new(),
            };
        }
        // The target is the next argument: one atom and those joined to it.
        self.blank();
        let mut target = Element {
            mode: Mode::Command,
            ..Element::default()
        };
        while let Some(byte) = self.peek(0) {
            let comment = byte == b'#' && target.atoms.is_empty()
                || byte == b'<' && self.peek(1) == Some(b'#');
            if comment
                || matches!(
                    byte,
                    b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b')' | b'}' | b'>' | b'&' | b','
                )
            {
                break;
            }
            let start = self.at;
            let first = target.atoms.is_empty();
            self.tokens += 1;
            let kind = self.operand(&target, first);
            target.atoms.push(Atom {
                kind,
                span: start..self.at,
                spaced: first,
            });
        }
        Redirect {
            operator,
            output: matches!(stream, None | Some(b'1' | b'*')),
            target: target.atoms,
        }
    }

    /// Lexes one atom other than a redirection. It always consumes a byte.
    fn operand(&mut self, element: &Element, spaced: bool) -> Kind {
        // Joined to a value, `.`, `::`, and `[` reach its members and items.
        let follows_value = !spaced
            && element
                .atoms
                .last()
                .is_some_and(|atom| !matches!(atom.kind, Kind::Bare(_) | Kind::Punct(_)));
        if let Some(text) = self.here_string() {
            return Kind::Here(text);
        }
        if quote(self.source, self.at).is_some() {
            let mut text = Vec::new();
            self.quoted(&mut text);
            return Kind::Quoted(lossy(&text));
        }
        let byte = self.peek(0).unwrap_or_default();
        match (byte, self.peek(1)) {
            (b'$' | b'@', Some(b'(')) => {
                self.at += 1;
                Kind::Group(Bracket::Paren, self.bracketed())
            }
            (b'@', Some(b'{')) => {
                self.at += 1;
                Kind::Group(Bracket::Hash, self.bracketed())
            }
            (b'(', _) => Kind::Group(Bracket::Paren, self.bracketed()),
            (b'{', _) => Kind::Group(Bracket::Block, self.bracketed()),
            (b'$', _) => self.variable(),
            (b'@', Some(next)) if is_name(next) => {
                self.at += 1;
                self.name();
                Kind::Splat
            }
            (b'[', _) if follows_value || element.mode != Mode::Command => self.type_literal(),
            (b'.', Some(next)) if follows_value && is_name(next) => {
                self.at += 1;
                Kind::Member(self.name())
            }
            (b':', Some(b':')) if follows_value => {
                self.at += 2;
                Kind::Member(self.name())
            }
            (b',' | b'&' | b'=', _) => {
                self.at += 1;
                Kind::Punct(byte)
            }
            (b'+' | b'-' | b'*' | b'/' | b'%', Some(b'=')) => {
                self.at += 2;
                Kind::Punct(b'=')
            }
            _ if dash(self.source, self.at).is_some() => self.parameter(),
            _ => Kind::Bare(self.bare()),
        }
    }

    /// Lexes a token that starts with a dash. Like PowerShell, it splits
    /// `-Name:value` after the colon, and it reads typographic dashes as `-`.
    fn parameter(&mut self) -> Kind {
        let start = self.at;
        let length = dash(self.source, start).unwrap_or(1);
        let name = self.source[start + length..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        if name > 0
            && self.source[start + length].is_ascii_alphabetic()
            && self.source.get(start + length + name) == Some(&b':')
        {
            self.at = start + length + name + 1;
            return Kind::Bare(format!(
                "-{}:",
                lossy(&self.source[start + length..start + length + name])
            ));
        }
        let text = self.bare();
        Kind::Bare(match text.get(length..) {
            Some(rest) if length > 1 => format!("-{rest}"),
            _ => text,
        })
    }

    /// Lexes a generic token: escapes and quoted parts are resolved, and
    /// variables and subexpressions stay verbatim.
    fn bare(&mut self) -> String {
        let start = self.at;
        let mut text = Vec::new();
        while let Some(byte) = self.peek(0) {
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b'&' | b'(' | b')' | b'{' | b'}'
                | b',' | b'>' | b'<' => break,
                b'`' => match self.peek(1) {
                    Some(b'\n') => break,
                    Some(next) => {
                        text.push(escape(next));
                        self.at += 2;
                    }
                    None => {
                        text.push(byte);
                        self.at += 1;
                    }
                },
                b'$' if self.peek(1) == Some(b'(') => self.subexpression(&mut text),
                b'$' if self.peek(1) == Some(b'{') => {
                    let end = self.source[self.at..]
                        .iter()
                        .position(|byte| *byte == b'}')
                        .map_or(self.source.len(), |offset| self.at + offset + 1);
                    text.extend_from_slice(&self.source[self.at..end]);
                    self.at = end;
                }
                _ if quote(self.source, self.at).is_some() => self.quoted(&mut text),
                _ => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
        if self.at == start
            && let Some(byte) = self.peek(0)
        {
            // Never stall on a byte the caller did not expect.
            text.push(byte);
            self.at += 1;
        }
        lossy(&text)
    }

    /// Lexes the quoted string at the cursor into `text`. Only the quote
    /// kind that opened it closes it, and a doubled quote is a literal one.
    fn quoted(&mut self, text: &mut Vec<u8>) {
        let Some((kind, length)) = quote(self.source, self.at) else {
            return;
        };
        self.at += length;
        loop {
            if let Some((closing, length)) = quote(self.source, self.at)
                && closing == kind
            {
                self.at += length;
                match quote(self.source, self.at) {
                    Some((next, length)) if next == kind => {
                        text.push(kind);
                        self.at += length;
                    }
                    _ => return,
                }
                continue;
            }
            let Some(byte) = self.peek(0) else {
                return;
            };
            match byte {
                b'`' if kind == b'"' => {
                    if let Some(next) = self.peek(1) {
                        text.push(escape(next));
                    }
                    self.at += 2;
                }
                b'$' if kind == b'"' && self.peek(1) == Some(b'(') => self.subexpression(text),
                _ => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
    }

    /// Lexes a here-string at the cursor, if one starts here: `@'` or `@"`
    /// ending its line, through a line that starts with `'@` or `"@`.
    fn here_string(&mut self) -> Option<String> {
        let (Some(b'@'), Some(kind @ (b'\'' | b'"'))) = (self.peek(0), self.peek(1)) else {
            return None;
        };
        let header = self.source[self.at + 2..]
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
            .map_or(self.source.len(), |offset| self.at + 2 + offset);
        if self.source.get(header) != Some(&b'\n') {
            return None;
        }
        let body = header + 1;
        let mut line = body;
        let end = loop {
            if self.source[line..].starts_with(&[kind, b'@']) {
                break Some(line);
            }
            match self.source[line..].iter().position(|byte| *byte == b'\n') {
                Some(offset) => line += offset + 1,
                None => break None,
            }
        };
        // The newline before the terminator is not part of the value.
        let (content, resume) = match end {
            Some(line) => (body..line.saturating_sub(1).max(body), line + 2),
            None => (body..self.source.len(), self.source.len()),
        };
        if kind == b'\'' {
            self.at = resume;
            return Some(lossy(&self.source[content]));
        }
        // An expandable here-string resolves escapes and holds subexpressions.
        let mut text = Vec::new();
        self.at = content.start;
        while self.at < content.end {
            match self.source[self.at] {
                b'`' => {
                    if let Some(next) = self.peek(1) {
                        text.push(escape(next));
                    }
                    self.at += 2;
                }
                b'$' if self.peek(1) == Some(b'(') => self.subexpression(&mut text),
                byte => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
        self.at = self.at.max(resume);
        Some(lossy(&text))
    }

    /// Lexes a `$(...)` inside a string or bareword into `nested`, keeping
    /// its text verbatim.
    fn subexpression(&mut self, text: &mut Vec<u8>) {
        let start = self.at;
        self.at += 1;
        let script = self.bracketed();
        self.nested.push(script);
        text.extend_from_slice(&self.source[start..self.at]);
    }

    /// Lexes the script inside the `(` or `{` at the cursor.
    fn bracketed(&mut self) -> Script {
        let Some(open) = self.peek(0) else {
            return Vec::new();
        };
        let close = if open == b'{' { b'}' } else { b')' };
        self.at += 1;
        if self.nesting >= MAX_NESTING {
            self.skip_balanced(open, close);
            return Vec::new();
        }
        self.nesting += 1;
        let script = self.script(Some(close));
        self.nesting -= 1;
        script
    }

    /// Skips past the bracket that closes one already consumed, without
    /// lexing what is inside.
    fn skip_balanced(&mut self, open: u8, close: u8) {
        let mut depth = 1_usize;
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'`' => self.at += 1,
                b'\'' | b'"' => {
                    while let Some(inner) = self.peek(0) {
                        self.at += 1;
                        if inner == byte {
                            break;
                        }
                        if inner == b'`' && byte == b'"' {
                            self.at += 1;
                        }
                    }
                }
                _ if byte == open => depth += 1,
                _ if byte == close => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        self.at = self.at.min(self.source.len());
    }

    /// Lexes `$name`, `$scope:name`, `${name}`, `$$`, `$?`, or `$^`.
    fn variable(&mut self) -> Kind {
        let start = self.at + 1;
        self.at = start;
        if self.peek(0) == Some(b'{') {
            let end = self.source[start..]
                .iter()
                .position(|byte| *byte == b'}')
                .map_or(self.source.len(), |offset| start + offset);
            self.at = (end + 1).min(self.source.len());
            return Kind::Variable(lossy(&self.source[start + 1..end]));
        }
        if matches!(self.peek(0), Some(b'$' | b'?' | b'^')) {
            self.at += 1;
        } else {
            self.name();
            // A drive or scope qualifier such as `env:`.
            if self.at > start && self.peek(0) == Some(b':') && self.peek(1).is_some_and(is_name) {
                self.at += 1;
                self.name();
            }
        }
        if self.at == start {
            return Kind::Bare("$".to_owned());
        }
        Kind::Variable(lossy(&self.source[start..self.at]))
    }

    fn name(&mut self) -> String {
        let start = self.at;
        while self.peek(0).is_some_and(is_name) {
            self.at += 1;
        }
        lossy(&self.source[start..self.at])
    }

    /// Lexes a `[...]` type literal or index, stopping at the end of a line.
    fn type_literal(&mut self) -> Kind {
        let start = self.at + 1;
        let mut depth = 0_usize;
        let mut end = None;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'\n' => break,
                b'[' => depth += 1,
                b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(self.at);
                        self.at += 1;
                        break;
                    }
                }
                _ => {}
            }
            self.at += 1;
        }
        let end = end.unwrap_or(self.at).max(start);
        Kind::Type(lossy(&self.source[start..end]))
    }
}

/// Ends the current pipeline unless it is still empty.
fn end(script: &mut Script) {
    if script
        .last()
        .is_some_and(|pipeline| pipeline.iter().any(|element| !element.is_empty()))
    {
        script.push(vec![Element::default()]);
    }
}

/// Updates whether an element is a command or an expression after its
/// latest atom.
fn decide(element: &mut Element) {
    let Some(atom) = element.atoms.last() else {
        return;
    };
    element.mode = match (&atom.kind, element.mode) {
        // The value after an assignment starts a command or an expression.
        (Kind::Punct(b'='), _) => Mode::Undecided,
        (Kind::Bare(text), Mode::Undecided)
            if KEYWORDS.contains(&text.to_ascii_lowercase().as_str()) =>
        {
            Mode::Undecided
        }
        (Kind::Bare(text), Mode::Undecided) if names_command(text) => Mode::Command,
        (Kind::Punct(b'&'), Mode::Undecided) => Mode::Command,
        (_, Mode::Undecided) => Mode::Expression,
        (_, mode) => mode,
    };
}

/// Whether a bareword that starts an element names a command rather than a
/// number or an operator.
fn names_command(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|first| !(first.is_ascii_digit() || matches!(first, '-' | '+' | '!')))
}

/// The kind and byte length of a quote at `at`. PowerShell also accepts
/// typographic single (`‘ ’ ‚ ‛`) and double (`“ ” „`) quotes.
pub(super) fn quote(source: &[u8], at: usize) -> Option<(u8, usize)> {
    match source.get(at..)? {
        [b'\'', ..] => Some((b'\'', 1)),
        [b'"', ..] => Some((b'"', 1)),
        [0xE2, 0x80, 0x98..=0x9B, ..] => Some((b'\'', 3)),
        [0xE2, 0x80, 0x9C..=0x9E, ..] => Some((b'"', 3)),
        _ => None,
    }
}

/// The byte a backtick escape stands for.
pub(super) fn escape(byte: u8) -> u8 {
    match byte {
        b'0' => 0,
        b'a' => 0x07,
        b'b' => 0x08,
        b'e' => 0x1b,
        b'f' => 0x0c,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'v' => 0x0b,
        other => other,
    }
}

/// The byte length of a dash at `at`: `-` or a typographic dash (`– — ―`),
/// which PowerShell accepts before parameters and operators.
fn dash(source: &[u8], at: usize) -> Option<usize> {
    match source.get(at..)? {
        [b'-', ..] => Some(1),
        [0xE2, 0x80, 0x93..=0x95, ..] => Some(3),
        _ => None,
    }
}

fn is_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Returns the first write into the project in a PowerShell script that runs
/// `depth` scripts deep.
pub(super) fn scan(script: &str, scope: &Scope, depth: usize) -> Option<Finding> {
    let mut lexer = Lexer::new(script);
    let body = lexer.script(None);
    let mut checker = Checker {
        context: Context {
            source: script,
            dialect: Dialect::PowerShell,
            scope,
            depth,
        },
        variables: HashMap::new(),
        flows: HashMap::new(),
    };
    checker.script(&body).or_else(|| {
        lexer
            .nested
            .iter()
            .find_map(|nested| checker.script(nested))
    })
}

/// What a pipeline, argument, or variable carries.
#[derive(Clone, Debug)]
enum Flow {
    /// Text embedded in the command.
    Embedded(Content),
    /// File content from `Get-Content` or `[IO.File]::ReadAllText`.
    Read,
    /// Read content rewritten by `-replace`, `.Replace()`, or `ForEach-Object`.
    Transformed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writer {
    SetContent,
    AddContent,
    OutFile,
    TeeObject,
    NewItem,
}

/// What a writer's parameter binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Path,
    Value,
    /// `New-Item -Name`, which is joined to the path.
    Name,
    ItemType,
    /// `Tee-Object -Variable`, which writes no file.
    Variable,
    Other,
    Switch,
}

const CONTENT_PARAMETERS: &[(&str, Role)] = &[
    ("path", Role::Path),
    ("literalpath", Role::Path),
    ("pspath", Role::Path),
    ("lp", Role::Path),
    ("value", Role::Value),
    ("credential", Role::Other),
    ("encoding", Role::Other),
    ("exclude", Role::Other),
    ("filter", Role::Other),
    ("include", Role::Other),
    ("stream", Role::Other),
    ("asbytestream", Role::Switch),
    ("confirm", Role::Switch),
    ("force", Role::Switch),
    ("nonewline", Role::Switch),
    ("passthru", Role::Switch),
    ("usetransaction", Role::Switch),
    ("whatif", Role::Switch),
];

const OUT_FILE_PARAMETERS: &[(&str, Role)] = &[
    ("filepath", Role::Path),
    ("path", Role::Path),
    ("literalpath", Role::Path),
    ("pspath", Role::Path),
    ("lp", Role::Path),
    ("inputobject", Role::Value),
    ("encoding", Role::Other),
    ("width", Role::Other),
    ("append", Role::Switch),
    ("confirm", Role::Switch),
    ("force", Role::Switch),
    ("noclobber", Role::Switch),
    ("nonewline", Role::Switch),
    ("whatif", Role::Switch),
];

const TEE_PARAMETERS: &[(&str, Role)] = &[
    ("filepath", Role::Path),
    ("path", Role::Path),
    ("literalpath", Role::Path),
    ("pspath", Role::Path),
    ("lp", Role::Path),
    ("inputobject", Role::Value),
    ("variable", Role::Variable),
    ("encoding", Role::Other),
    ("append", Role::Switch),
];

const NEW_ITEM_PARAMETERS: &[(&str, Role)] = &[
    ("path", Role::Path),
    ("name", Role::Name),
    ("value", Role::Value),
    ("itemtype", Role::ItemType),
    ("type", Role::ItemType),
    ("credential", Role::Other),
    ("confirm", Role::Switch),
    ("force", Role::Switch),
    ("usetransaction", Role::Switch),
    ("whatif", Role::Switch),
];

/// Parameters that every cmdlet accepts.
const COMMON_PARAMETERS: &[(&str, Role)] = &[
    ("debug", Role::Switch),
    ("db", Role::Switch),
    ("verbose", Role::Switch),
    ("vb", Role::Switch),
    ("erroraction", Role::Other),
    ("ea", Role::Other),
    ("errorvariable", Role::Other),
    ("ev", Role::Other),
    ("informationaction", Role::Other),
    ("infa", Role::Other),
    ("informationvariable", Role::Other),
    ("iv", Role::Other),
    ("outbuffer", Role::Other),
    ("ob", Role::Other),
    ("outvariable", Role::Other),
    ("ov", Role::Other),
    ("pipelinevariable", Role::Other),
    ("pv", Role::Other),
    ("progressaction", Role::Other),
    ("proga", Role::Other),
    ("warningaction", Role::Other),
    ("wa", Role::Other),
    ("warningvariable", Role::Other),
    ("wv", Role::Other),
];

impl Writer {
    fn name(self) -> &'static str {
        match self {
            Self::SetContent => "Set-Content",
            Self::AddContent => "Add-Content",
            Self::OutFile => "Out-File",
            Self::TeeObject => "Tee-Object",
            Self::NewItem => "New-Item",
        }
    }

    fn parameters(self) -> &'static [(&'static str, Role)] {
        match self {
            Self::SetContent | Self::AddContent => CONTENT_PARAMETERS,
            Self::OutFile => OUT_FILE_PARAMETERS,
            Self::TeeObject => TEE_PARAMETERS,
            Self::NewItem => NEW_ITEM_PARAMETERS,
        }
    }

    /// What positional arguments bind, in order.
    fn positions(self) -> &'static [Role] {
        match self {
            Self::SetContent | Self::AddContent => &[Role::Path, Role::Value],
            Self::OutFile => &[Role::Path, Role::Other],
            Self::TeeObject | Self::NewItem => &[Role::Path],
        }
    }

    /// What a parameter name binds, accepting any unambiguous prefix as
    /// PowerShell does; `None` for an unknown or ambiguous name.
    fn role(self, name: &str) -> Option<Role> {
        let name = name.to_ascii_lowercase();
        let parameters = self.parameters().iter().chain(COMMON_PARAMETERS);
        if let Some((_, role)) = parameters.clone().find(|(known, _)| *known == name) {
            return Some(*role);
        }
        let mut roles = parameters
            .filter(|(known, _)| known.starts_with(&name))
            .map(|(_, role)| *role);
        let role = roles.next()?;
        roles.all(|other| other == role).then_some(role)
    }
}

/// How a command affects what flows through a pipeline.
enum Cmdlet {
    Read,
    Output,
    PassThrough,
    ForEach,
    Writer(Writer),
    Evaluate,
    NewObject,
    Inert,
    External,
}

fn cmdlet(name: &str, first: bool) -> Cmdlet {
    let name = name.to_ascii_lowercase();
    // A module-qualified name such as `Microsoft.PowerShell.Management\Set-Content`.
    let name = match name.rsplit_once('\\') {
        Some((module, cmdlet)) if !module.contains(':') && cmdlet.contains('-') => cmdlet,
        _ => name.as_str(),
    };
    match name {
        "get-content" | "gc" | "cat" | "type" => Cmdlet::Read,
        "write-output" | "echo" | "write" => Cmdlet::Output,
        "set-content" | "sc" => Cmdlet::Writer(Writer::SetContent),
        "add-content" | "ac" => Cmdlet::Writer(Writer::AddContent),
        "out-file" => Cmdlet::Writer(Writer::OutFile),
        "tee-object" | "tee" => Cmdlet::Writer(Writer::TeeObject),
        "new-item" | "ni" => Cmdlet::Writer(Writer::NewItem),
        // `foreach` starting a statement is the loop keyword.
        "foreach" if first => Cmdlet::Inert,
        "foreach-object" | "foreach" | "%" => Cmdlet::ForEach,
        "invoke-expression" | "iex" => Cmdlet::Evaluate,
        "new-object" => Cmdlet::NewObject,
        _ if PASS_THROUGH.contains(&name) => Cmdlet::PassThrough,
        _ if KEYWORDS.contains(&name) || INERT.contains(&name) => Cmdlet::Inert,
        _ => Cmdlet::External,
    }
}

/// A writer's arguments, bound as PowerShell binds them.
#[derive(Default)]
struct Bound<'a> {
    paths: Vec<&'a [Atom]>,
    value: Option<&'a [Atom]>,
    item_type: Option<&'a [Atom]>,
    variable: bool,
    /// Whether an unknown parameter, `-Name`, or a surplus argument leaves
    /// the written path uncertain.
    uncertain: bool,
}

fn bind<'a>(writer: Writer, arguments: &[&'a [Atom]]) -> Bound<'a> {
    let mut bound = Bound::default();
    let mut positions = writer.positions().iter();
    let mut index = 0;
    while let Some(&argument) = arguments.get(index) {
        index += 1;
        let (role, value) = match parameter(argument) {
            None => match positions.next() {
                Some(&role) => (role, argument),
                None => {
                    bound.uncertain = true;
                    continue;
                }
            },
            Some((name, attached)) => match (writer.role(name), attached) {
                (None, _) => {
                    bound.uncertain = true;
                    continue;
                }
                (Some(Role::Switch), _) => continue,
                (Some(role), Some(value)) => (role, value),
                (Some(role), None) => {
                    let Some(&value) = arguments.get(index) else {
                        continue;
                    };
                    index += 1;
                    (role, value)
                }
            },
        };
        match role {
            Role::Path => bound.paths.extend(
                value
                    .split(|atom| matches!(atom.kind, Kind::Punct(b',')))
                    .filter(|path| !path.is_empty()),
            ),
            Role::Value => bound.value = Some(value),
            Role::ItemType => bound.item_type = Some(value),
            Role::Name => bound.uncertain = true,
            Role::Variable => bound.variable = true,
            Role::Other | Role::Switch => {}
        }
    }
    bound
}

/// The name of a `-Name` or `-Name:value` argument and its attached value.
fn parameter(argument: &[Atom]) -> Option<(&str, Option<&[Atom]>)> {
    let Kind::Bare(text) = &argument.first()?.kind else {
        return None;
    };
    let name = text
        .strip_prefix('-')
        .filter(|name| name.starts_with(|character: char| character.is_ascii_alphabetic()))?;
    Some(match name.strip_suffix(':') {
        Some(name) => (name, Some(&argument[1..]).filter(|value| !value.is_empty())),
        None => (name, None),
    })
}

/// Splits arguments into runs of adjacent atoms; a comma joins the atoms
/// around it into one array argument.
fn runs(atoms: &[Atom]) -> Vec<&[Atom]> {
    let mut runs = Vec::new();
    let mut start = 0;
    for index in 1..atoms.len() {
        let joined = !atoms[index].spaced
            || matches!(atoms[index].kind, Kind::Punct(b','))
            || matches!(atoms[index - 1].kind, Kind::Punct(b','));
        if !joined {
            runs.push(&atoms[start..index]);
            start = index;
        }
    }
    if start < atoms.len() {
        runs.push(&atoms[start..]);
    }
    runs
}

/// Where the command an element runs is named: first, or after the `&`
/// call or `.` dot-source operator. `None` means the element is an expression.
fn command_start(atoms: &[Atom]) -> Option<usize> {
    let literal = |atom: &Atom| matches!(atom.kind, Kind::Bare(_) | Kind::Quoted(_));
    match atoms {
        [
            Atom {
                kind: Kind::Punct(b'&'),
                ..
            },
            name,
            ..,
        ] if literal(name) => Some(1),
        [
            Atom {
                kind: Kind::Bare(dot),
                ..
            },
            name,
            ..,
        ] if dot == "." && literal(name) => Some(1),
        [
            Atom {
                kind: Kind::Bare(name),
                ..
            },
            ..,
        ] if names_command(name) => Some(0),
        _ => None,
    }
}

/// What an assignment stores into.
enum Assigned {
    Nothing,
    /// `$name` or `[type]$name`.
    Variable(String),
    /// An item or member, as in `$lines[3]` or `$item.Text`.
    Part(String),
}

/// Splits `$name = value` into what it assigns and the value's atoms.
fn assignment(atoms: &[Atom]) -> (Assigned, &[Atom]) {
    let Some(equals) = atoms
        .iter()
        .position(|atom| matches!(atom.kind, Kind::Punct(b'=')))
    else {
        return (Assigned::Nothing, atoms);
    };
    let targets = &atoms[..equals];
    if !targets.iter().all(|atom| {
        matches!(
            atom.kind,
            Kind::Variable(_) | Kind::Type(_) | Kind::Member(_) | Kind::Punct(b',')
        )
    }) {
        return (Assigned::Nothing, atoms);
    }
    let mut variables = targets
        .iter()
        .enumerate()
        .filter_map(|(index, atom)| match &atom.kind {
            Kind::Variable(name) => Some((index, key(name))),
            _ => None,
        });
    let assigned = match (variables.next(), variables.next()) {
        (Some((index, name)), None) if index + 1 == targets.len() => Assigned::Variable(name),
        (Some((_, name)), None) => Assigned::Part(name),
        _ => Assigned::Nothing,
    };
    (assigned, &atoms[equals + 1..])
}

/// The variable a name refers to, without case or a scope such as `script:`.
fn key(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    ["global:", "local:", "private:", "script:", "using:"]
        .iter()
        .find_map(|scope| name.strip_prefix(scope))
        .map_or_else(|| name.clone(), str::to_owned)
}

/// A .NET type name in lowercase without `System.`.
fn class_name(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    match name.strip_prefix("system.") {
        Some(rest) => rest.to_owned(),
        None => name,
    }
}

/// Whether an expression rewrites text: `-replace`, `.Replace()`,
/// `.Insert()`, `.Remove()`, or `+` with a literal.
fn transforms(atoms: &[Atom]) -> bool {
    // Concatenating a literal adds text to the content: a `+` before any
    // string or here-string.
    let mut concatenates = false;
    atoms.iter().any(|atom| match &atom.kind {
        Kind::Bare(text) if text == "+" => {
            concatenates = true;
            false
        }
        Kind::Quoted(_) | Kind::Here(_) => concatenates,
        Kind::Bare(text) => matches!(
            text.to_ascii_lowercase().as_str(),
            "-replace" | "-creplace" | "-ireplace"
        ),
        Kind::Member(name) => matches!(
            name.to_ascii_lowercase().as_str(),
            "insert" | "remove" | "replace"
        ),
        _ => false,
    })
}

/// The first argument of the `(...)` call joined to a method name.
fn first_argument(call: Option<&Atom>) -> Option<&[Atom]> {
    let Some(Atom {
        kind: Kind::Group(Bracket::Paren, script),
        spaced: false,
        ..
    }) = call
    else {
        return None;
    };
    let [pipeline] = script.as_slice() else {
        return None;
    };
    let [element] = pipeline.as_slice() else {
        return None;
    };
    element
        .atoms
        .split(|atom| matches!(atom.kind, Kind::Punct(b',')))
        .next()
}

/// Attributes embedded content to the program it came from or, for a
/// literal, to the program that writes it.
fn attribute(content: &Content, writer: &str) -> Finding {
    let program = if content.origin.is_empty() {
        writer
    } else {
        &content.origin
    };
    Finding {
        pattern: content.pattern,
        program: program.to_owned(),
    }
}

fn inline_write() -> Finding {
    Finding {
        pattern: Pattern::InlineScriptWrite,
        program: "PowerShell".to_owned(),
    }
}

struct Checker<'a> {
    context: Context<'a>,
    /// What assigned variables hold, by key. Assignments apply in source
    /// order, whatever block or branch they are in.
    variables: HashMap<String, Flow>,
    /// What each pipeline already run outputs, by address. A bracketed
    /// pipeline is checked as a script and then read again as the value of
    /// the expression around it, so without this, nested brackets would run
    /// their inner pipelines once per enclosing level.
    flows: HashMap<*const Pipeline, Option<Flow>>,
}

impl Checker<'_> {
    /// Returns the first finding in a script, its bracketed scripts included.
    fn script(&mut self, script: &Script) -> Option<Finding> {
        script
            .iter()
            .find_map(|pipeline| self.pipeline(pipeline).err())
    }

    fn pipeline(&mut self, pipeline: &Pipeline) -> Result<(), Finding> {
        for element in pipeline {
            let targets = element
                .redirects
                .iter()
                .flat_map(|redirect| &redirect.target);
            for atom in element.atoms.iter().chain(targets) {
                if let Kind::Group(_, script) = &atom.kind
                    && let Some(finding) = self.script(script)
                {
                    return Err(finding);
                }
            }
            self.dotnet(&element.atoms)?;
        }
        self.flow(pipeline).map(drop)
    }

    /// Runs a pipeline's elements in order and returns what it outputs,
    /// recording what an assignment stores.
    fn flow(&mut self, pipeline: &Pipeline) -> Result<Option<Flow>, Finding> {
        let key = std::ptr::from_ref(pipeline);
        if let Some(flow) = self.flows.get(&key) {
            return Ok(flow.clone());
        }
        let mut flow = None;
        let mut assigned = Assigned::Nothing;
        for (index, element) in pipeline.iter().enumerate() {
            let mut atoms = element.atoms.as_slice();
            if index == 0 {
                (assigned, atoms) = assignment(atoms);
            }
            flow = self.element(atoms, &element.redirects, flow.take(), index == 0)?;
        }
        match assigned {
            Assigned::Variable(name) => {
                match &flow {
                    Some(flow) => self.variables.insert(name, flow.clone()),
                    None => self.variables.remove(&name),
                };
            }
            // Replacing part of file content rewrites it.
            Assigned::Part(name) => {
                if let Some(held @ Flow::Read) = self.variables.get_mut(&name) {
                    *held = Flow::Transformed;
                }
            }
            Assigned::Nothing => {}
        }
        self.flows.insert(key, flow.clone());
        Ok(flow)
    }

    /// Checks one pipeline element and returns what it passes on.
    fn element(
        &mut self,
        atoms: &[Atom],
        redirects: &[Redirect],
        input: Option<Flow>,
        first: bool,
    ) -> Result<Option<Flow>, Finding> {
        // `return`, `throw`, and `exit` pass on the pipeline after them.
        let atoms = match atoms.split_first() {
            Some((
                Atom {
                    kind: Kind::Bare(word),
                    ..
                },
                rest,
            )) if first
                && matches!(
                    word.to_ascii_lowercase().as_str(),
                    "return" | "throw" | "exit"
                ) =>
            {
                rest
            }
            _ => atoms,
        };
        let output = match command_start(atoms) {
            Some(start) => self.command(&atoms[start..], input, first)?,
            None if first => self.expression(atoms),
            None => None,
        };
        self.redirect(output, redirects)
    }

    /// Checks a command, whose name is its first atom.
    fn command(
        &mut self,
        atoms: &[Atom],
        input: Option<Flow>,
        first: bool,
    ) -> Result<Option<Flow>, Finding> {
        let name = match atoms.first().map(|atom| &atom.kind) {
            Some(Kind::Bare(name) | Kind::Quoted(name)) => name.as_str(),
            _ => return Ok(None),
        };
        let arguments = runs(&atoms[1..]);
        Ok(match cmdlet(name, first) {
            Cmdlet::Read => Some(Flow::Read),
            Cmdlet::Output if arguments.is_empty() => input,
            Cmdlet::Output => Some(self.output(&arguments)),
            Cmdlet::PassThrough => input,
            Cmdlet::ForEach => input.map(|flow| match flow {
                Flow::Embedded(content) => Flow::Embedded(content),
                Flow::Read | Flow::Transformed => Flow::Transformed,
            }),
            Cmdlet::Writer(writer) => return self.writer(writer, &arguments, input),
            Cmdlet::Evaluate => return self.evaluate(&arguments, input),
            Cmdlet::NewObject if self.creates_writer(&arguments) => return Err(inline_write()),
            Cmdlet::NewObject | Cmdlet::Inert => None,
            Cmdlet::External => return self.external(atoms, input),
        })
    }

    /// What `Write-Output` passes on: its arguments, or the file content a
    /// single variable argument holds.
    fn output(&mut self, arguments: &[&[Atom]]) -> Flow {
        if let [argument] = arguments
            && let Some(flow @ (Flow::Read | Flow::Transformed)) = self.value(argument)
        {
            return flow;
        }
        let here = arguments
            .iter()
            .flat_map(|argument| argument.iter())
            .any(|atom| matches!(atom.kind, Kind::Here(_)));
        Flow::Embedded(Content {
            text: arguments
                .iter()
                .map(|argument| self.text(argument))
                .collect::<Vec<_>>()
                .join("\n"),
            pattern: if here {
                Pattern::HeredocToFile
            } else {
                Pattern::GeneratedToFile
            },
            origin: "Write-Output".to_owned(),
        })
    }

    /// Checks a writer cmdlet. Embedded or rewritten content written to a
    /// path that may be in the project is a finding; `Tee-Object` also
    /// passes its input on.
    fn writer(
        &mut self,
        writer: Writer,
        arguments: &[&[Atom]],
        input: Option<Flow>,
    ) -> Result<Option<Flow>, Finding> {
        let bound = bind(writer, arguments);
        let passed = if writer == Writer::TeeObject {
            input.clone()
        } else {
            None
        };
        // `-ItemType Directory` or a link takes no content.
        let item = bound
            .item_type
            .is_none_or(|item| self.text(item).eq_ignore_ascii_case("file"));
        if bound.variable || !item {
            return Ok(passed);
        }
        let content = match bound.value {
            Some(value) => self.value(value),
            None => input,
        };
        let finding = match content {
            Some(Flow::Embedded(content)) => attribute(&content, writer.name()),
            Some(Flow::Transformed) => Finding {
                pattern: Pattern::InPlaceEdit,
                program: writer.name().to_owned(),
            },
            Some(Flow::Read) | None => return Ok(passed),
        };
        let inside = bound.uncertain
            || bound.paths.is_empty()
            || bound.paths.iter().any(|path| self.counts(path));
        if inside { Err(finding) } else { Ok(passed) }
    }

    /// Sends an element's output through its redirections. Embedded or
    /// rewritten content redirected into the project is a finding, and
    /// output redirected anywhere no longer reaches the next element.
    fn redirect(
        &self,
        mut output: Option<Flow>,
        redirects: &[Redirect],
    ) -> Result<Option<Flow>, Finding> {
        for redirect in redirects.iter().filter(|redirect| redirect.output) {
            let finding = match output.take() {
                Some(Flow::Embedded(content)) => attribute(&content, &redirect.operator),
                Some(Flow::Transformed) => Finding {
                    pattern: Pattern::InPlaceEdit,
                    program: redirect.operator.clone(),
                },
                Some(Flow::Read) | None => continue,
            };
            if !redirect.target.is_empty() && self.counts(&redirect.target) {
                return Err(finding);
            }
        }
        Ok(output)
    }

    /// What an argument passes to a writer: literal text, or what an
    /// expression yields.
    fn value(&mut self, atoms: &[Atom]) -> Option<Flow> {
        match atoms.first()?.kind {
            Kind::Bare(_) => Some(self.literal(atoms)),
            _ => self.expression(atoms),
        }
    }

    /// What an expression yields: literal text, a variable's content, a
    /// bracketed pipeline's output, or a .NET read or regex replacement,
    /// rewritten by any transform that follows it.
    fn expression(&mut self, atoms: &[Atom]) -> Option<Flow> {
        let first = atoms.first()?;
        let flow = match &first.kind {
            // `"$text"` holds what `$text` holds.
            Kind::Quoted(_) if atoms.len() == 1 && self.interpolated(first).is_some() => {
                self.variable(self.interpolated(first)?)?
            }
            Kind::Quoted(_) | Kind::Here(_) => return Some(self.literal(atoms)),
            Kind::Variable(name) => self.variable(name)?,
            Kind::Group(Bracket::Paren, script) => match script.as_slice() {
                [pipeline] => self.flow(pipeline).ok().flatten()?,
                _ => return None,
            },
            Kind::Type(class) => {
                let Some(Kind::Member(member)) = atoms.get(1).map(|atom| &atom.kind) else {
                    return None;
                };
                match (
                    class_name(class).as_str(),
                    member.to_ascii_lowercase().as_str(),
                ) {
                    ("io.file", "readalllines" | "readalltext") => Flow::Read,
                    // `[regex]::Replace(text, ...)` rewrites its first argument.
                    ("regex" | "text.regularexpressions.regex", "replace") => {
                        match self.expression(first_argument(atoms.get(2))?)? {
                            Flow::Read => Flow::Transformed,
                            flow => flow,
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(match flow {
            Flow::Read if transforms(atoms) => Flow::Transformed,
            flow => flow,
        })
    }

    /// The variable a double-quoted string interpolates when it holds nothing
    /// else, as in `"$text"` or `"${text}"`.
    fn interpolated(&self, atom: &Atom) -> Option<&str> {
        let raw = self.context.source.get(atom.span.clone())?;
        let inner = raw
            .strip_prefix('"')?
            .strip_suffix('"')?
            .strip_prefix('$')?;
        let name = inner
            .strip_prefix('{')
            .and_then(|inner| inner.strip_suffix('}'))
            .unwrap_or(inner);
        let simple = name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':'));
        (!name.is_empty() && simple).then_some(name)
    }

    /// What a variable holds. `$args` holds arguments from the command line.
    fn variable(&self, name: &str) -> Option<Flow> {
        let key = key(name);
        if key == "args" {
            return Some(Flow::Embedded(Content {
                text: String::new(),
                pattern: Pattern::GeneratedToFile,
                origin: String::new(),
            }));
        }
        self.variables.get(&key).cloned()
    }

    /// Embedded text from literal atoms, such as `'a','b'` or a here-string.
    fn literal(&self, atoms: &[Atom]) -> Flow {
        let here = atoms.iter().any(|atom| matches!(atom.kind, Kind::Here(_)));
        Flow::Embedded(Content {
            text: self.text(atoms),
            pattern: if here {
                Pattern::HeredocToFile
            } else {
                Pattern::GeneratedToFile
            },
            origin: String::new(),
        })
    }

    /// Checks the script `Invoke-Expression` runs when it is literal.
    fn evaluate(
        &mut self,
        arguments: &[&[Atom]],
        input: Option<Flow>,
    ) -> Result<Option<Flow>, Finding> {
        let script = match arguments
            .iter()
            .find(|argument| parameter(argument).is_none())
        {
            Some(argument)
                if argument.iter().all(|atom| {
                    matches!(atom.kind, Kind::Bare(_) | Kind::Quoted(_) | Kind::Here(_))
                }) =>
            {
                Some(self.text(argument))
            }
            Some(_) => None,
            None => match input {
                Some(Flow::Embedded(content)) => Some(content.text),
                _ => None,
            },
        };
        if let Some(script) = script
            && self.context.depth < MAX_SCRIPT_DEPTH
            && let Some(finding) = scan(&script, self.context.scope, self.context.depth + 1)
        {
            return Err(finding);
        }
        Ok(None)
    }

    /// Checks an external program with the Bash checks, passing embedded
    /// input as its standard input.
    fn external(&mut self, atoms: &[Atom], input: Option<Flow>) -> Result<Option<Flow>, Finding> {
        let words = runs(atoms)
            .into_iter()
            .map(|run| self.word(run))
            .collect::<Vec<_>>();
        let stdin = match input {
            Some(Flow::Embedded(mut content)) => {
                if content.origin.is_empty() {
                    content.origin = words
                        .first()
                        .map(|word| program_name(&word.text))
                        .unwrap_or_default();
                }
                Some(content)
            }
            _ => None,
        };
        let command = Command {
            words,
            ..Command::default()
        };
        Ok(inspect(&command, stdin, self.context)?.map(Flow::Embedded))
    }

    /// Whether `New-Object` creates a `StreamWriter`.
    fn creates_writer(&self, arguments: &[&[Atom]]) -> bool {
        let mut index = 0;
        let class = loop {
            let Some(&argument) = arguments.get(index) else {
                return false;
            };
            index += 1;
            match parameter(argument) {
                None => break argument,
                Some((name, attached)) if "typename".starts_with(&name.to_ascii_lowercase()) => {
                    match attached.or_else(|| arguments.get(index).copied()) {
                        Some(value) => break value,
                        None => return false,
                    }
                }
                Some(_) => {}
            }
        };
        match class.first().map(|atom| &atom.kind) {
            Some(Kind::Bare(name) | Kind::Quoted(name)) => class_name(name) == "io.streamwriter",
            _ => false,
        }
    }

    /// Checks an element for .NET file-write calls such as
    /// `[IO.File]::WriteAllText(...)` and `[IO.StreamWriter]::new(...)`. A
    /// call whose first argument is certainly outside the project is allowed.
    fn dotnet(&self, atoms: &[Atom]) -> Result<(), Finding> {
        for (index, atom) in atoms.iter().enumerate() {
            let (Kind::Type(class), Some(Kind::Member(member))) =
                (&atom.kind, atoms.get(index + 1).map(|atom| &atom.kind))
            else {
                continue;
            };
            let member = member.to_ascii_lowercase();
            let writes = match class_name(class).as_str() {
                "io.file" => FILE_WRITES.contains(&member.as_str()),
                "io.streamwriter" => member == "new",
                _ => false,
            };
            if writes && self.call_inside(atoms.get(index + 2)) {
                return Err(inline_write());
            }
        }
        Ok(())
    }

    /// Whether a .NET call's first argument may be a path in the project.
    fn call_inside(&self, call: Option<&Atom>) -> bool {
        first_argument(call).is_none_or(|path| path.is_empty() || self.counts(path))
    }

    /// Whether a write to the path `atoms` name may reach a project file.
    fn counts(&self, atoms: &[Atom]) -> bool {
        self.context.counts(&self.word(atoms))
    }

    /// An argument as a word of the Bash command model.
    fn word(&self, atoms: &[Atom]) -> Word {
        let span = match (atoms.first(), atoms.last()) {
            (Some(first), Some(last)) => first.span.start..last.span.end,
            _ => 0..0,
        };
        Word {
            text: self.text(atoms),
            assignment: false,
            span,
        }
    }

    /// An argument's text: literal values, and the source of anything else.
    fn text(&self, atoms: &[Atom]) -> String {
        atoms
            .iter()
            .map(|atom| match &atom.kind {
                Kind::Bare(text) | Kind::Quoted(text) | Kind::Here(text) => text.as_str(),
                Kind::Punct(b',') => "\n",
                _ => self
                    .context
                    .source
                    .get(atom.span.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }
}
