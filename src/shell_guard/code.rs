//! Finds the file-write calls in interpreter code and decides whether each
//! target lies certainly outside the project.
//!
//! Code is read as text, not parsed. A target counts as outside when it
//! resolves to a path, or to a leading directory, outside the project:
//! through string literals, f-strings and template literals, `+` and path
//! joins, names bound once, and the temporary-directory, home-directory, and
//! standard-stream APIs. Anything else, such as a command-line argument, a
//! loop variable, or a function parameter, counts as inside.

use std::cell::Cell;

use super::{Language, Scope, is_absolute};

/// Bytes of code examined for each candidate write call or bound value.
const MAX_CALL_SPAN: usize = 512;
/// How many names, operands, and calls deep a target is resolved.
const MAX_DEPTH: usize = 8;
/// Work spent resolving one script's targets, counting a step per value and
/// a step per byte searched for a name's bindings; past it, targets count as
/// inside.
const MAX_WORK: usize = 1 << 23;

/// Python modules whose `open` takes a path first, as the builtin does.
const PATH_OPENERS: &[&str] = &[
    "aifc", "bz2", "codecs", "dbm", "gzip", "io", "lzma", "os", "shelve", "sunau", "tarfile",
    "wave",
];

/// Calls that join their arguments into one path.
const JOINS: &[&str] = &[
    "File.join",
    "Path",
    "Pathname",
    "Pathname.new",
    "PosixPath",
    "PurePath",
    "PurePosixPath",
    "PureWindowsPath",
    "WindowsPath",
    "ntpath.join",
    "os.path.join",
    "path.join",
    "path.posix.join",
    "path.resolve",
    "path.win32.join",
    "pathlib.Path",
    "posixpath.join",
];

/// Calls that return their first argument as a path in another form.
const WRAPPERS: &[&str] = &[
    "File.expand_path",
    "String",
    "os.fspath",
    "os.path.abspath",
    "os.path.expanduser",
    "os.path.normpath",
    "os.path.realpath",
    "path.normalize",
    "realpath",
    "str",
];

/// Methods that return their receiver path in another form.
const METHODS: &[&str] = &[
    ".absolute()",
    ".as_posix()",
    ".expanduser()",
    ".resolve()",
    ".toString()",
];

/// Calls that make a new file or directory in the temporary directory,
/// unless a `dir` argument puts it elsewhere.
const FRESH: &[&str] = &[
    "Dir.mktmpdir",
    "mkdtemp(",
    "tempfile.NamedTemporaryFile(",
    "tempfile.TemporaryDirectory(",
    "tempfile.mkdtemp(",
    "tempfile.mkstemp(",
    "tmp_path_factory.mktemp(",
    "tmpdir_factory.mktemp(",
];

/// Calls that make a new file or directory whose path starts with their
/// first argument: Node's `mkdtemp` and PHP's `tempnam`.
const PREFIXED: &[&str] = &[
    "fs.mkdtempSync",
    "fs.promises.mkdtemp",
    "fsp.mkdtemp",
    "mkdtempSync",
    "tempnam",
];

/// Expressions for the temporary directory.
const TEMPORARY: &[&str] = &[
    "Dir.tmpdir",
    "gettempdir()",
    "os.tmpdir()",
    "sys_get_temp_dir()",
    "tempfile.gettempdir()",
    "tmpdir()",
];

/// Expressions for the home directory.
const HOMES: &[&str] = &[
    "$ENV{HOME}",
    "Dir.home",
    "ENV[\"HOME\"]",
    "ENV['HOME']",
    "Path.home()",
    "getenv(\"HOME\")",
    "getenv('HOME')",
    "homedir()",
    "os.environ.get(\"HOME\")",
    "os.environ.get('HOME')",
    "os.environ[\"HOME\"]",
    "os.environ['HOME']",
    "os.getenv(\"HOME\")",
    "os.getenv('HOME')",
    "os.homedir()",
    "pathlib.Path.home()",
    "process.env.HOME",
];

