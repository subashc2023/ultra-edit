//! Decides whether a write target lies certainly outside the project.
//!
//! Paths are compared lexically: `.` and `..` are resolved on the text and
//! nothing touches the filesystem, so symlinks are not followed. Names compare
//! without regard to case, and under a Windows root `\` and `/` are the same
//! separator. A target that depends on anything unknown counts as inside.

use super::Dialect;
use super::powershell::{escape, quote};

/// Environment variables whose values a [`Scope`] may expand in targets.
pub const SCOPE_VARIABLES: [&str; 6] = [
    "CLAUDE_PROJECT_DIR",
    "HOME",
    "TMPDIR",
    "USERPROFILE",
    "TEMP",
    "TMP",
];

/// Files that GitHub Actions and compatible runners read after a step.
const RUNNER_FILES: [&str; 5] = [
    "GITHUB_ENV",
    "GITHUB_OUTPUT",
    "GITHUB_PATH",
    "GITHUB_STATE",
    "GITHUB_STEP_SUMMARY",
];

/// PowerShell drives that hold no files.
const NON_FILE_DRIVES: [&str; 8] = [
    "alias", "cert", "env", "function", "hkcu", "hklm", "variable", "wsman",
];

/// Target words longer than this count as inside without being read.
const MAX_TARGET_BYTES: usize = 4096;

/// The project a command runs in and the environment values its write
/// targets may expand. The hook builds it from its own environment and the
/// event; the classifier never reads the environment itself.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    root: Option<Root>,
    variables: Vec<(&'static str, String)>,
    /// Whether the script passed through another shell's expansions, such
    /// as `pwsh -c "..."` in Bash, so the variables it names are unknown.
    foreign: bool,
}

impl Scope {
    /// Scopes writes to `root` when it is an absolute path; otherwise every
    /// file target counts as inside the project.
    pub fn new(root: &str) -> Self {
        Self {
            root: Root::parse(root),
            ..Self::default()
        }
    }

    /// This scope for a script that another shell's quoting and expansion
    /// produced: a target that starts with a variable counts as inside.
    pub(super) fn foreign(&self) -> Self {
        Self {
            root: self.root.clone(),
            variables: Vec::new(),
            foreign: true,
        }
    }

    /// Records the value of one of [`SCOPE_VARIABLES`]. Other names and empty
    /// values are ignored, so targets using them stay inside.
    #[must_use]
    pub fn with_variable(mut self, name: &str, value: &str) -> Self {
        if let Some(name) = SCOPE_VARIABLES.into_iter().find(|known| *known == name)
            && !value.is_empty()
        {
            self.variables.retain(|(known, _)| *known != name);
            self.variables.push((name, value.to_owned()));
        }
        self
    }

    fn variable(&self, name: &str) -> Option<&str> {
        self.variables
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether a write to the word whose source text is `raw` may reach a
    /// project file. Discarded output never does; under a root, runner files
    /// and absolute paths outside the root do not either.
    pub(super) fn inside(&self, dialect: Dialect, raw: &str) -> bool {
        let windows = self.root.as_ref().is_some_and(|root| root.windows);
        let target = match dialect {
            _ if raw.len() > MAX_TARGET_BYTES => Target::Dynamic,
            Dialect::Bash => bash_target(raw, self),
            Dialect::PowerShell => powershell_target(raw, self, windows),
        };
        match (target, &self.root) {
            (Target::Device, _) => false,
            (_, None) | (Target::Dynamic, _) => true,
            (Target::Runner, Some(_)) => false,
            (Target::Path(path), Some(root)) => !root.excludes(&path, dialect),
        }
    }
}

/// Whether `path` is absolute as a POSIX, drive-letter, or UNC path.
pub fn is_absolute(path: &str) -> bool {
    Root::parse(path).is_some()
}

#[derive(Clone, Debug)]
struct Root {
    windows: bool,
    /// Folded components, starting with `/`, a drive such as `C:`, or a UNC
    /// share such as `//SERVER/SHARE`.
    components: Vec<String>,
}

impl Root {
    fn parse(path: &str) -> Option<Self> {
        if path.starts_with('/') {
            return posix(path).map(|components| Self {
                windows: false,
                components,
            });
        }
        windows(path, false).map(|components| Self {
            windows: true,
            components,
        })
    }

