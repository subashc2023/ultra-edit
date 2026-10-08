//! Classifies Bash and PowerShell commands for the Claude Code `PreToolUse`
//! guard.
//!
//! A command is denied when its text carries file content or edit logic that
//! the shell writes into project files: heredoc and `echo` redirection, inline
//! interpreter code that calls a file-write API, and in-place editors, and
//! their PowerShell forms. Anything the lexers cannot place with confidence is
//! allowed, because a false positive blocks legitimate work. A write target
//! counts as outside the project only when that is certain.

use std::cell::RefCell;
use std::fmt;
use std::ops::Range;

mod powershell;
mod scope;

pub use scope::{SCOPE_VARIABLES, Scope, is_absolute};

/// Setting this environment variable to `allow` disables the guard.
pub const ESCAPE_HATCH: &str = "ULTRA_EDIT_SHELL_WRITES";

/// How deep `bash -c`, `eval`, and backtick scripts are classified.
const MAX_SCRIPT_DEPTH: usize = 3;
/// How deep command substitutions are parsed before the rest is skipped.
const MAX_NESTING: usize = 64;
/// Words, redirections, and PowerShell atoms lexed from one script before the
/// rest is allowed unread. Real commands stay orders of magnitude below it;
/// the bound keeps a command of millions of tiny statements from costing
/// gigabytes of memory and more than the hook's timeout.
const MAX_TOKENS: usize = 250_000;
/// Bytes of interpreter code examined for each candidate write call.
const MAX_CALL_SPAN: usize = 512;
/// Program names longer than this are shortened in the deny reason.
const MAX_PROGRAM_CHARS: usize = 16;
/// Files a command list writes that later commands may run as scripts.
const MAX_WRITTEN: usize = 64;
/// The largest recorded script text; longer appends are not recorded.
const MAX_SCRIPT_BYTES: usize = 1 << 20;

/// Reserved words that can precede a command without changing what it runs.
const KEYWORDS: &[&str] = &[
    "!", "{", "}", "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "esac",
];

/// Programs whose standard output carries the content of their standard input.
const FILTERS: &[&str] = &[
    "base32", "base64", "cat", "column", "cut", "dos2unix", "egrep", "envsubst", "expand", "fgrep",
    "fmt", "fold", "grep", "head", "iconv", "jq", "nl", "paste", "rev", "sort", "tac", "tail",
    "tr", "unexpand", "uniq", "unix2dos", "xxd", "yq",
];

/// PowerShell parameters whose value is the next word.
const POWERSHELL_VALUED: &[&str] = &[
    "config",
    "configurationname",
    "custompipename",
    "ep",
    "ex",
    "executionpolicy",
    "if",
    "inp",
    "inputformat",
    "o",
    "of",
    "outputformat",
    "psconsolefile",
    "settings",
    "settingsfile",
    "v",
    "version",
    "w",
    "wd",
    "windowstyle",
    "workingdirectory",
];

/// How a denied command writes embedded content into files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Heredoc or here-string content redirected or `tee`d into a file.
    HeredocToFile,
    /// `echo` or `printf` output redirected or `tee`d into a file.
    GeneratedToFile,
    /// Inline interpreter code that calls a file-write API.
    InlineScriptWrite,
    /// A script saved outside the project, by the Write tool or by the shell,
    /// or run after the command saved it, that may write a project file: it
    /// calls a file-write API on a path that is not a literal outside it.
    ScriptFileWrite,
    /// An in-place editor such as `sed -i` or `perl -pi`.
    InPlaceEdit,
    /// Embedded content applied to files by `patch`, `git apply`, or `ultra-edit`.
    AppliedContent,
}

/// A detected shell file write and the program it is attributed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub pattern: Pattern,
    pub program: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let program = match self.program.char_indices().nth(MAX_PROGRAM_CHARS) {
            Some((end, _)) => &self.program[..end],
            None => &self.program,
        };
        match self.pattern {
            Pattern::HeredocToFile => {
                write!(formatter, "a heredoc written to a file by `{program}`")
            }
            Pattern::GeneratedToFile => write!(formatter, "`{program}` output written to a file"),
            Pattern::InlineScriptWrite => {
                write!(formatter, "inline `{program}` code that writes a file")
            }
            Pattern::ScriptFileWrite => {
                write!(
                    formatter,
                    "a `{program}` script that may write project files"
                )
            }
            Pattern::InPlaceEdit => write!(formatter, "in-place editing with `{program}`"),
            Pattern::AppliedContent => {
                write!(
                    formatter,
                    "embedded content applied to files by `{program}`"
                )
            }
        }
    }
}

/// The `permissionDecisionReason` shown to Claude for a denied command.
pub fn deny_reason(finding: &Finding) -> String {
    if finding.pattern == Pattern::ScriptFileWrite {
        return format!(
            "Ultra Edit guard blocked {finding}: scripted edits can lose line endings or \
             encoding. Edit files with ultra_edit or native Edit, or create them with Write; a \
             script may write only literal paths outside the project. If the user explicitly \
             asked for it, report that the Ultra Edit guard blocked it; {ESCAPE_HATCH}=allow \
             disables the guard."
        );
    }
    format!(
        "Ultra Edit guard blocked {finding}: shell writes can lose backslashes, line endings, or \
         encoding. Edit existing files with ultra_edit or native Edit; create files, including \
         multiline command input, with Write. If the user explicitly asked for it, report that \
         the Ultra Edit guard blocked it; {ESCAPE_HATCH}=allow disables the guard."
    )
}

/// Returns the first shell file write in a Bash tool command, counting every
/// file target as inside the project.
pub fn classify(command: &str) -> Option<Finding> {
    classify_bash(command, &Scope::default())
}

/// Returns the first write into the project in a Bash tool command.
pub fn classify_bash(command: &str, scope: &Scope) -> Option<Finding> {
    scan(
        &command.replace("\r\n", "\n"),
        scope,
        0,
        &Written::default(),
    )
}

/// Returns the first write into the project in a PowerShell tool command.
pub fn classify_powershell(command: &str, scope: &Scope) -> Option<Finding> {
    powershell::scan(&command.replace("\r\n", "\n"), scope, 0)
}

/// Returns a finding when the Write tool saves, outside the project, a script
/// that may write project files. Files inside the project, and files that are
/// not scripts by their `#!` line or extension, are never judged.
pub fn classify_write(path: &str, content: &str, scope: &Scope) -> Option<Finding> {
    if scope.literal_inside(path) {
        return None;
    }
    let written = Written::default();
    let context = Context {
        source: "",
        dialect: Dialect::Bash,
        scope,
        depth: 0,
        written: &written,
    };
    saved_script(path, &content.replace("\r\n", "\n"), context)
}