/// Standard streams and other targets that are no file.
const DEVICES: &[&str] = &[
    "$stderr",
    "$stdout",
    "STDERR",
    "STDOUT",
    "os.devnull",
    "process.stderr",
    "process.stderr.fd",
    "process.stdout",
    "process.stdout.fd",
    "sys.stderr",
    "sys.stdout",
];

/// Pytest fixtures that are new temporary directories.
const FIXTURES: &[&str] = &["tmp_path", "tmpdir"];

/// Words that open a line binding a name other than by assignment: a loop,
/// a function's parameters, an exception, or an import.
const REBINDERS: &[&str] = &[
    "async", "catch", "class", "def", "except", "for", "foreach", "from", "function", "import",
    "lambda", "sub",
];

/// Whether code calls a file-write API on a target that may be inside the
/// project.
pub(super) fn writes_inside(language: Language, code: &str, scope: &Scope) -> bool {
    let code = without_comments(language, code);
    let resolver = Resolver {
        language,
        code: &code,
        scope,
        work: Cell::new(MAX_WORK),
    };
    write_targets(language, &code)
        .into_iter()
        .any(|target| !resolver.outside(target))
}

/// What a write call writes to.
enum Target<'c> {
    /// The expression that names the file.
    Expression(&'c str),
    /// A path spelled within a mode, as in Perl's `open(F, '>out.txt')`.
    Spelled(String),
    /// A target the call does not show.
    Unknown,
}

/// Code without whole-line comments, so code a comment mentions is not
/// taken for a call. String contents that look like comments go too, which
/// removes no call.
fn without_comments(language: Language, code: &str) -> String {
    let markers: &[&str] = match language {
        Language::Node => &["//", "/*", "*"],
        Language::Php => &["#", "//", "/*", "*"],
        _ => &["#"],
    };
    code.split_inclusive('\n')
        .map(|line| {
            let comment = markers
                .iter()
                .any(|marker| line.trim_start().starts_with(marker));
            if comment { "\n" } else { line }
        })
        .collect()
}