    /// Whether `path` is absolute and lies outside this root.
    fn excludes(&self, path: &str, dialect: Dialect) -> bool {
        let components = if self.windows {
            // Git Bash spells `C:\x` as `/c/x`; PowerShell reads `/c/x` as a
            // folder on the current drive.
            windows(path, dialect == Dialect::Bash)
        } else if dialect == Dialect::PowerShell {
            // PowerShell accepts `\` as a separator on every platform.
            posix(&path.replace('\\', "/"))
        } else {
            posix(path)
        };
        components.is_some_and(|components| !components.starts_with(&self.components))
    }
}

/// The folded components of an absolute POSIX path.
fn posix(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix('/')?;
    Some(resolve(vec!["/".to_owned()], rest.split('/'), false))
}

/// The folded components of a drive-letter or UNC path, or with `msys` of a
/// Git Bash `/c/...` path. Other rooted paths depend on the current drive or
/// the MSYS mount table, so they are not absolute here.
fn windows(path: &str, msys: bool) -> Option<Vec<String>> {
    let path = path.replace('\\', "/");
    // Verbatim `\\?\` and device `\\.\` prefixes name the same paths.
    let path = match path
        .strip_prefix("//?/")
        .or_else(|| path.strip_prefix("//./"))
    {
        Some(rest) => match rest.get(..4) {
            Some(unc) if unc.eq_ignore_ascii_case("UNC/") => format!("//{}", &rest[4..]),
            _ => rest.to_owned(),
        },
        None => path,
    };
    let (prefix, rest) = match path.as_bytes() {
        [letter, b':', b'/', ..] if letter.is_ascii_alphabetic() => {
            (drive(*letter), path.get(3..).unwrap_or_default())
        }
        [b'/', b'/', ..] => {
            let mut parts = path[2..].splitn(3, '/');
            let server = parts.next().filter(|server| !server.is_empty())?;
            let share = parts.next().filter(|share| !share.is_empty())?;
            (
                format!("//{}/{}", fold(server), fold(share)),
                parts.next().unwrap_or_default(),
            )
        }
        [b'/', letter, rest @ ..]
            if msys
                && letter.is_ascii_alphabetic()
                && matches!(rest.first(), None | Some(b'/')) =>
        {
            (drive(*letter), path.get(2..).unwrap_or_default())
        }
        _ => return None,
    };
    Some(resolve(vec![prefix], rest.split('/'), true))
}

fn drive(letter: u8) -> String {
    format!("{}:", letter.to_ascii_uppercase() as char)
}

/// Appends path parts to `components`, resolving `.` and `..` lexically.
fn resolve<'p>(
    mut components: Vec<String>,
    parts: impl Iterator<Item = &'p str>,
    windows: bool,
) -> Vec<String> {
    for part in parts {
        // Windows drops trailing dots and spaces from each name.
        let part = match part {
            "." | ".." => part,
            _ if windows => part.trim_end_matches(['.', ' ']),
            _ => part,
        };
        match part {
            "" | "." => {}
            ".." => {
                if components.len() > 1 {
                    components.pop();
                }
            }
            _ => components.push(fold(part)),
        }
    }
    components
}

/// Folds case, so paths that differ only in case compare equal. Full
/// uppercasing never separates names that Windows or macOS treat as one.
fn fold(name: &str) -> String {
    name.to_uppercase()
}

/// A write target read from its source text.
enum Target {
    /// Output that no file receives, such as `$null` or the `Env:` drive.
    Device,
    /// A CI runner file such as `$GITHUB_OUTPUT`.
    Runner,
    /// A path whose text is fully known.
    Path(String),
    /// A path that depends on anything else only known at run time.
    Dynamic,
}