/// Checks a Bash script. `written` holds the files that commands before it
/// wrote, shared with the scripts it runs through `bash -c`, `eval`, and
/// backticks.
fn scan(script: &str, scope: &Scope, depth: usize, written: &Written) -> Option<Finding> {
    let mut lexer = Lexer::new(script);
    let commands = lexer.list(false);
    let context = Context {
        source: script,
        dialect: Dialect::Bash,
        scope,
        depth,
        written,
    };
    check(&commands, context)
        .or_else(|| {
            lexer
                .nested
                .iter()
                .find_map(|commands| check(commands, context))
        })
        .or_else(|| {
            (depth < MAX_SCRIPT_DEPTH)
                .then(|| {
                    lexer
                        .backticks
                        .iter()
                        .find_map(|body| scan(body, scope, depth + 1, written))
                })
                .flatten()
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    Bash,
    PowerShell,
}

/// What checking a lexed script needs besides its commands.
#[derive(Clone, Copy)]
struct Context<'a> {
    /// The script whose bytes word spans index.
    source: &'a str,
    dialect: Dialect,
    scope: &'a Scope,
    /// How many enclosing `bash -c`, `eval`, backtick, or `pwsh -Command`
    /// scripts this one runs in.
    depth: usize,
    /// Files outside the project that earlier commands wrote, with their text.
    written: &'a Written,
}

/// Files that commands of one script wrote outside the project, as (target
/// word, text), so a later command that runs one as a script can be checked.
pub(super) type Written = RefCell<Vec<(String, String)>>;

impl Context<'_> {
    /// Notes that `target`, a file outside the project, now holds `text`, or
    /// `text` appended to what it held (a write inside the project is reported
    /// before this is reached). A script it saves that may write project files
    /// is reported here, whether or not a later command runs it.
    fn record(&self, target: &Word, text: &str, append: bool) -> Result<(), Finding> {
        if !is_file(&target.text) {
            return Ok(());
        }
        let path = normalized(&target.text);
        let text = {
            let mut written = self.written.borrow_mut();
            let earlier = written.iter().rposition(|(known, _)| *known == path);
            match (append, earlier) {
                (true, Some(at)) => {
                    let content = &mut written[at].1;
                    if content.len() + text.len() <= MAX_SCRIPT_BYTES {
                        content.push_str(text);
                    }
                    content.clone()
                }
                _ if written.len() < MAX_WRITTEN => {
                    written.push((path.clone(), text.to_owned()));
                    text.to_owned()
                }
                _ => text.to_owned(),
            }
        };
        match saved_script(&path, &text, *self) {
            Some(finding) => Err(finding),
            None => Ok(()),
        }
    }

    /// The text of a script file that `path` names, when an earlier command
    /// of this script wrote it: the same path, or the same name when `path`
    /// is a bare name (a `cd` may lie between them). Files on disk are not
    /// read, since a script this session saved was judged when it was saved.
    fn script(&self, path: &str) -> Option<String> {
        let path = normalized(path);
        let bare = !path.contains(['/', '\\']);
        let name = |text: &str| text.rsplit(['/', '\\']).next().unwrap_or(text).to_owned();
        let written = self.written.borrow();
        written
            .iter()
            .rev()
            .find(|(target, _)| *target == path)
            .or_else(|| {
                written
                    .iter()
                    .rev()
                    .find(|(target, _)| bare && name(target) == path)
            })
            .map(|(_, text)| text.clone())
    }

    /// Whether a write to `word` may reach a file inside the project.
    fn counts(&self, word: &Word) -> bool {
        let raw = self.source.get(word.span.clone()).unwrap_or_default();
        is_file(&word.text) && self.scope.inside(self.dialect, raw)
    }

    /// Whether an in-place editor changes a project file: it has no file
    /// operand, so the target is unknown, or one that may be inside.
    fn edits(&self, files: &[&Word]) -> bool {
        files.is_empty() || files.iter().any(|file| self.counts(file))
    }
}

#[derive(Clone, Debug, Default)]
struct Word {
    /// The value after quote removal. Expansions and substitutions stay
    /// verbatim, so inline code keeps the text a substitution would read.
    text: String,
    /// Whether the word starts with an unquoted `NAME=` assignment.
    assignment: bool,
    /// Where the word lies in its script, for reading a target's quoting.
    span: Range<usize>,
}

#[derive(Debug)]
enum Redirect {
    /// Output to a path; `stdout` is false for other descriptors, and
    /// `append` is true for `>>`.
    Output {
        stdout: bool,
        append: bool,
        target: Word,
    },
    /// Heredoc or here-string content on standard input.
    Input(String),
    /// Descriptor duplication, input files, and other descriptors.
    Other,
}

#[derive(Debug, Default)]
struct Command {
    words: Vec<Word>,
    redirects: Vec<Redirect>,
    /// Whether standard output feeds the next command.
    piped: bool,
}

impl Command {
    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.redirects.is_empty()
    }
}

/// A heredoc whose body starts after the next newline.
struct Heredoc {
    command: usize,
    redirect: usize,
    delimiter: String,
    strip_tabs: bool,
}