/// The file-write calls in code and what each writes: `open('out.txt',
/// 'w')`, `Path(out).write_text(...)`, `fs.writeFileSync(out, ...)`, `open(my
/// $f, '>', $out)`, `shutil.move(src, out)`, and so on.
fn write_targets(language: Language, code: &str) -> Vec<Target<'_>> {
    let mut targets = Vec::new();
    match language {
        Language::Python => {
            for name in ["write_text(", "write_bytes("] {
                for at in calls(code, name) {
                    targets.push(receiver(&code[..at]).map_or(Target::Unknown, Target::Expression));
                }
            }
            for at in calls(code, "open(") {
                let before = &code[..at];
                let call = &code[at + "open".len()..];
                // `open(path, mode)` and `gzip.open(path, mode)` take the path
                // first; `Path(...).open(mode)` writes to its receiver.
                let method = before.ends_with('.').then(|| receiver(before));
                let path_first = match method {
                    None => true,
                    Some(receiver) => receiver.is_some_and(|name| PATH_OPENERS.contains(&name)),
                };
                if call_writes(call, usize::from(path_first), "+Uabrtwx") {
                    targets.push(match method {
                        Some(receiver) if !path_first => {
                            receiver.map_or(Target::Unknown, Target::Expression)
                        }
                        _ => argument(call, 0),
                    });
                }
            }
            let moves = [
                "os.rename",
                "os.renames",
                "os.replace",
                "shutil.copy",
                "shutil.copy2",
                "shutil.copyfile",
                "shutil.copytree",
                "shutil.move",
            ];
            targets.extend(named(language, code, &moves.map(|name| (name, 1))));
        }
        Language::Node => {
            targets.extend(named(
                language,
                code,
                &[
                    ("appendFile", 0),
                    ("appendFileSync", 0),
                    ("copyFileSync", 1),
                    ("cpSync", 1),
                    ("createWriteStream", 0),
                    ("fs.copyFile", 1),
                    ("fs.cp", 1),
                    ("fs.rename", 1),
                    ("fsp.copyFile", 1),
                    ("fsp.cp", 1),
                    ("fsp.rename", 1),
                    ("promises.copyFile", 1),
                    ("promises.cp", 1),
                    ("promises.rename", 1),
                    ("renameSync", 1),
                    ("writeFile", 0),
                    ("writeFileSync", 0),
                ],
            ));
        }
        Language::Perl => {
            targets.extend(calls(code, "open").filter_map(|at| {
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
                        .map_or(Target::Unknown, |argument| {
                            Target::Expression(argument.trim())
                        })
                } else {
                    Target::Spelled(path.to_owned())
                })
            }));
            targets.extend(named(language, code, &[("rename", 1)]));
        }
        Language::Ruby => {
            targets.extend(named(
                language,
                code,
                &[
                    ("File.binwrite", 0),
                    ("File.rename", 1),
                    ("File.write", 0),
                    ("FileUtils.copy", 1),
                    ("FileUtils.cp", 1),
                    ("FileUtils.cp_r", 1),
                    ("FileUtils.install", 1),
                    ("FileUtils.move", 1),
                    ("FileUtils.mv", 1),
                    ("IO.binwrite", 0),
                    ("IO.write", 0),
                ],
            ));
            for name in ["File.open", "File.new"] {
                for at in calls(code, name) {
                    let call = code[at + name.len()..].trim_start_matches(' ');
                    if call_writes(call, 1, "+abrtwx") {
                        targets.push(argument(call, 0));
                    }
                }
            }
        }
        Language::Php => {
            targets.extend(named(
                language,
                code,
                &[("copy", 1), ("file_put_contents", 0), ("rename", 1)],
            ));
            for at in calls(code, "fopen(") {
                let call = &code[at + "fopen".len()..];
                if call_writes(call, 1, "+abcertwx") {
                    targets.push(argument(call, 0));
                }
            }
        }
        // `inspect` classifies PowerShell code as a script of its own.
        Language::PowerShell => {}
    }
    targets
}

/// The targets of calls to `names`, each the argument at its index. A call
/// opens its parentheses right after the name or, in Ruby and Perl, passes
/// its arguments after a space.
fn named<'c>(language: Language, code: &'c str, names: &[(&str, usize)]) -> Vec<Target<'c>> {
    let mut targets = Vec::new();
    for &(name, index) in names {
        for at in calls(code, name) {
            let rest = &code[at + name.len()..];
            let spaced = rest.trim_start_matches([' ', '\t']);
            let call = spaced.starts_with('(')
                || (matches!(language, Language::Ruby | Language::Perl)
                    && rest.starts_with([' ', '\t'])
                    && !spaced.starts_with(['=', '.', ',', ')', '\n', ';']));
            if call {
                targets.push(argument(spaced, index));
            }
        }
    }
    targets
}

/// The argument at `index` of the call whose arguments start `call`.
fn argument(call: &str, index: usize) -> Target<'_> {
    arguments(call)
        .get(index)
        .map(|argument| argument.trim())
        .filter(|argument| !argument.is_empty())
        .map_or(Target::Unknown, Target::Expression)
}

/// What a target expression names.
#[derive(Debug)]
enum Value {
    /// A path whose text is fully known.
    Path(String),
    /// A path whose leading text is known.
    Prefix(String),
    /// A new temporary file or directory, or a path within one.
    Fresh,
    /// A stream or descriptor rather than a file.
    Device,
    Unknown,
}

/// How an operand joins the path before it.
#[derive(Clone, Copy)]
enum Join {
    /// String concatenation.
    Concat,
    /// A path join, which puts a separator between the parts.
    Separator,
}