/// Reads a Bash word as a path, expanding only a leading `~`, `$HOME`,
/// `$TMPDIR`, or `$CLAUDE_PROJECT_DIR`. Other expansions, substitutions, and
/// unquoted glob or brace characters make it dynamic.
fn bash_target(raw: &str, scope: &Scope) -> Target {
    let bytes = raw.as_bytes();
    let mut text = Vec::new();
    let mut lead = None;
    let mut quoted = false;
    let mut at = 0;
    if bytes.first() == Some(&b'~') && matches!(bytes.get(1), None | Some(b'/')) {
        lead = Some("HOME");
        at = 1;
    }
    while let Some(&byte) = bytes.get(at) {
        at += 1;
        match byte {
            b'"' => quoted = !quoted,
            b'\'' if !quoted => {
                let Some(end) = bytes[at..].iter().position(|&byte| byte == b'\'') else {
                    return Target::Dynamic;
                };
                text.extend_from_slice(&bytes[at..at + end]);
                at += end + 1;
            }
            b'\\' => match bytes.get(at) {
                Some(b'\n') => at += 1,
                Some(&next) if !quoted || matches!(next, b'$' | b'`' | b'"' | b'\\') => {
                    text.push(next);
                    at += 1;
                }
                _ => text.push(byte),
            },
            b'$' => {
                let Some((name, length)) = bash_variable(&bytes[at..]) else {
                    return Target::Dynamic;
                };
                // Only a leading variable expands; a later one is unknown.
                if lead.is_some() || !text.is_empty() {
                    return Target::Dynamic;
                }
                lead = Some(name);
                at += length;
            }
            b'`' => return Target::Dynamic,
            b'*' | b'?' | b'[' | b'{' if !quoted => return Target::Dynamic,
            _ => text.push(byte),
        }
    }
    if quoted || (scope.foreign && lead.is_some()) {
        return Target::Dynamic;
    }
    let rest = String::from_utf8_lossy(&text);
    match lead {
        None => Target::Path(rest.into_owned()),
        Some(name) if RUNNER_FILES.contains(&name) => runner(&rest),
        Some(name @ ("HOME" | "TMPDIR" | "CLAUDE_PROJECT_DIR")) => {
            expanded(scope.variable(name), &rest)
        }
        Some(_) => Target::Dynamic,
    }
}