struct Lexer<'a> {
    source: &'a [u8],
    at: usize,
    /// Commands inside `$(...)` and process substitutions.
    nested: Vec<Vec<Command>>,
    /// Backtick bodies, which are classified as separate scripts.
    backticks: Vec<String>,
    nesting: usize,
    /// Words and redirections lexed so far, bounded by `MAX_TOKENS`.
    tokens: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            at: 0,
            nested: Vec::new(),
            backticks: Vec::new(),
            nesting: 0,
            tokens: 0,
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.source.get(self.at + offset).copied()
    }

    /// Lexes commands up to the end of input or, in a substitution, past its
    /// closing parenthesis.
    fn list(&mut self, substitution: bool) -> Vec<Command> {
        let mut commands = vec![Command::default()];
        let mut heredocs = Vec::new();
        let mut depth = 0_usize;
        while let Some(byte) = self.peek(0) {
            if self.tokens > MAX_TOKENS {
                // Every enclosing list stops as well.
                self.at = self.source.len();
                break;
            }
            match byte {
                b' ' | b'\t' | b'\r' => self.at += 1,
                b'\\' if self.peek(1) == Some(b'\n') => self.at += 2,
                b'\n' => {
                    self.at += 1;
                    self.heredoc_bodies(&mut commands, &mut heredocs);
                    end(&mut commands, false);
                }
                b'#' => {
                    while self.peek(0).is_some_and(|byte| byte != b'\n') {
                        self.at += 1;
                    }
                }
                // `((` in command position is arithmetic, where `<<` is a shift.
                b'(' if self.peek(1) == Some(b'(')
                    && commands.last().is_some_and(Command::is_empty) =>
                {
                    self.skip_balanced();
                }
                b'(' => {
                    depth += 1;
                    self.at += 1;
                    end(&mut commands, false);
                }
                b')' => {
                    self.at += 1;
                    if depth == 0 && substitution {
                        break;
                    }
                    depth = depth.saturating_sub(1);
                    end(&mut commands, false);
                }
                b';' => {
                    self.at += 1;
                    end(&mut commands, false);
                }
                b'&' if self.peek(1) == Some(b'>') => {
                    self.redirect(&mut commands, &mut heredocs, None);
                }
                b'&' => {
                    self.at += if self.peek(1) == Some(b'&') { 2 } else { 1 };
                    end(&mut commands, false);
                }
                b'|' => {
                    let piped = self.peek(1) != Some(b'|');
                    self.at += if matches!(self.peek(1), Some(b'|' | b'&')) {
                        2
                    } else {
                        1
                    };
                    end(&mut commands, piped);
                }
                b'<' | b'>' if self.peek(1) != Some(b'(') => {
                    self.redirect(&mut commands, &mut heredocs, None);
                }
                b'0'..=b'9' => {
                    let digits = self.source[self.at..]
                        .iter()
                        .take_while(|byte| byte.is_ascii_digit())
                        .count();
                    if matches!(self.peek(digits), Some(b'<' | b'>'))
                        && self.peek(digits + 1) != Some(b'(')
                    {
                        let descriptor = std::str::from_utf8(&self.source[self.at..][..digits])
                            .ok()
                            .and_then(|digits| digits.parse().ok())
                            .unwrap_or(u32::MAX);
                        self.at += digits;
                        self.redirect(&mut commands, &mut heredocs, Some(descriptor));
                    } else {
                        self.push_word(&mut commands);
                    }
                }
                _ => self.push_word(&mut commands),
            }
        }
        commands
    }

    fn push_word(&mut self, commands: &mut [Command]) {
        self.tokens += 1;
        let word = self.word();
        if let Some(command) = commands.last_mut() {
            command.words.push(word);
        }
    }

    fn redirect(
        &mut self,
        commands: &mut [Command],
        heredocs: &mut Vec<Heredoc>,
        descriptor: Option<u32>,
    ) {
        self.tokens += 1;
        let stdout = matches!(descriptor, None | Some(1));
        let stdin = matches!(descriptor, None | Some(0));
        let redirect = match (self.peek(0), self.peek(1), self.peek(2)) {
            (Some(b'&'), _, next) => {
                self.at += if next == Some(b'>') { 3 } else { 2 };
                Redirect::Output {
                    stdout: true,
                    append: next == Some(b'>'),
                    target: self.target(),
                }
            }
            (Some(b'>'), Some(b'&'), _) => {
                self.at += 2;
                let target = self.target();
                if target.text.starts_with('$')
                    || target
                        .text
                        .trim_end_matches('-')
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
                {
                    Redirect::Other
                } else {
                    Redirect::Output {
                        stdout,
                        append: false,
                        target,
                    }
                }
            }
            (Some(b'>'), Some(next @ (b'>' | b'|')), _) => {
                self.at += 2;
                Redirect::Output {
                    stdout,
                    append: next == b'>',
                    target: self.target(),
                }
            }
            (Some(b'>'), _, _) => {
                self.at += 1;
                Redirect::Output {
                    stdout,
                    append: false,
                    target: self.target(),
                }
            }
            (Some(b'<'), Some(b'<'), Some(b'<')) => {
                self.at += 3;
                let content = self.target().text;
                if stdin {
                    Redirect::Input(content)
                } else {
                    Redirect::Other
                }
            }
            (Some(b'<'), Some(b'<'), _) => {
                self.at += 2;
                let strip_tabs = self.peek(0) == Some(b'-');
                self.at += usize::from(strip_tabs);
                let delimiter = self.target().text;
                if let Some(command) = commands.len().checked_sub(1) {
                    heredocs.push(Heredoc {
                        command,
                        redirect: commands[command].redirects.len(),
                        delimiter,
                        strip_tabs,
                    });
                }
                if stdin {
                    Redirect::Input(String::new())
                } else {
                    Redirect::Other
                }
            }
            (Some(b'<'), Some(b'&' | b'>'), _) => {
                self.at += 2;
                self.target();
                Redirect::Other
            }
            _ => {
                self.at += 1;
                self.target();
                Redirect::Other
            }
        };
        if let Some(command) = commands.last_mut() {
            command.redirects.push(redirect);
        }
    }

    /// Lexes the word after a redirection operator, or nothing at a terminator.
    fn target(&mut self) -> Word {
        while let Some(byte) = self.peek(0) {
            match byte {
                b' ' | b'\t' | b'\r' => self.at += 1,
                b'\\' if self.peek(1) == Some(b'\n') => self.at += 2,
                _ => break,
            }
        }
        match (self.peek(0), self.peek(1)) {
            (None | Some(b'\n' | b';' | b'&' | b'|' | b'(' | b')'), _) => Word::default(),
            (Some(b'<' | b'>'), next) if next != Some(b'(') => Word::default(),
            _ => self.word(),
        }
    }

    fn word(&mut self) -> Word {
        let start = self.at;
        let mut text = Vec::new();
        // Length of `text` before its first quoted, escaped, or expanded byte.
        let mut plain = usize::MAX;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'<' | b'>' if self.peek(1) == Some(b'(') => {
                    plain = plain.min(text.len());
                    self.substitution(&mut text);
                }
                b' ' | b'\t' | b'\r' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' => {
                    break;
                }
                b'\\' => {
                    plain = plain.min(text.len());
                    match self.peek(1) {
                        Some(b'\n') => self.at += 2,
                        Some(next) => {
                            text.push(next);
                            self.at += 2;
                        }
                        None => {
                            text.push(byte);
                            self.at += 1;
                        }
                    }
                }
                b'\'' => {
                    plain = plain.min(text.len());
                    self.at += 1;
                    while let Some(byte) = self.peek(0) {
                        self.at += 1;
                        if byte == b'\'' {
                            break;
                        }
                        text.push(byte);
                    }
                }
                b'"' => {
                    plain = plain.min(text.len());
                    self.at += 1;
                    self.double_quoted(&mut text);
                }
                b'$' => {
                    plain = plain.min(text.len());
                    self.dollar(&mut text);
                }
                b'`' => {
                    plain = plain.min(text.len());
                    self.backtick(&mut text);
                }
                _ => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
        if self.at == start {
            // Never stall on a byte the caller did not expect.
            self.at += 1;
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        let assignment = is_assignment(&text, plain);
        Word {
            text,
            assignment,
            span: start..self.at,
        }
    }

    fn double_quoted(&mut self, text: &mut Vec<u8>) {
        while let Some(byte) = self.peek(0) {
            match byte {
                b'"' => {
                    self.at += 1;
                    return;
                }
                b'\\' => match self.peek(1) {
                    Some(b'\n') => self.at += 2,
                    Some(next @ (b'$' | b'`' | b'"' | b'\\')) => {
                        text.push(next);
                        self.at += 2;
                    }
                    _ => {
                        text.push(byte);
                        self.at += 1;
                    }
                },
                b'$' => self.dollar(text),
                b'`' => self.backtick(text),
                _ => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
    }

    fn dollar(&mut self, text: &mut Vec<u8>) {
        let start = self.at;
        match self.peek(1) {
            Some(b'(') if self.peek(2) == Some(b'(') => {
                self.at += 1;
                self.skip_balanced();
                text.extend_from_slice(&self.source[start..self.at]);
            }
            Some(b'(') => self.substitution(text),
            Some(b'{') => {
                self.at += 1;
                self.skip_balanced();
                text.extend_from_slice(&self.source[start..self.at]);
            }
            Some(b'\'') => {
                self.at += 2;
                self.ansi_c(text);
            }
            Some(b'"') => {
                self.at += 2;
                self.double_quoted(text);
            }
            _ => {
                text.push(b'$');
                self.at += 1;
            }
        }
    }

    /// Lexes a `$(...)`, `<(...)`, or `>(...)` substitution into `nested`.
    fn substitution(&mut self, text: &mut Vec<u8>) {
        let start = self.at;
        if self.nesting < MAX_NESTING {
            self.at += 2;
            self.nesting += 1;
            let commands = self.list(true);
            self.nesting -= 1;
            self.nested.push(commands);
        } else {
            self.at += 1;
            self.skip_balanced();
        }
        text.extend_from_slice(&self.source[start..self.at]);
    }

    /// Records a backtick body, which runs as its own script.
    fn backtick(&mut self, text: &mut Vec<u8>) {
        let start = self.at;
        self.at += 1;
        let mut body = Vec::new();
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'`' => break,
                b'\\' => match self.peek(0) {
                    Some(next @ (b'`' | b'\\' | b'$')) => {
                        body.push(next);
                        self.at += 1;
                    }
                    _ => body.push(byte),
                },
                _ => body.push(byte),
            }
        }
        text.extend_from_slice(&self.source[start..self.at]);
        self.backticks
            .push(String::from_utf8_lossy(&body).into_owned());
    }

    /// Reads a `$'...'` string after its opening quote.
    fn ansi_c(&mut self, text: &mut Vec<u8>) {
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'\'' => return,
                b'\\' => {
                    let Some(next) = self.peek(0) else {
                        return;
                    };
                    self.at += 1;
                    text.push(match next {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        other => other,
                    });
                }
                _ => text.push(byte),
            }
        }
    }

    /// Skips a bracketed span that starts at the current `(` or `{`.
    fn skip_balanced(&mut self) {
        let Some(open) = self.peek(0) else {
            return;
        };
        let close = if open == b'{' { b'}' } else { b')' };
        let mut depth = 0_usize;
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'\\' => self.at += 1,
                b'\'' | b'"' => {
                    while let Some(inner) = self.peek(0) {
                        self.at += 1;
                        if inner == byte {
                            break;
                        }
                        if inner == b'\\' && byte == b'"' {
                            self.at += 1;
                        }
                    }
                }
                _ if byte == open => depth += 1,
                _ if byte == close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
        self.at = self.at.min(self.source.len());
    }

    /// Reads the bodies of heredocs whose operators preceded this newline.
    fn heredoc_bodies(&mut self, commands: &mut [Command], heredocs: &mut Vec<Heredoc>) {
        let source = self.source;
        for heredoc in heredocs.drain(..) {
            let mut body = String::new();
            while let Some(rest) = source.get(self.at..).filter(|rest| !rest.is_empty()) {
                let length = rest
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .unwrap_or(rest.len());
                let mut line = &rest[..length];
                self.at += (length + 1).min(rest.len());
                if heredoc.strip_tabs {
                    while let [b'\t', tail @ ..] = line {
                        line = tail;
                    }
                }
                if line == heredoc.delimiter.as_bytes() {
                    break;
                }
                body.push_str(&String::from_utf8_lossy(line));
                body.push('\n');
            }
            if let Some(Redirect::Input(content)) = commands
                .get_mut(heredoc.command)
                .and_then(|command| command.redirects.get_mut(heredoc.redirect))
            {
                *content = body;
            }
        }
    }
}