struct Resolver<'c> {
    language: Language,
    code: &'c str,
    scope: &'c Scope,
    /// Work left before every target counts as inside.
    work: Cell<usize>,
}

impl<'c> Resolver<'c> {
    /// Whether a write call's target lies certainly outside the project.
    fn outside(&self, target: Target<'c>) -> bool {
        let value = match target {
            Target::Expression(expression) => self.value(expression, 0),
            Target::Spelled(path) => self.spelled(&path),
            Target::Unknown => Value::Unknown,
        };
        match value {
            Value::Path(path) => !self.scope.literal_inside(&path),
            Value::Prefix(prefix) => self.scope.prefix_outside(&prefix),
            Value::Fresh | Value::Device => true,
            Value::Unknown => false,
        }
    }

    /// A path Perl spells within its mode: `&STDOUT` and `-` are streams.
    fn spelled(&self, path: &str) -> Value {
        if path.starts_with('&') || path == "-" {
            return Value::Device;
        }
        let cut = path.find(['$', '@', '{']).unwrap_or(path.len());
        if cut == path.len() {
            Value::Path(path.to_owned())
        } else {
            Value::Prefix(path[..cut].to_owned())
        }
    }

    /// Takes `cost` from the work left, or reports that too little is left.
    fn spend(&self, cost: usize) -> bool {
        let left = self.work.get();
        self.work.set(left.saturating_sub(cost));
        left >= cost
    }

    fn value(&self, expression: &'c str, depth: usize) -> Value {
        let expression = unwrapped(expression);
        if expression.is_empty() || depth > MAX_DEPTH || !self.spend(1) {
            return Value::Unknown;
        }
        // Perl's `\$buffer` and `\*STDOUT` are in-memory or stream handles.
        if is_device(expression)
            || (matches!(self.language, Language::Perl) && expression.starts_with('\\'))
        {
            return Value::Device;
        }
        if self.branches(expression) {
            return Value::Unknown;
        }
        let operands = self.operands(expression);
        if operands.len() > 1 {
            return self.fold(operands, depth);
        }
        self.literal(expression)
            .or_else(|| self.call(expression, depth))
            .or_else(|| self.name(expression, depth))
            .unwrap_or(Value::Unknown)
    }

