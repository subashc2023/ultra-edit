//! Classifies Bash commands for the Claude Code `PreToolUse` guard.
//!
//! A command is denied when its text carries file content or edit logic that
//! the shell writes into files: heredoc and `echo` redirection, inline
//! interpreter code that calls a file-write API, and in-place editors.
//! Anything the lexer cannot place with confidence is allowed, because a false
//! positive blocks legitimate work.

use std::fmt;

/// Setting this environment variable to `allow` disables the guard.
pub const ESCAPE_HATCH: &str = "ULTRA_EDIT_SHELL_WRITES";

/// How deep `bash -c`, `eval`, and backtick scripts are classified.
const MAX_SCRIPT_DEPTH: usize = 3;
/// How deep command substitutions are parsed before the rest is skipped.
const MAX_NESTING: usize = 64;
/// Bytes of interpreter code examined for each candidate write call.
const MAX_CALL_SPAN: usize = 512;
/// Program names longer than this are shortened in the deny reason.
const MAX_PROGRAM_CHARS: usize = 16;

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
    format!(
        "Ultra Edit guard blocked {finding}: shell payloads can lose backslashes. Edit existing \
         files with ultra_edit or native Edit, create files with Write, and put multiline command \
         input in a file created with Write. If the user explicitly asked for this command, \
         report that the Ultra Edit guard blocked it; {ESCAPE_HATCH}=allow disables the guard."
    )
}

/// Returns the first shell file write in a Bash tool command, if any.
pub fn classify(command: &str) -> Option<Finding> {
    scan(&command.replace("\r\n", "\n"), 0)
}

fn scan(script: &str, depth: usize) -> Option<Finding> {
    let mut lexer = Lexer::new(script);
    let commands = lexer.list(false);
    check(&commands, depth)
        .or_else(|| {
            lexer
                .nested
                .iter()
                .find_map(|commands| check(commands, depth))
        })
        .or_else(|| {
            (depth < MAX_SCRIPT_DEPTH)
                .then(|| {
                    lexer
                        .backticks
                        .iter()
                        .find_map(|body| scan(body, depth + 1))
                })
                .flatten()
        })
}

#[derive(Clone, Debug, Default)]
struct Word {
    /// The value after quote removal. Expansions and substitutions stay
    /// verbatim, so inline code keeps the text a substitution would read.
    text: String,
    /// Whether the word starts with an unquoted `NAME=` assignment.
    assignment: bool,
}