/// Ends the current command unless it is still empty, as after `(`.
fn end(commands: &mut Vec<Command>, piped: bool) {
    if let Some(command) = commands.last_mut()
        && !command.is_empty()
    {
        command.piped = piped;
        commands.push(Command::default());
    }
}

/// Whether `text` starts with `NAME=`, `NAME+=`, or `NAME[...]=` before
/// byte `plain`, where quoting or expansion begins.
fn is_assignment(text: &str, plain: usize) -> bool {
    let Some(equals) = text.find('=').filter(|&equals| equals < plain) else {
        return false;
    };
    let name = text[..equals].strip_suffix('+').unwrap_or(&text[..equals]);
    let name = match name.split_once('[') {
        Some((name, index)) if index.ends_with(']') => name,
        _ => name,
    };
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

/// Embedded text on a command's standard input, and where it came from.
#[derive(Clone, Debug)]
struct Content {
    text: String,
    pattern: Pattern,
    /// The program the text came from; empty for a PowerShell literal, which
    /// is attributed to the program that writes it.
    origin: String,
}

impl Content {
    fn finding(self) -> Finding {
        Finding {
            pattern: self.pattern,
            program: self.origin,
        }
    }
}

fn check(commands: &[Command], context: Context) -> Option<Finding> {
    let mut piped = None;
    for command in commands {
        match inspect(command, piped.take(), context) {
            Err(finding) => return Some(finding),
            Ok(output) => piped = output.filter(|_| command.piped),
        }
    }
    None
}

#[derive(Clone, Copy)]
enum Language {
    Python,
    Node,
    Perl,
    Ruby,
    Php,
    PowerShell,
}

enum Family {
    Generator,
    Tee,
    Filter,
    Sed,
    Awk,
    Interpreter(Language),
    Shell,
    Source,
    Eval,
    Find,
    Applier,
    Other,
}

fn family(program: &str) -> Family {
    let versioned = |base: &str| {
        program.strip_prefix(base).is_some_and(|version| {
            version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
    };
    match program {
        "echo" | "printf" => Family::Generator,
        "tee" => Family::Tee,
        "sed" | "gsed" => Family::Sed,
        "awk" | "gawk" | "mawk" | "nawk" => Family::Awk,
        "bash" | "sh" | "zsh" | "dash" | "ksh" => Family::Shell,
        "source" | "." => Family::Source,
        "eval" => Family::Eval,
        "find" => Family::Find,
        "git" | "patch" | "ultra-edit" => Family::Applier,
        "node" | "nodejs" => Family::Interpreter(Language::Node),
        "py" => Family::Interpreter(Language::Python),
        "pwsh" | "powershell" => Family::Interpreter(Language::PowerShell),
        _ if FILTERS.contains(&program) => Family::Filter,
        _ if versioned("python") => Family::Interpreter(Language::Python),
        _ if versioned("perl") => Family::Interpreter(Language::Perl),
        _ if versioned("ruby") => Family::Interpreter(Language::Ruby),
        _ if versioned("php") => Family::Interpreter(Language::Php),
        _ => Family::Other,
    }
}

/// Checks one simple command. `Ok` carries embedded content that the command
/// passes on to its standard output.
fn inspect(
    command: &Command,
    piped: Option<Content>,
    context: Context,
) -> Result<Option<Content>, Finding> {
    let Some((first, arguments)) = resolve(&command.words).split_first() else {
        return Ok(None);
    };
    let program = program_name(&first.text);
    let heredoc = command
        .redirects
        .iter()
        .rev()
        .find_map(|redirect| match redirect {
            Redirect::Input(text) => Some(Content {
                text: text.clone(),
                pattern: Pattern::HeredocToFile,
                origin: program.clone(),
            }),
            _ => None,
        });
    let stdin = heredoc.or(piped);
    let to_file = command.redirects.iter().any(|redirect| {
        matches!(redirect, Redirect::Output { stdout: true, target, .. } if context.counts(target))
    });
    let found = |pattern| {
        Err(Finding {
            pattern,
            program: program.clone(),
        })
    };
    match family(&program) {
        Family::Generator => {
            let text = join(arguments);
            let content = Content {
                // printf, and echo -e, turn `\047` and `\"` into the quotes
                // a write call needs.
                text: if program == "printf"
                    || arguments.first().is_some_and(|word| word.text == "-e")
                {
                    unescape(&text)
                } else {
                    text
                },
                pattern: Pattern::GeneratedToFile,
                origin: program.clone(),
            };
            if to_file {
                Err(content.finding())
            } else {
                record_outputs(command, &content.text, context)?;
                Ok(Some(content))
            }
        }
        Family::Tee => match stdin {
            Some(content) if tee_writes(arguments, context) => Err(content.finding()),
            stdin => {
                if let Some(content) = &stdin {
                    let append = arguments.iter().any(|word| {
                        word.text == "--append"
                            || word.text.strip_prefix('-').is_some_and(|cluster| {
                                !cluster.starts_with('-') && cluster.contains('a')
                            })
                    });
                    for file in arguments.iter().filter(|word| !word.text.starts_with('-')) {
                        context.record(file, &content.text, append)?;
                    }
                }
                Ok(stdin)
            }
        },
        Family::Sed if sed_in_place(arguments).is_some_and(|files| context.edits(&files)) => {
            found(Pattern::InPlaceEdit)
        }
        Family::Awk if awk_in_place(arguments).is_some_and(|files| context.edits(&files)) => {
            found(Pattern::InPlaceEdit)
        }
        Family::Filter | Family::Sed | Family::Awk => match stdin {
            Some(content) if to_file => Err(content.finding()),
            stdin => {
                // `cat > /tmp/edit.py <<'EOF'` leaves a script a later command may run.
                if let (Family::Filter, Some(content)) = (family(&program), &stdin) {
                    record_outputs(command, &content.text, context)?;
                }
                Ok(stdin)
            }
        },
        Family::Interpreter(language) => {
            let invocation = invocation(language, &program, arguments);
            let files = arguments.get(invocation.operands..).unwrap_or_default();
            if invocation.in_place && context.edits(&files.iter().collect::<Vec<_>>()) {
                return found(Pattern::InPlaceEdit);
            }
            // A syntax check such as `perl -c` or `node --check` runs nothing.
            if invocation.check {
                return Ok(None);
            }
            // A script file runs instead of inline code. Its text counts when an
            // earlier command of this script wrote it.
            let script = invocation
                .script_at
                .and_then(|at| arguments.get(at))
                .and_then(|word| context.script(&word.text));
            if invocation.code.is_empty()
                && invocation.script
                && let Some(script) = script
            {
                return match script_writes(&program, &script, context) {
                    Some(finding) => Err(finding),
                    None => Ok(None),
                };
            }
            // Without inline code or a script file, standard input is the program.
            let code = if !invocation.code.is_empty() {
                invocation.code.join("\n")
            } else if invocation.script {
                String::new()
            } else {
                stdin.map(|content| content.text).unwrap_or_default()
            };
            let writes = match language {
                // PowerShell code gets the PowerShell tool's checks, reported
                // as inline code of the program that runs it.
                Language::PowerShell => {
                    let foreign;
                    let scope = match context.dialect {
                        Dialect::PowerShell => context.scope,
                        // Bash may already have expanded `$` in the code.
                        Dialect::Bash => {
                            foreign = context.scope.foreign();
                            &foreign
                        }
                    };
                    context.depth < MAX_SCRIPT_DEPTH
                        && powershell::scan(&code, scope, context.depth + 1).is_some()
                }
                _ => writes(language, &code),
            };
            if writes {
                found(Pattern::InlineScriptWrite)
            } else {
                Ok(None)
            }
        }
        Family::Shell => match shell_script(arguments, stdin) {
            Some(ShellInput::Code(script)) => nested(&script, context),
            Some(ShellInput::File(word)) => match context.script(&word.text) {
                Some(script) => script_writes(&program, &script, context).map_or(Ok(None), Err),
                None => Ok(None),
            },
            None => Ok(None),
        },
        // `source FILE` and `. FILE` run a script in the current shell.
        Family::Source => {
            let file = arguments.iter().find(|word| word.text != "--");
            match file.and_then(|word| context.script(&word.text)) {
                Some(script) => script_writes("source", &script, context).map_or(Ok(None), Err),
                None => Ok(None),
            }
        }
        Family::Eval => nested(&join(arguments), context),
        Family::Find => find_exec(arguments, context),
        Family::Applier if stdin.is_some() && applies(&program, arguments) => {
            found(Pattern::AppliedContent)
        }
        Family::Applier => Ok(None),
        // `chmod +x /tmp/edit.py && /tmp/edit.py` runs the script by its path;
        // without a `#!` line, the shell runs it as a shell script.
        Family::Other if first.text.contains(['/', '\\']) => {
            let Some(script) = context.script(&first.text) else {
                return Ok(None);
            };
            let interpreter = shebang(&script).unwrap_or_else(|| "sh".to_owned());
            script_writes(&interpreter, &script, context).map_or(Ok(None), Err)
        }
        Family::Other => Ok(None),
    }
}

/// Records `text` as the content of each file the command's standard output
/// is redirected to.
fn record_outputs(command: &Command, text: &str, context: Context) -> Result<(), Finding> {
    for redirect in &command.redirects {
        if let Redirect::Output {
            stdout: true,
            append,
            target,
        } = redirect
        {
            context.record(target, text, *append)?;
        }
    }
    Ok(())
}

/// The program a script's `#!` line names, past `env` and its options.
fn shebang(script: &str) -> Option<String> {
    let line = script.lines().next()?.strip_prefix("#!")?;
    let mut words = line.split_whitespace();
    let mut program = program_name(words.next()?);
    if program == "env" {
        program = program_name(words.find(|word| !word.starts_with('-'))?);
    }
    Some(program)
}

/// The finding for a script saved at `path` with `text` when it may write
/// project files. Its language comes from its `#!` line, else from the
/// file's extension; a file with neither is not a script.
fn saved_script(path: &str, text: &str, context: Context) -> Option<Finding> {
    let program = shebang(text).or_else(|| {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, extension) = name.rsplit_once('.')?;
        let program = match extension.to_ascii_lowercase().as_str() {
            "py" | "pyw" => "python3",
            "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" => "node",
            "pl" | "pm" => "perl",
            "rb" => "ruby",
            "php" => "php",
            "ps1" => "pwsh",
            "sh" => "sh",
            "bash" => "bash",
            "zsh" => "zsh",
            _ => return None,
        };
        Some(program.to_owned())
    })?;
    script_writes(&program, text, context)
}

/// The finding for a script that `program` runs when it may write project
/// files: an interpreter's write call whose target is not a literal path
/// outside the project, or any write the shell checks report.
fn script_writes(program: &str, script: &str, context: Context) -> Option<Finding> {
    let writes = match family(program) {
        Family::Interpreter(Language::PowerShell) => {
            let foreign = context.scope.foreign();
            context.depth < MAX_SCRIPT_DEPTH
                && powershell::scan(script, &foreign, context.depth + 1).is_some()
        }
        Family::Interpreter(language) => writes_inside(language, script, context.scope),
        Family::Shell | Family::Source => nested(script, context).is_err(),
        _ => false,
    };
    writes.then(|| Finding {
        pattern: Pattern::ScriptFileWrite,
        program: program.to_owned(),
    })
}

/// Text after printf's backslash escapes: `\n`, `\t`, `\\`, quotes, octal
/// `\NNN` or `\0NNN`, and hex `\xHH`. Others stay as written.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let digits = |chars: &mut std::iter::Peekable<std::str::Chars>, radix: u32, most: usize| {
            let mut value = 0;
            let mut count = 0;
            while count < most
                && let Some(digit) = chars.peek().and_then(|next| next.to_digit(radix))
            {
                value = value * radix + digit;
                count += 1;
                chars.next();
            }
            (count > 0).then(|| char::from_u32(value).unwrap_or('\u{fffd}'))
        };
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(quote @ ('\\' | '\'' | '"')) => out.push(quote),
            Some('x') => match digits(&mut chars, 16, 2) {
                Some(decoded) => out.push(decoded),
                None => out.push_str("\\x"),
            },
            Some(first @ '0'..='7') => {
                let rest = if first == '0' { 3 } else { 2 };
                let value = first.to_digit(8).unwrap_or_default();
                let mut decoded = value;
                let mut count = 0;
                while count < rest
                    && let Some(digit) = chars.peek().and_then(|next| next.to_digit(8))
                {
                    decoded = decoded * 8 + digit;
                    count += 1;
                    chars.next();
                }
                out.push(char::from_u32(decoded).unwrap_or('\u{fffd}'));
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A script path compared as text: without `./` segments or doubled slashes.
fn normalized(path: &str) -> String {
    let mut path = path.to_owned();
    while path.contains("//") && !path.starts_with("//") {
        path = path.replace("//", "/");
    }
    while path.contains("/./") {
        path = path.replace("/./", "/");
    }
    path.strip_prefix("./").map(str::to_owned).unwrap_or(path)
}

/// Classifies a Bash script that the command runs, such as `bash -c` text.
fn nested(script: &str, context: Context) -> Result<Option<Content>, Finding> {
    let foreign;
    let scope = match context.dialect {
        Dialect::Bash => context.scope,
        // PowerShell may already have expanded `$` in the script.
        Dialect::PowerShell => {
            foreign = context.scope.foreign();
            &foreign
        }
    };
    match (context.depth < MAX_SCRIPT_DEPTH)
        .then(|| scan(script, scope, context.depth + 1, context.written))
        .flatten()
    {
        Some(finding) => Err(finding),
        None => Ok(None),
    }
}

/// Checks the commands that `find -exec` and its variants run.
fn find_exec(arguments: &[Word], context: Context) -> Result<Option<Content>, Finding> {
    let mut rest = arguments;
    while let Some(start) = rest
        .iter()
        .position(|word| matches!(word.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        let tail = &rest[start + 1..];
        let end = tail
            .iter()
            .position(|word| word.text == ";" || word.text == "+")
            .unwrap_or(tail.len());
        let command = Command {
            words: tail[..end].to_vec(),
            ..Command::default()
        };
        inspect(&command, None, context)?;
        rest = &tail[end..];
    }
    Ok(None)
}

/// Options and leading operands of a command that runs another command.
struct Wrapper {
    /// Options whose value is the next word.
    valued: &'static [&'static str],
    /// Operands before the wrapped command, such as `timeout`'s duration.
    operands: usize,
}

fn wrapper(program: &str) -> Option<Wrapper> {
    let (valued, operands): (&'static [&'static str], usize) = match program {
        "builtin" | "command" | "nohup" => (&[], 0),
        "doas" => (&["-u", "-C"], 0),
        "env" => (
            &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"],
            0,
        ),
        "exec" => (&["-a"], 0),
        "nice" => (&["-n", "--adjustment"], 0),
        "stdbuf" => (&["-i", "-o", "-e", "--input", "--output", "--error"], 0),
        "sudo" => (
            &[
                "-C",
                "-D",
                "-R",
                "-T",
                "-U",
                "-g",
                "-h",
                "-p",
                "-r",
                "-t",
                "-u",
                "--chdir",
                "--chroot",
                "--close-from",
                "--command-timeout",
                "--group",
                "--host",
                "--other-user",
                "--prompt",
                "--role",
                "--type",
                "--user",
            ],
            0,
        ),
        "time" => (&["-f", "-o", "--format", "--output"], 0),
        "timeout" => (&["-k", "-s", "--kill-after", "--signal"], 1),
        "xargs" => (
            &[
                "-E",
                "-I",
                "-L",
                "-P",
                "-a",
                "-d",
                "-n",
                "-s",
                "--arg-file",
                "--delimiter",
                "--max-args",
                "--max-chars",
                "--max-lines",
                "--max-procs",
            ],
            0,
        ),
        _ => return None,
    };
    Some(Wrapper { valued, operands })
}

/// The words from the program that actually runs, past reserved words,
/// assignments, and wrappers such as `sudo` or `env`.
fn resolve(words: &[Word]) -> &[Word] {
    let mut index = 0;
    loop {
        while words
            .get(index)
            .is_some_and(|word| word.assignment || KEYWORDS.contains(&word.text.as_str()))
        {
            index += 1;
        }
        let Some(spec) = words
            .get(index)
            .and_then(|word| wrapper(&program_name(&word.text)))
        else {
            break;
        };
        index += 1;
        while let Some(word) = words.get(index) {
            let text = word.text.as_str();
            if !text.starts_with('-') || text == "-" {
                break;
            }
            index += 1;
            if text == "--" {
                break;
            }
            if spec.valued.contains(&text) {
                index += 1;
            }
        }
        index += spec.operands;
    }
    words.get(index..).unwrap_or_default()
}

/// The command's basename, lowercased, without a Windows `.exe` suffix.
fn program_name(text: &str) -> String {
    let name = text
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(text)
        .to_ascii_lowercase();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => name,
    }
}