    /// Joins the values of `parts`, each with how it joins the ones before.
    fn fold(&self, parts: Vec<(Join, &'c str)>, depth: usize) -> Value {
        let mut parts = parts.into_iter();
        let Some((_, first)) = parts.next() else {
            return Value::Unknown;
        };
        let mut value = self.value(first, depth + 1);
        for (join, part) in parts {
            value = match (value, self.value(part, depth + 1)) {
                (Value::Path(base), Value::Path(part)) => Value::Path(joined(&base, join, &part)),
                (Value::Path(base), Value::Prefix(part)) => {
                    Value::Prefix(joined(&base, join, &part))
                }
                (Value::Path(base), _) => Value::Prefix(joined(&base, join, "")),
                (Value::Prefix(base), _) => Value::Prefix(base),
                (Value::Fresh, _) => Value::Fresh,
                _ => Value::Unknown,
            };
        }
        value
    }

    /// Whether an expression picks between values: a conditional or a
    /// logical operator at its top level.
    fn branches(&self, expression: &str) -> bool {
        let words: &[&str] = match self.language {
            Language::Python => &[" if ", " or ", " and "],
            Language::Ruby | Language::Perl => &[" if ", " or ", " and ", " unless "],
            _ => &[],
        };
        surface(expression).into_iter().any(|at| {
            let rest = &expression[at..];
            rest.starts_with(['?', '|'])
                || rest.starts_with("&&")
                || words.iter().any(|word| rest.starts_with(word))
        })
    }

    /// The operands of top-level concatenation and path joins: `a + b` in
    /// Python, Node, and Ruby, `a / b` in Python, and `a . b` in PHP and
    /// Perl.
    fn operands(&self, expression: &'c str) -> Vec<(Join, &'c str)> {
        let bytes = expression.as_bytes();
        let mut parts = Vec::new();
        let mut start = 0;
        for at in surface(expression) {
            let previous = expression[..at].trim_end();
            let next = bytes.get(at + 1).copied();
            let join = match bytes[at] {
                b'+' if !matches!(self.language, Language::Php | Language::Perl)
                    && !matches!(next, Some(b'+' | b'='))
                    && !previous.is_empty()
                    && !previous.ends_with(['+', '(', ',', '=']) =>
                {
                    Join::Concat
                }
                b'/' if matches!(self.language, Language::Python)
                    && !matches!(next, Some(b'/' | b'='))
                    && !previous.ends_with('/')
                    && !previous.is_empty() =>
                {
                    Join::Separator
                }
                b'.' if matches!(self.language, Language::Php | Language::Perl)
                    && (previous.len() < at
                        || matches!(next, Some(b' ' | b'\'' | b'"'))
                        || previous.ends_with(['\'', '"'])) =>
                {
                    Join::Concat
                }
                _ => continue,
            };
            parts.push((join, &expression[start..at]));
            start = at + 1;
        }
        if parts.is_empty() {
            return Vec::new();
        }
        parts.push((Join::Concat, &expression[start..]));
        // Each part joins the one before it with the operator that preceded it.
        let mut joined = Vec::with_capacity(parts.len());
        let mut previous = Join::Concat;
        for (join, part) in parts {
            joined.push((previous, part));
            previous = join;
        }
        joined
    }

    /// A string literal: a full path, or the text before an interpolation, a
    /// `%` format, or `.format()`.
    fn literal(&self, expression: &str) -> Option<Value> {
        let letters = expression
            .bytes()
            .take_while(u8::is_ascii_alphabetic)
            .count();
        if letters > 2 {
            return None;
        }
        let quote = *expression.as_bytes().get(letters)?;
        if !matches!(quote, b'\'' | b'"' | b'`') {
            return None;
        }
        // Backticks run a command in Perl, Ruby, and PHP.
        if quote == b'`' && !matches!(self.language, Language::Node) {
            return Some(Value::Unknown);
        }
        let close = closing(expression, letters + 1, quote)?;
        let body = &expression[letters + 1..close];
        let rest = expression[close + 1..].trim_start();
        let python = matches!(self.language, Language::Python);
        let formatted = python && (rest.starts_with('%') || rest.starts_with(".format("));
        if !rest.is_empty() && !formatted {
            return Some(Value::Unknown);
        }
        // `$` and `{` may be interpolated, by the language or by the shell
        // that passed the code on.
        let mut markers = vec!['$', '{'];
        if quote == b'"' && matches!(self.language, Language::Perl) {
            markers.push('@');
        }
        if python && rest.starts_with('%') {
            markers.push('%');
        }
        let cut = body.find(markers.as_slice()).unwrap_or(body.len());
        let raw = python && expression[..letters].contains(['r', 'R']);
        let fixed = if raw {
            body[..cut].to_owned()
        } else {
            body[..cut].replace("\\\\", "\\")
        };
        if fixed.starts_with("php://") {
            return Some(Value::Device);
        }
        Some(if cut == body.len() && !formatted {
            Value::Path(fixed)
        } else {
            Value::Prefix(fixed)
        })
    }

    /// A call that yields a path: a join, a wrapper, a temporary or home
    /// directory, or a new temporary file.
    fn call(&self, expression: &'c str, depth: usize) -> Option<Value> {
        let expression = expression
            .strip_prefix("await ")
            .map_or(expression, str::trim_start);
        if FRESH.iter().any(|fresh| expression.starts_with(fresh)) {
            let placed = expression.contains("dir=") || expression.contains("dir =");
            return Some(if placed { Value::Unknown } else { Value::Fresh });
        }
        if TEMPORARY.contains(&expression) {
            return Some(path(self.scope.temporary()));
        }
        if HOMES.contains(&expression) {
            return Some(path(self.scope.home()));
        }
        for method in METHODS {
            if let Some(receiver) = expression.strip_suffix(method) {
                let value = self.value(receiver, depth + 1);
                return Some(if *method == ".expanduser()" {
                    self.home_expanded(value)
                } else {
                    value
                });
            }
        }
        let (name, inner) = called(expression)?;
        if PREFIXED.contains(&name) {
            let first = arguments(inner).into_iter().next()?;
            return Some(match self.value(first, depth + 1) {
                Value::Path(prefix) => Value::Prefix(match name {
                    "tempnam" => joined(&prefix, Join::Separator, ""),
                    _ => prefix,
                }),
                value => value,
            });
        }
        if JOINS.contains(&name) {
            let parts = arguments(inner);
            if parts.iter().all(|part| part.trim().is_empty()) {
                return Some(Value::Path(".".to_owned()));
            }
            return Some(
                self.fold(
                    parts
                        .into_iter()
                        .map(|part| (Join::Separator, part))
                        .collect(),
                    depth,
                ),
            );
        }
        if WRAPPERS.contains(&name) {
            let first = arguments(inner).into_iter().next()?;
            let value = self.value(first, depth + 1);
            return Some(match name {
                "os.path.expanduser" | "File.expand_path" => self.home_expanded(value),
                _ => value,
            });
        }
        None
    }

    /// A value with a leading `~` replaced by the home directory.
    fn home_expanded(&self, value: Value) -> Value {
        let expand = |text: &str| {
            let rest = text.strip_prefix('~')?;
            (rest.is_empty() || rest.starts_with(['/', '\\']))
                .then(|| self.scope.home().map(|home| format!("{home}{rest}")))
        };
        match value {
            Value::Path(text) => match expand(&text) {
                Some(Some(path)) => Value::Path(path),
                Some(None) => Value::Unknown,
                None => Value::Path(text),
            },
            Value::Prefix(text) => match expand(&text) {
                Some(Some(prefix)) => Value::Prefix(prefix),
                Some(None) => Value::Unknown,
                None => Value::Prefix(text),
            },
            value => value,
        }
    }

    /// A name's value, when the code binds it exactly once by assignment or
    /// `with ... as`. A pytest temporary-directory fixture is fresh.
    fn name(&self, expression: &str, depth: usize) -> Option<Value> {
        let plain = match self.language {
            Language::Php | Language::Perl => expression.strip_prefix('$')?,
            _ => expression,
        };
        let mut characters = plain.chars();
        if !characters
            .next()
            .is_some_and(|first| first == '_' || first.is_alphabetic())
            || !characters.all(|character| character == '_' || character.is_alphanumeric())
        {
            return None;
        }
        if !self.spend(self.code.len()) {
            return Some(Value::Unknown);
        }
        let mut bound = None;
        let mut count = 0;
        let fixture = FIXTURES.contains(&expression);
        for at in occurrences(self.code, expression) {
            match self.binding(at, expression) {
                Binding::Use => {}
                // A test function takes a fixture as a parameter.
                Binding::Other if fixture && self.parameter(at) => {}
                Binding::Value(value) => {
                    count += 1;
                    bound = Some(value);
                }
                Binding::Other => return Some(Value::Unknown),
            }
        }
        Some(match bound {
            Some(value) if count == 1 => self.value(value, depth + 1),
            None if fixture => Value::Fresh,
            _ => Value::Unknown,
        })
    }

    /// Whether the occurrence at `at` lies in a function's parameter list.
    fn parameter(&self, at: usize) -> bool {
        let start = self.code[..at].rfind('\n').map_or(0, |newline| newline + 1);
        let lead = self.code[start..at].trim_start();
        (lead.starts_with("def ") || lead.starts_with("async def ")) && lead.contains('(')
    }

    /// How the occurrence of `name` at `at` binds it.
    fn binding(&self, at: usize, name: &str) -> Binding<'c> {
        let code = self.code;
        let start = code[..at].rfind('\n').map_or(0, |newline| newline + 1);
        let lead = code[start..at].trim();
        let after = code[at + name.len()..].trim_start_matches([' ', '\t']);
        let header = lead
            .split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .next()
            .unwrap_or_default();
        if REBINDERS.contains(&header) || lead.ends_with('|') {
            return if lead.starts_with("with ") || lead.starts_with("async with ") {
                self.with_binding(lead)
            } else {
                Binding::Other
            };
        }
        if lead.ends_with(" as") || lead == "as" {
            return if header == "with" {
                self.with_binding(lead)
            } else {
                Binding::Other
            };
        }
        if after.starts_with("=>") || after.starts_with("==") || after.starts_with("=~") {
            return Binding::Use;
        }
        if let Some(value) = after.strip_prefix(":=") {
            return Binding::Value(statement(value));
        }
        if let Some(value) = after.strip_prefix('=') {
            let declared = matches!(
                lead,
                "" | "const"
                    | "let"
                    | "var"
                    | "my"
                    | "our"
                    | "local"
                    | "export const"
                    | "export let"
            );
            return if declared {
                Binding::Value(statement(value))
            } else if lead.ends_with(',') {
                Binding::Other
            } else {
                // A keyword argument, a key, or an attribute.
                Binding::Use
            };
        }
        let augmented = [
            "+=", "-=", "*=", "/=", ".=", "|=", "||=", "&&=", "??=", "<<=",
        ];
        if augmented.iter().any(|operator| after.starts_with(operator)) {
            return Binding::Other;
        }
        Binding::Use
    }