/// The name of the `$NAME` or `${NAME}` expansion whose `$` precedes
/// `bytes`, and how many bytes it spans after the `$`.
fn bash_variable(bytes: &[u8]) -> Option<(&str, usize)> {
    let braced = bytes.first() == Some(&b'{');
    let start = usize::from(braced);
    let length = bytes[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    if length == 0 || bytes[start].is_ascii_digit() {
        return None;
    }
    let name = std::str::from_utf8(&bytes[start..start + length]).ok()?;
    if !braced {
        return Some((name, length));
    }
    (bytes.get(start + length) == Some(&b'}')).then_some((name, length + 2))
}

/// Reads a PowerShell word as a path, expanding only a leading `~`, `$HOME`,
/// or `$env:` variable, and recognizing `$null` and drives without files.
/// Other variables, subexpressions, and wildcard characters, which `-Path`
/// expands even inside quotes, make it dynamic.
fn powershell_target(raw: &str, scope: &Scope, windows: bool) -> Target {
    if raw.contains(['*', '?', '[']) {
        return Target::Dynamic;
    }
    let bytes = raw.as_bytes();
    let mut text = Vec::new();
    let mut lead = None;
    let mut double = false;
    let mut at = 0;
    if bytes.first() == Some(&b'~') && matches!(bytes.get(1), None | Some(b'/' | b'\\')) {
        lead = Some("home");
        at = 1;
    }
    while let Some(&byte) = bytes.get(at) {
        match quote(bytes, at) {
            Some((b'"', length)) => {
                at += length;
                match quote(bytes, at) {
                    // `""` inside double quotes is a literal quote.
                    Some((b'"', next)) if double => {
                        text.push(b'"');
                        at += next;
                    }
                    _ => double = !double,
                }
                continue;
            }
            Some((_, length)) if !double => {
                at += length;
                // Single-quoted text is literal, and `''` is a quote.
                loop {
                    if let Some((b'\'', length)) = quote(bytes, at) {
                        at += length;
                        let Some((b'\'', next)) = quote(bytes, at) else {
                            break;
                        };
                        text.push(b'\'');
                        at += next;
                    } else if let Some(&inner) = bytes.get(at) {
                        text.push(inner);
                        at += 1;
                    } else {
                        return Target::Dynamic;
                    }
                }
                continue;
            }
            _ => {}
        }
        at += 1;
        match byte {
            b'`' => {
                let Some(&next) = bytes.get(at) else {
                    return Target::Dynamic;
                };
                text.push(escape(next));
                at += 1;
            }
            b'$' => {
                let Some((name, length)) = powershell_variable(&bytes[at..]) else {
                    return Target::Dynamic;
                };
                if lead.is_some() || !text.is_empty() {
                    return Target::Dynamic;
                }
                lead = Some(name);
                at += length;
            }
            _ => text.push(byte),
        }
    }
    if double || (scope.foreign && lead.is_some()) {
        return Target::Dynamic;
    }
    let rest = String::from_utf8_lossy(&text);
    let Some(name) = lead else {
        return drive_path(&rest, scope);
    };
    let lower = name.to_ascii_lowercase();
    // Environment names ignore case on Windows only.
    let environment = lower.strip_prefix("env:").and_then(|_| {
        let variable = &name[4..];
        SCOPE_VARIABLES
            .into_iter()
            .chain(RUNNER_FILES)
            .find(|known| known.eq_ignore_ascii_case(variable))
            .filter(|known| windows || *known == variable)
    });
    match (lower.as_str(), environment) {
        ("home", _) => expanded(
            scope
                .variable("USERPROFILE")
                .or_else(|| scope.variable("HOME")),
            &rest,
        ),
        ("null", _) if rest.is_empty() => Target::Device,
        (_, Some(known)) if RUNNER_FILES.contains(&known) => runner(&rest),
        (_, Some(known)) => expanded(scope.variable(known), &rest),
        _ => Target::Dynamic,
    }
}

/// The name after `$` in `$name`, `$scope:name`, or `${name}`, and how many
/// bytes it spans after the `$`.
fn powershell_variable(bytes: &[u8]) -> Option<(&str, usize)> {
    if bytes.first() == Some(&b'{') {
        let end = bytes.iter().position(|&byte| byte == b'}')?;
        return Some((std::str::from_utf8(&bytes[1..end]).ok()?, end + 1));
    }
    let name = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count()
    };
    let mut length = name(0);
    if length == 0 {
        return None;
    }
    if bytes.get(length) == Some(&b':') && name(length + 1) > 0 {
        length += 1 + name(length + 1);
    }
    Some((std::str::from_utf8(&bytes[..length]).ok()?, length))
}

/// Reads a literal PowerShell path's drive: `Env:` and other drives without
/// files receive no file, `Temp:` is the temporary directory, and other named
/// drives may map anywhere.
fn drive_path(path: &str, scope: &Scope) -> Target {
    let Some((drive, rest)) = path.split_once(':') else {
        return Target::Path(path.to_owned());
    };
    let named = drive.len() > 1
        && drive.starts_with(|character: char| character.is_ascii_alphabetic())
        && drive
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !named {
        return Target::Path(path.to_owned());
    }
    let drive = drive.to_ascii_lowercase();
    if NON_FILE_DRIVES.contains(&drive.as_str()) {
        Target::Device
    } else if drive == "temp" {
        expanded(scope.variable("TEMP"), &format!("/{rest}"))
    } else {
        Target::Dynamic
    }
}

fn runner(rest: &str) -> Target {
    if rest.is_empty() {
        Target::Runner
    } else {
        Target::Dynamic
    }
}

fn expanded(value: Option<&str>, rest: &str) -> Target {
    value.map_or(Target::Dynamic, |value| {
        Target::Path(format!("{value}{rest}"))
    })
}