/// Whether a redirection or `tee` target names a file rather than a device
/// or a process substitution.
fn is_file(target: &str) -> bool {
    !target.is_empty()
        && (target.starts_with("/dev/shm/")
            || !["/dev/", "/proc/", "/sys/", ">(", "<("]
                .iter()
                .any(|device| target.starts_with(device)))
        && !target.eq_ignore_ascii_case("nul")
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `tee` names an output file that may be in the project.
fn tee_writes(arguments: &[Word], context: Context) -> bool {
    let mut options = true;
    arguments.iter().any(|word| {
        let text = word.text.as_str();
        if options && text.starts_with('-') && text != "-" {
            options = text != "--";
            return false;
        }
        text != "-" && context.counts(word)
    })
}

/// Whether the long option `given` names `option`. GNU getopt accepts any
/// unambiguous prefix, and an ambiguous one fails before the program runs.
fn abbreviates(given: &str, option: &str) -> bool {
    !given.is_empty() && option.starts_with(given)
}

/// The file operands of `sed` when it edits in place (`-i`, `-i.bak`, `-ni`,
/// `--in-place`). Without `-e` or `-f`, the first operand is the script.
fn sed_in_place(arguments: &[Word]) -> Option<Vec<&Word>> {
    let mut in_place = false;
    let mut script = false;
    let mut options = true;
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let text = word.text.as_str();
        if !options || text == "-" || !text.starts_with('-') {
            operands.push(word);
            continue;
        }
        if text == "--" {
            options = false;
            continue;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            let is = |option| abbreviates(name, option);
            in_place |= is("in-place");
            script |= is("expression") || is("file");
            if value.is_none() && (is("expression") || is("file") || is("line-length")) {
                index += 1;
            }
            continue;
        }
        let cluster = &text[1..];
        for (position, flag) in cluster.char_indices() {
            match flag {
                // BSD sed spells in-place editing `-I` as well; the rest of
                // the cluster is a backup suffix.
                'i' | 'I' => {
                    in_place = true;
                    break;
                }
                // The rest of the cluster, or else the next word, is the value.
                'e' | 'f' | 'l' => {
                    script |= flag != 'l';
                    index += usize::from(position + 1 == cluster.len());
                    break;
                }
                _ => {}
            }
        }
    }
    if !script && !operands.is_empty() {
        operands.remove(0);
    }
    in_place.then_some(operands)
}