#[derive(Debug)]
enum Redirect {
    /// Output to a path; `stdout` is false for other descriptors.
    Output { stdout: bool, target: String },
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
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            at: 0,
            nested: Vec::new(),
            backticks: Vec::new(),
            nesting: 0,
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
        let stdout = matches!(descriptor, None | Some(1));
        let stdin = matches!(descriptor, None | Some(0));
        let redirect = match (self.peek(0), self.peek(1), self.peek(2)) {
            (Some(b'&'), _, next) => {
                self.at += if next == Some(b'>') { 3 } else { 2 };
                Redirect::Output {
                    stdout: true,
                    target: self.target().text,
                }
            }
            (Some(b'>'), Some(b'&'), _) => {
                self.at += 2;
                let target = self.target().text;
                if target.starts_with('$')
                    || target
                        .trim_end_matches('-')
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
                {
                    Redirect::Other
                } else {
                    Redirect::Output { stdout, target }
                }
            }
            (Some(b'>'), Some(b'>' | b'|'), _) => {
                self.at += 2;
                Redirect::Output {
                    stdout,
                    target: self.target().text,
                }
            }
            (Some(b'>'), _, _) => {
                self.at += 1;
                Redirect::Output {
                    stdout,
                    target: self.target().text,
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
        Word { text, assignment }
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
struct Content {
    text: String,
    pattern: Pattern,
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

fn check(commands: &[Command], depth: usize) -> Option<Finding> {
    let mut piped = None;
    for command in commands {
        match inspect(command, piped.take(), depth) {
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
    depth: usize,
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
    let to_file = command.redirects.iter().any(
        |redirect| matches!(redirect, Redirect::Output { stdout: true, target } if is_file(target)),
    );
    let found = |pattern| {
        Err(Finding {
            pattern,
            program: program.clone(),
        })
    };
    match family(&program) {
        Family::Generator => {
            let content = Content {
                text: join(arguments),
                pattern: Pattern::GeneratedToFile,
                origin: program.clone(),
            };
            if to_file {
                Err(content.finding())
            } else {
                Ok(Some(content))
            }
        }
        Family::Tee => match stdin {
            Some(content) if tee_writes(arguments) => Err(content.finding()),
            stdin => Ok(stdin),
        },
        Family::Sed if sed_in_place(arguments) => found(Pattern::InPlaceEdit),
        Family::Awk if awk_in_place(arguments) => found(Pattern::InPlaceEdit),
        Family::Filter | Family::Sed | Family::Awk => match stdin {
            Some(content) if to_file => Err(content.finding()),
            stdin => Ok(stdin),
        },
        Family::Interpreter(language) => {
            let invocation = invocation(language, &program, arguments);
            if invocation.in_place {
                return found(Pattern::InPlaceEdit);
            }
            // Without inline code or a script file, standard input is the program.
            let code = if !invocation.code.is_empty() {
                invocation.code.join("\n")
            } else if invocation.script {
                String::new()
            } else {
                stdin.map(|content| content.text).unwrap_or_default()
            };
            if writes(language, &code) {
                found(Pattern::InlineScriptWrite)
            } else {
                Ok(None)
            }
        }
        Family::Shell => match shell_script(arguments, stdin) {
            Some(script) => nested(&script, depth),
            None => Ok(None),
        },
        Family::Eval => nested(&join(arguments), depth),
        Family::Find => find_exec(arguments, depth),
        Family::Applier if stdin.is_some() && applies(&program, arguments) => {
            found(Pattern::AppliedContent)
        }
        Family::Applier | Family::Other => Ok(None),
    }
}

/// Classifies a script that the command runs, such as `bash -c` text.
fn nested(script: &str, depth: usize) -> Result<Option<Content>, Finding> {
    match (depth < MAX_SCRIPT_DEPTH)
        .then(|| scan(script, depth + 1))
        .flatten()
    {
        Some(finding) => Err(finding),
        None => Ok(None),
    }
}

/// Checks the commands that `find -exec` and its variants run.
fn find_exec(arguments: &[Word], depth: usize) -> Result<Option<Content>, Finding> {
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
        inspect(&command, None, depth)?;
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
        && !["/dev/", "/proc/", "/sys/", ">(", "<("]
            .iter()
            .any(|device| target.starts_with(device))
        && !target.eq_ignore_ascii_case("nul")
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `tee` names an output file other than a device.
fn tee_writes(arguments: &[Word]) -> bool {
    let mut options = true;
    arguments.iter().any(|word| {
        let text = word.text.as_str();
        if options && text.starts_with('-') && text != "-" {
            options = text != "--";
            return false;
        }
        text != "-" && is_file(text)
    })
}

/// Whether `sed` edits files in place (`-i`, `-i.bak`, `-ni`, `--in-place`).
fn sed_in_place(arguments: &[Word]) -> bool {
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let text = word.text.as_str();
        if text == "--" {
            break;
        }
        if let Some(long) = text.strip_prefix("--") {
            if long == "in-place" || long.starts_with("in-place=") {
                return true;
            }
            if matches!(long, "expression" | "file" | "line-length") {
                index += 1;
            }
            continue;
        }
        let Some(cluster) = text.strip_prefix('-') else {
            continue;
        };
        for (position, flag) in cluster.char_indices() {
            match flag {
                // BSD sed spells in-place editing `-I` as well.
                'i' | 'I' => return true,
                // The rest of the cluster, or else the next word, is the value.
                'e' | 'f' | 'l' => {
                    index += usize::from(position + 1 == cluster.len());
                    break;
                }
                _ => {}
            }
        }
    }
    false
}

/// Whether `gawk` loads its in-place extension (`-i inplace`).
fn awk_in_place(arguments: &[Word]) -> bool {
    arguments.iter().enumerate().any(|(index, word)| {
        let text = word.text.as_str();
        let library = match text {
            "-i" | "--include" => arguments.get(index + 1).map(|word| word.text.as_str()),
            _ => text
                .strip_prefix("--include=")
                .or_else(|| text.strip_prefix("-i")),
        };
        library.is_some_and(|library| library == "inplace" || library == "inplace.awk")
    })
}

/// The script a shell runs from `-c` or, with no script file, standard input.
fn shell_script(arguments: &[Word], stdin: Option<Content>) -> Option<String> {
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
        index += 1;
        // `-o pipefail` and `-O extglob` take the next word.
        if cluster.contains(['o', 'O']) {
            index += 1;
        }
    }
    if command {
        return arguments.get(index).map(|word| word.text.clone());
    }
    if read_stdin || index >= arguments.len() {
        return stdin.map(|content| content.text);
    }
    None
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
    in_place: bool,
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
    long_code: &'static [&'static str],
    long_valued: &'static [&'static str],
}

const PYTHON: Switches = Switches {
    code: "c",
    code_ends_options: true,
    valued: "XW",
    attached: "",
    stops: "m",
    in_place: false,
    long_code: &[],
    long_valued: &["check-hash-based-pycs"],
};

const NODE: Switches = Switches {
    code: "ep",
    code_ends_options: false,
    valued: "rC",
    attached: "",
    stops: "",
    in_place: false,
    long_code: &["eval", "print"],
    long_valued: &[
        "conditions",
        "experimental-loader",
        "import",
        "loader",
        "require",
    ],
};

const PERL: Switches = Switches {
    code: "eE",
    code_ends_options: false,
    valued: "I",
    attached: "CDFMVdmx",
    stops: "",
    in_place: true,
    long_code: &[],
    long_valued: &[],
};

const RUBY: Switches = Switches {
    code: "e",
    code_ends_options: false,
    valued: "CEIr",
    attached: "FKTWx",
    stops: "",
    in_place: true,
    long_code: &[],
    long_valued: &["encoding", "external-encoding", "internal-encoding"],
};

const PHP: Switches = Switches {
    code: "BERr",
    code_ends_options: false,
    valued: "cdz",
    attached: "",
    stops: "f",
    in_place: false,
    long_code: &[],
    long_valued: &[],
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
    let mut invocation = Invocation::default();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        index += 1;
        let text = word.text.as_str();
        if text == "-" {
            break;
        }
        if text == "--" {
            invocation.script = index < arguments.len();
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
            continue;
        }
        let Some(cluster) = text.strip_prefix('-').filter(|cluster| !cluster.is_empty()) else {
            invocation.script = true;
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
            if spec.stops.contains(flag) {
                invocation.script = true;
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
            }
            break;
        };
        if matches!(name, "c" | "cwa" | "commandwithargs")
            || (name.len() >= 3 && "command".starts_with(name))
        {
            // `-Command -` reads the commands from standard input.
            let code = join(&arguments[index..]);
            if code != "-" {
                invocation.code.push(code);
            }
            break;
        }
        // Script files and encoded commands are not inspected.
        if matches!(name, "f" | "e" | "ec")
            || (name.len() >= 2 && ("file".starts_with(name) || "encodedcommand".starts_with(name)))
        {
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
    match language {
        Language::Python => {
            calls(code, "write_text(").next().is_some()
                || calls(code, "write_bytes(").next().is_some()
                || calls(code, "open(").any(|at| {
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
                    call_writes(&code[at + "open".len()..], skip, "+Uabrtwx")
                })
        }
        Language::Node => ["writeFile", "appendFile", "createWriteStream"]
            .iter()
            .any(|name| calls(code, name).next().is_some()),
        Language::Perl => calls(code, "open").any(|at| {
            let rest = &code[at + "open".len()..];
            if !rest.starts_with(|character: char| character == '(' || character.is_whitespace()) {
                return false;
            }
            let statement = prefix(rest, MAX_CALL_SPAN);
            let statement = statement.split(';').next().unwrap_or_default();
            statement.match_indices(['\'', '"']).any(|(quote, _)| {
                let mode = statement[quote + 1..].trim_start();
                mode.starts_with('>') || mode.starts_with("+<") || mode.starts_with("+>")
            })
        }),
        Language::Ruby => {
            ["File.write", "File.binwrite", "IO.write", "IO.binwrite"]
                .iter()
                .any(|name| calls(code, name).next().is_some())
                || ["File.open", "File.new"].iter().any(|name| {
                    calls(code, name).any(|at| {
                        call_writes(
                            code[at + name.len()..].trim_start_matches(' '),
                            1,
                            "+abrtwx",
                        )
                    })
                })
        }
        Language::Php => {
            calls(code, "file_put_contents").next().is_some()
                || calls(code, "fopen(")
                    .any(|at| call_writes(&code[at + "fopen".len()..], 1, "+abcertwx"))
        }
        Language::PowerShell => {
            let code = code.to_ascii_lowercase();
            [
                "set-content",
                "add-content",
                "out-file",
                "io.file]::write",
                "io.file]::append",
            ]
            .iter()
            .any(|name| code.contains(name))
        }
    }
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