    /// The value `with EXPRESSION as NAME` binds, from the line before the
    /// name: the last item of the statement.
    fn with_binding(&self, lead: &'c str) -> Binding<'c> {
        let Some(item) = lead.strip_suffix("as") else {
            return Binding::Other;
        };
        let item = item.trim_end();
        let item = item
            .strip_prefix("async ")
            .unwrap_or(item)
            .trim_start()
            .strip_prefix("with ")
            .unwrap_or(item);
        let last = surface(item)
            .into_iter()
            .rfind(|&at| item.as_bytes()[at] == b',')
            .map_or(item, |comma| &item[comma + 1..]);
        Binding::Value(last.trim())
    }
}

enum Binding<'c> {
    /// A use, a keyword argument, or anything else that leaves the name.
    Use,
    /// An assignment of this expression.
    Value(&'c str),
    /// A loop, parameter, unpacking, or update that rebinds the name.
    Other,
}

/// `path` as a value, when it is known.
fn path(path: Option<&str>) -> Value {
    path.map_or(Value::Unknown, |path| Value::Path(path.to_owned()))
}

/// `base` and `part` joined as `join` says.
fn joined(base: &str, join: Join, part: &str) -> String {
    match join {
        Join::Concat => format!("{base}{part}"),
        Join::Separator if is_absolute(part) => part.to_owned(),
        Join::Separator if base.is_empty() || base.ends_with(['/', '\\']) => {
            format!("{base}{part}")
        }
        Join::Separator => format!("{base}/{part}"),
    }
}

/// Whether an expression is a stream or descriptor rather than a file:
/// `sys.stdout`, a file descriptor number, or `x.fileno()`.
fn is_device(expression: &str) -> bool {
    DEVICES.contains(&expression)
        || expression.bytes().all(|byte| byte.is_ascii_digit())
        || expression.ends_with(".fileno()")
}

/// An expression without the parentheses around all of it.
fn unwrapped(expression: &str) -> &str {
    let mut expression = expression.trim();
    while expression.starts_with('(')
        && matching(expression, 0).is_some_and(|close| close + 1 == expression.len())
    {
        expression = expression[1..expression.len() - 1].trim();
    }
    expression
}

/// The name and parenthesized arguments of a call that is the whole
/// expression, as in `os.path.join(a, b)`.
fn called(expression: &str) -> Option<(&str, &str)> {
    let open = expression.find('(')?;
    let close = matching(expression, open)?;
    let name = expression[..open].trim_end();
    (close + 1 == expression.len() && !name.is_empty()).then(|| (name, &expression[open..]))
}

/// Offsets in `text` at its top level: outside strings and brackets.
fn surface(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut offsets = Vec::new();
    let mut depth = 0_usize;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' | b'`' => {
                index = closing(text, index + 1, byte).unwrap_or(bytes.len());
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => offsets.push(index),
            _ => {}
        }
        index += 1;
    }
    offsets
}

/// The offset of the quote that closes a string whose body starts at
/// `from`, past backslash escapes.
fn closing(text: &str, from: usize, quote: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'\\' {
            index += 2;
            continue;
        }
        if byte == quote {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// The offset of the bracket that closes the one at `open`.
fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = open;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' | b'`' => index = closing(text, index + 1, byte)?,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// The offset of the bracket that opens the one at `close`, read backwards.
fn opening(text: &str, close: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = close + 1;
    while index > 0 {
        index -= 1;
        match bytes[index] {
            quote @ (b'\'' | b'"' | b'`') => index = text[..index].rfind(char::from(quote))?,
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The receiver of a method call, given the code before the call's `.`: a
/// chain of names, calls, subscripts, and parenthesized expressions, such as
/// `Path('a.txt')`, `out_dir`, or `(base / 'a.txt')`.
fn receiver(before: &str) -> Option<&str> {
    let text = before.strip_suffix('.')?.trim_end();
    // Only the end of the code is read, so many calls cost linear time.
    let mut cut = text.len().saturating_sub(MAX_CALL_SPAN);
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    let text = &text[cut..];
    let bytes = text.as_bytes();
    let mut start = bytes.len();
    while let Some(&last) = start.checked_sub(1).and_then(|at| bytes.get(at)) {
        let bracket = matches!(last, b')' | b']');
        start = match last {
            b')' | b']' => opening(text, start - 1)?,
            b'\'' | b'"' => text[..start - 1].rfind(char::from(last))?,
            _ if is_word(last) => text[..start]
                .trim_end_matches(|character: char| {
                    character.is_ascii_alphanumeric() || character == '_'
                })
                .len(),
            _ => break,
        };
        match start.checked_sub(1).map(|at| bytes[at]) {
            Some(b'.') => start -= 1,
            // A call's or subscript's name comes before its brackets.
            Some(byte) if bracket && is_word(byte) => {}
            _ => break,
        }
    }
    let expression = text[start..].trim();
    (!expression.is_empty()).then_some(expression)
}

/// Offsets where the name `name` occurs in `code` as a whole word, not as
/// an attribute.
fn occurrences<'c>(code: &'c str, name: &'c str) -> impl Iterator<Item = usize> + 'c {
    code.match_indices(name)
        .map(|(at, _)| at)
        .filter(move |&at| {
            let before = code[..at].chars().next_back();
            let after = code[at + name.len()..].chars().next();
            !before.is_some_and(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '.' | '$')
            }) && !after.is_some_and(|character| character.is_alphanumeric() || character == '_')
        })
}

/// The expression an assignment's value starts, up to the end of its
/// statement: a newline or `;` outside brackets.
fn statement(text: &str) -> &str {
    let text = prefix(text, MAX_CALL_SPAN);
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' | b'`' => match closing(text, index + 1, byte) {
                Some(close) => index = close,
                None => break,
            },
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => break,
            b')' | b']' | b'}' => depth -= 1,
            b'\n' | b';' if depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    text[..index.min(text.len())].trim()
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