/// The file operands of `gawk` when it loads its in-place extension
/// (`-i inplace`). Without `-f`, `-e`, or `-E`, the first operand is the
/// program.
fn awk_in_place(arguments: &[Word]) -> Option<Vec<&Word>> {
    let mut in_place = false;
    let mut program = false;
    let mut options = true;
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let text = word.text.as_str();
        if !options || text == "-" || !text.starts_with('-') {
            // Options end at the first operand.
            options = false;
            operands.push(word);
            continue;
        }
        if text == "--" {
            options = false;
            continue;
        }
        // The short flag, or the long option's name, and an attached value.
        let (flag, long, attached) = match text.strip_prefix("--") {
            Some(long) => match long.split_once('=') {
                Some((name, value)) => ("", name, Some(value)),
                None => ("", long, None),
            },
            None => match text.get(1..2).zip(text.get(2..)) {
                Some((flag, rest)) => (flag, "", (!rest.is_empty()).then_some(rest)),
                None => continue,
            },
        };
        let is = |short: &str, option: &str| flag == short || abbreviates(long, option);
        let included = is("i", "include");
        let source = is("f", "file") || is("e", "source") || is("E", "exec");
        program |= source;
        let valued = included
            || source
            || is("v", "assign")
            || is("F", "field-separator")
            || is("l", "load")
            || is("W", "");
        let value = match attached {
            Some(value) => Some(value),
            None if valued => {
                index += 1;
                arguments.get(index - 1).map(|word| word.text.as_str())
            }
            None => None,
        };
        in_place |= included && matches!(value, Some("inplace" | "inplace.awk"));
    }
    if !program && !operands.is_empty() {
        operands.remove(0);
    }
    in_place.then_some(operands)
}

/// The script a shell runs from `-c` or, with no script file, standard input.
/// What a shell runs: inline or standard-input code, or a script file.
enum ShellInput<'w> {
    Code(String),
    File(&'w Word),
}

fn shell_script(arguments: &[Word], stdin: Option<Content>) -> Option<ShellInput<'_>> {
    let mut command = false;
    let mut read_stdin = false;
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        let text = word.text.as_str();
        if text == "--" || text == "-" {
            index += 1;
            break;
        }
        if text.starts_with("--") {
            index += 1;
            if matches!(text, "--init-file" | "--rcfile") {
                index += 1;
            }
            continue;
        }
        let Some(cluster) = text
            .strip_prefix(['-', '+'])
            .filter(|cluster| !cluster.is_empty())
        else {
            break;
        };
        command |= cluster.contains('c');
        read_stdin |= cluster.contains('s');
        // `bash -n` reads the script without running it.
        if text.starts_with('-') && cluster.contains('n') {
            return None;
        }
        index += 1;
        // `-o pipefail` and `-O extglob` take the next word.
        if cluster.contains(['o', 'O']) {
            index += 1;
        }
    }
    if command {
        return arguments
            .get(index)
            .map(|word| ShellInput::Code(word.text.clone()));
    }
    if read_stdin || index >= arguments.len() {
        return stdin.map(|content| ShellInput::Code(content.text));
    }
    arguments.get(index).map(ShellInput::File)
}

/// Whether `patch`, `git apply`, or an `ultra-edit` edit command applies its
/// standard input to files.
fn applies(program: &str, arguments: &[Word]) -> bool {
    let has = |options: &[&str]| {
        arguments
            .iter()
            .any(|word| options.contains(&word.text.as_str()))
    };
    match program {
        "patch" => !has(&["--dry-run", "--check"]),
        "git" => {
            subcommand(
                arguments,
                &["-C", "-c", "--git-dir", "--namespace", "--work-tree"],
            ) == Some("apply")
                && !has(&["--check", "--numstat", "--stat", "--summary"])
        }
        _ => matches!(
            subcommand(arguments, &["--root"]),
            Some("edit" | "prepare" | "repair")
        ),
    }
}

/// The first operand after global options, skipping the values of `valued`.
fn subcommand<'w>(arguments: &'w [Word], valued: &[&str]) -> Option<&'w str> {
    let mut words = arguments.iter().map(|word| word.text.as_str());
    while let Some(word) = words.next() {
        if valued.contains(&word) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

#[derive(Default)]
struct Invocation {
    /// Code passed inline, as with `-c` or `-e`.
    code: Vec<String>,
    /// Whether a script file runs, so standard input is data rather than code.
    script: bool,
    /// Which argument names that script file, when it is one.
    script_at: Option<usize>,
    in_place: bool,
    /// Whether an option such as `perl -c` only checks the syntax.
    check: bool,
    /// Where the operands after the options, code, and any script file start.
    operands: usize,
}

/// Short-option behavior of a Unix-style interpreter.
struct Switches {
    /// Options whose value is code.
    code: &'static str,
    /// Whether inline code ends option parsing.
    code_ends_options: bool,
    /// Options whose value is attached or the next word.
    valued: &'static str,
    /// Options whose value is the rest of the switch cluster.
    attached: &'static str,
    /// Options that run a module or file instead of inline code.
    stops: &'static str,
    /// Whether `-i` edits files in place.
    in_place: bool,
    /// Options that only check the syntax.
    check: &'static str,
    long_code: &'static [&'static str],
    long_valued: &'static [&'static str],
    long_check: &'static [&'static str],
}

const PYTHON: Switches = Switches {
    code: "c",
    code_ends_options: true,
    valued: "XW",
    attached: "",
    stops: "m",
    in_place: false,
    check: "",
    long_code: &[],
    long_valued: &["check-hash-based-pycs"],
    long_check: &[],
};

const NODE: Switches = Switches {
    code: "ep",
    code_ends_options: false,
    valued: "rC",
    attached: "",
    stops: "",
    in_place: false,
    check: "c",
    long_code: &["eval", "print"],
    long_valued: &[
        "conditions",
        "env-file",
        "env-file-if-exists",
        "experimental-loader",
        "import",
        "loader",
        "require",
    ],
    long_check: &["check"],
};

const PERL: Switches = Switches {
    code: "eE",
    code_ends_options: false,
    valued: "I",
    attached: "CDFMVdmx",
    stops: "",
    in_place: true,
    check: "c",
    long_code: &[],
    long_valued: &[],
    long_check: &[],
};

const RUBY: Switches = Switches {
    code: "e",
    code_ends_options: false,
    valued: "CEIr",
    attached: "FKTWx",
    stops: "",
    in_place: true,
    check: "c",
    long_code: &[],
    long_valued: &["encoding", "external-encoding", "internal-encoding"],
    long_check: &[],
};

const PHP: Switches = Switches {
    code: "BERr",
    code_ends_options: false,
    valued: "cdz",
    attached: "",
    stops: "f",
    in_place: false,
    check: "l",
    long_code: &[],
    long_valued: &[],
    long_check: &["syntax-check"],
};

fn invocation(language: Language, program: &str, arguments: &[Word]) -> Invocation {
    let spec = match language {
        Language::Python => &PYTHON,
        Language::Node => &NODE,
        Language::Perl => &PERL,
        Language::Ruby => &RUBY,
        Language::Php => &PHP,
        Language::PowerShell => return powershell(program, arguments),
    };
    let mut invocation = Invocation {
        operands: arguments.len(),
        ..Invocation::default()
    };
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let text = word.text.as_str();
        if text == "-" {
            invocation.operands = index;
            break;
        }
        if text == "--" {
            invocation.script = invocation.code.is_empty() && index < arguments.len();
            invocation.script_at = invocation.script.then_some(index);
            invocation.operands = index + usize::from(invocation.script);
            break;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            if spec.long_code.contains(&name) {
                match value {
                    Some(value) => invocation.code.push(value.to_owned()),
                    None => {
                        invocation
                            .code
                            .extend(arguments.get(index).map(|word| word.text.clone()));
                        index += 1;
                    }
                }
            } else if value.is_none() && spec.long_valued.contains(&name) {
                index += 1;
            }
            invocation.check |= spec.long_check.contains(&name);
            continue;
        }
        let Some(cluster) = text.strip_prefix('-').filter(|cluster| !cluster.is_empty()) else {
            // With inline code, the first operand is data; otherwise it is the script.
            invocation.script = invocation.code.is_empty();
            invocation.script_at = invocation.script.then_some(index - 1);
            invocation.operands = index - usize::from(!invocation.script);
            break;
        };
        for (position, flag) in cluster.char_indices() {
            let rest = &cluster[position + flag.len_utf8()..];
            if spec.in_place && flag == 'i' {
                invocation.in_place = true;
                break;
            }
            if spec.code.contains(flag) {
                if rest.is_empty() {
                    invocation
                        .code
                        .extend(arguments.get(index).map(|word| word.text.clone()));
                    index += 1;
                } else {
                    invocation.code.push(rest.to_owned());
                }
                if spec.code_ends_options {
                    return invocation;
                }
                break;
            }
            invocation.check |= spec.check.contains(flag);
            if spec.stops.contains(flag) {
                invocation.script = true;
                // `php -f FILE` names the script; `python -m` a module.
                if flag == 'f' && rest.is_empty() && index < arguments.len() {
                    invocation.script_at = Some(index);
                }
                return invocation;
            }
            if spec.valued.contains(flag) {
                index += usize::from(rest.is_empty());
                break;
            }
            if spec.attached.contains(flag) {
                break;
            }
        }
    }
    invocation
}

fn powershell(program: &str, arguments: &[Word]) -> Invocation {
    let mut invocation = Invocation::default();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let parameter = word.text.to_ascii_lowercase();
        let Some(name) = parameter.strip_prefix('-') else {
            // Windows PowerShell runs a bare operand as a command; pwsh as a file.
            if program == "powershell" {
                invocation.code.push(join(&arguments[index - 1..]));
            } else {
                invocation.script = true;
                invocation.script_at = Some(index - 1);
            }
            break;
        };
        if matches!(name, "cwa" | "commandwithargs") {
            // Later words are the script's `$args`.
            invocation
                .code
                .extend(arguments.get(index).map(|word| word.text.clone()));
            break;
        }
        if name == "c" || (name.len() >= 3 && "command".starts_with(name)) {
            // `-Command -` reads the commands from standard input.
            let code = join(&arguments[index..]);
            if code != "-" {
                invocation.code.push(code);
            }
            break;
        }
        // A script file is inspected when its text is known; an encoded
        // command is not.
        if name == "f" || (name.len() >= 2 && "file".starts_with(name)) {
            invocation.script = true;
            invocation.script_at = Some(index).filter(|&at| at < arguments.len());
            break;
        }
        if matches!(name, "e" | "ec") || (name.len() >= 2 && "encodedcommand".starts_with(name)) {
            invocation.script = true;
            break;
        }
        if POWERSHELL_VALUED.contains(&name) {
            index += 1;
        }
    }
    invocation
}

/// Whether inline code calls a file-write API of its language.
fn writes(language: Language, code: &str) -> bool {
    !write_targets(language, code).is_empty()
}

/// Whether script code may write a project file: a write call's target is not
/// a string literal, or names a path that is not certainly outside.
fn writes_inside(language: Language, code: &str, scope: &Scope) -> bool {
    write_targets(language, code).iter().any(|target| {
        target
            .as_deref()
            .is_none_or(|path| scope.literal_inside(path))
    })
}

/// The file-write calls in code, each with its target when that is a string
/// literal: `open('out.txt', 'w')`, `Path('out.txt').write_text(...)`,
/// `fs.writeFileSync('out.txt', ...)`, `open(my $f, '>', 'out.txt')`, and so on.
fn write_targets(language: Language, code: &str) -> Vec<Option<String>> {
    match language {
        Language::Python => {
            let mut targets: Vec<_> = ["write_text(", "write_bytes("]
                .iter()
                .flat_map(|name| calls(code, name).map(|at| receiver_path(&code[..at])))
                .collect();
            for at in calls(code, "open(") {
                // `open(path, mode)` takes the path first; `Path.open(mode)` does not.
                let receiver = code[..at].strip_suffix('.').map(|before| {
                    before
                        .rsplit(|character: char| {
                            !(character.is_alphanumeric() || character == '_')
                        })
                        .next()
                        .unwrap_or_default()
                });
                let skip = usize::from(matches!(
                    receiver,
                    None | Some("bz2" | "codecs" | "gzip" | "io" | "lzma" | "os")
                ));
                let call = &code[at + "open".len()..];
                if call_writes(call, skip, "+Uabrtwx") {
                    targets.push(if skip == 1 {
                        first_literal(call)
                    } else {
                        receiver_path(&code[..at])
                    });
                }
            }
            targets
        }
        Language::Node => ["writeFile", "appendFile", "createWriteStream"]
            .iter()
            .flat_map(|name| {
                calls(code, name).map(move |at| {
                    let rest = &code[at + name.len()..];
                    let call =
                        rest.trim_start_matches(|character: char| character.is_alphanumeric());
                    first_literal(call)
                })
            })
            .collect(),
        Language::Perl => calls(code, "open")
            .filter_map(|at| {
                let rest = &code[at + "open".len()..];
                if !rest
                    .starts_with(|character: char| character == '(' || character.is_whitespace())
                {
                    return None;
                }
                let statement = prefix(rest, MAX_CALL_SPAN);
                let statement = statement.split(';').next().unwrap_or_default();
                let arguments = arguments(statement.trim_start());
                let (index, mode) =
                    arguments.iter().enumerate().find_map(|(index, argument)| {
                        let mode = leading_literal(argument)?.trim_start();
                        (mode.starts_with('>') || mode.starts_with("+<") || mode.starts_with("+>"))
                            .then_some((index, mode))
                    })?;
                // Two arguments put the path after the mode: `open(F, '>out.txt')`.
                let path = mode.trim_start_matches(['+', '<', '>']).trim();
                Some(if path.is_empty() || path.starts_with(':') {
                    arguments
                        .get(index + 1)
                        .and_then(|argument| whole_literal(argument))
                } else {
                    Some(path.to_owned())
                })
            })
            .collect(),
        Language::Ruby => {
            let mut targets: Vec<_> = ["File.write", "File.binwrite", "IO.write", "IO.binwrite"]
                .iter()
                .flat_map(|name| {
                    calls(code, name).map(move |at| {
                        first_literal(code[at + name.len()..].trim_start_matches(' '))
                    })
                })
                .collect();
            for name in ["File.open", "File.new"] {
                for at in calls(code, name) {
                    let call = code[at + name.len()..].trim_start_matches(' ');
                    if call_writes(call, 1, "+abrtwx") {
                        targets.push(first_literal(call));
                    }
                }
            }
            targets
        }
        Language::Php => {
            let mut targets: Vec<_> = calls(code, "file_put_contents")
                .map(|at| first_literal(&code[at + "file_put_contents".len()..]))
                .collect();
            for at in calls(code, "fopen(") {
                let call = &code[at + "fopen".len()..];
                if call_writes(call, 1, "+abcertwx") {
                    targets.push(first_literal(call));
                }
            }
            targets
        }
        // `inspect` classifies PowerShell code as a script of its own.
        Language::PowerShell => Vec::new(),
    }
}

/// The path literal that a call's first argument is, when it is nothing else.
fn first_literal(call: &str) -> Option<String> {
    arguments(call)
        .first()
        .and_then(|argument| whole_literal(argument))
}

/// The path of a `Path('...')` receiver just before a method call's `.`.
fn receiver_path(before: &str) -> Option<String> {
    let call = before.strip_suffix('.')?.trim_end().strip_suffix(')')?;
    let open = call.rfind("Path(")?;
    whole_literal(&call[open + "Path(".len()..])
}

/// The body of `argument` when it is a single string literal, with escaped
/// backslashes undone so a Windows path reads as the path.
fn whole_literal(argument: &str) -> Option<String> {
    let argument = argument.trim();
    let body = leading_literal(argument)?;
    // At most two prefix letters and the two quotes surround the body.
    (argument.len() <= body.len() + 4 && !body.contains(['{', '$', '#']))
        .then(|| body.replace("\\\\", "\\"))
}

/// Offsets where `name` occurs in `code` without an identifier character
/// immediately before it.
fn calls<'c>(code: &'c str, name: &'c str) -> impl Iterator<Item = usize> + 'c {
    code.match_indices(name)
        .map(|(at, _)| at)
        .filter(move |&at| {
            !code[..at].ends_with(|character: char| {
                character.is_alphanumeric() || character == '_' || character == '$'
            })
        })
}

/// Whether the call whose arguments start `call` passes a writing file mode
/// after its first `skip` arguments or as a `mode` keyword.
fn call_writes(call: &str, skip: usize, letters: &str) -> bool {
    arguments(call).iter().enumerate().any(|(index, argument)| {
        let argument = argument.trim_start();
        match argument
            .strip_prefix("mode")
            .and_then(|rest| rest.trim_start().strip_prefix(['=', ':']))
        {
            Some(value) => write_mode(value, letters),
            None => index >= skip && write_mode(argument, letters),
        }
    })
}

/// Top-level arguments of a call: within its parentheses, or else to the end
/// of the line as in Ruby's `File.open path, "w"`.
fn arguments(call: &str) -> Vec<&str> {
    let call = prefix(call, MAX_CALL_SPAN);
    let bytes = call.as_bytes();
    let parenthesized = bytes.first() == Some(&b'(');
    let mut arguments = Vec::new();
    let mut depth = 0_usize;
    let mut start = usize::from(parenthesized);
    let mut index = start;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' => {
                index += 1;
                while let Some(&inner) = bytes.get(index) {
                    if inner == byte {
                        break;
                    }
                    index += if inner == b'\\' { 2 } else { 1 };
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b')' | b']' | b'}' => {
                arguments.push(&call[start..index]);
                return arguments;
            }
            b'\n' | b';' if !parenthesized && depth == 0 => {
                arguments.push(&call[start..index]);
                return arguments;
            }
            b',' if depth == 0 => {
                arguments.push(&call[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if let Some(rest) = call.get(start..) {
        arguments.push(rest);
    }
    arguments
}

/// Whether `argument` starts with a string literal naming a writing file mode.
fn write_mode(argument: &str, letters: &str) -> bool {
    leading_literal(argument).is_some_and(|literal| {
        // Ruby and `tarfile` append `:encoding` or `:compression`.
        let mode = literal.split(':').next().unwrap_or_default();
        (1..=4).contains(&mode.len())
            && mode.chars().all(|character| letters.contains(character))
            && mode.contains(['+', 'a', 'c', 'w', 'x'])
    })
}

/// The body of the string literal that starts `text`, after at most two
/// prefix letters such as Python's `rb`.
fn leading_literal(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let body = text.trim_start_matches(|character: char| character.is_ascii_alphabetic());
    if text.len() - body.len() > 2 {
        return None;
    }
    let quote = body
        .chars()
        .next()
        .filter(|&character| character == '\'' || character == '"')?;
    let body = &body[1..];
    body.find(quote).map(|end| &body[..end])
}

/// At most `limit` bytes of `text`, ending on a character boundary.
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
