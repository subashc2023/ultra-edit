//! Measures how precise and how complete `similar` near-miss candidates are on
//! this repository's own text, through the public compiler.
//!
//! - False positives: a line, or a block of two or three lines, from one file is
//!   searched in another file of the same language, where it occurs neither
//!   exactly nor whitespace-equal. Any `similar` candidate is a false positive.
//! - Recall: a line or block is mutated the way a model misremembers text (a
//!   typo, a renamed identifier, a changed literal, a deleted or inserted token,
//!   a changed argument, or two of those for a stale line) and searched in its
//!   own file. It counts as found when its origin lines, or lines with the same
//!   text, are the first candidate (top-1), or any candidate (top-3).
//! - False positives in large files: as the first, but searching every other file
//!   of a language joined into one, since longer files offer more lines of the
//!   same shape.
//!
//! Needles are as read or with the first line's indentation trimmed. Needles
//! with fewer than 12 visible characters, or with only punctuation and common
//! keywords, are skipped. Sampling is seeded, so runs over the same text agree;
//! the digest changes whenever any candidate does.
//!
//! ```text
//! cargo run --release --example candidate_quality -- [ROOT] [--examples N]
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use ultra_edit::compiler::{compile, snapshot};
use ultra_edit::{Candidate, CandidateKind, Change, EditRequest, FileRequest, Snapshot, Target};

const SEED: u64 = 0x5eed_1e55;
/// Searched unrelated needles per language and shape.
const UNRELATED: usize = 2_000;
/// Searched mutated needles per mutation and shape.
const MUTATED: usize = 500;
/// Searched unrelated needles per language and shape in large joined files.
const LARGE: usize = 500;
/// A line, or a block of two or three lines; as read, or with the first line's
/// indentation trimmed, as a model often quotes code.
const SHAPES: [Shape; 4] = [
    Shape {
        block: false,
        trimmed: false,
    },
    Shape {
        block: false,
        trimmed: true,
    },
    Shape {
        block: true,
        trimmed: false,
    },
    Shape {
        block: true,
        trimmed: true,
    },
];
const MUTATIONS: [&str; 6] = ["typo", "rename", "literal", "token", "argument", "stale"];
/// Words that alone do not make a line worth searching for.
const KEYWORDS: &[&str] = &[
    "and", "as", "async", "await", "break", "case", "class", "const", "continue", "def", "default",
    "do", "done", "elif", "else", "end", "enum", "esac", "except", "false", "False", "fi",
    "finally", "fn", "for", "from", "func", "function", "if", "impl", "import", "in", "is", "let",
    "loop", "match", "mod", "mut", "new", "nil", "None", "not", "null", "Ok", "Err", "or", "pass",
    "pub", "raise", "return", "self", "Self", "Some", "static", "struct", "super", "then", "this",
    "true", "True", "try", "type", "use", "var", "where", "while", "with", "yield",
];

/// SplitMix64: small, seedable, and the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

struct File {
    path: String,
    language: &'static str,
    base: Snapshot,
    bases: BTreeMap<String, Snapshot>,
    /// Line bodies, without line endings or a leading BOM.
    lines: Vec<Range<usize>>,
    /// Lines worth searching for.
    useful: Vec<usize>,
    /// Identifiers of three or more characters, for renames and insertions.
    vocabulary: Vec<String>,
}

impl File {
    fn text(&self, lines: Range<usize>) -> &str {
        &self.base.text[self.lines[lines.start].start..self.lines[lines.end - 1].end]
    }
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Maximal runs of word characters, with their byte ranges.
fn words(line: &str) -> Vec<(Range<usize>, &str)> {
    let mut found = Vec::new();
    let mut start = None;
    for (offset, character) in line.char_indices().chain([(line.len(), ' ')]) {
        match (is_word(character), start) {
            (true, None) => start = Some(offset),
            (false, Some(begin)) => {
                found.push((begin..offset, &line[begin..offset]));
                start = None;
            }
            _ => {}
        }
    }
    found
}

fn is_identifier(word: &str) -> bool {
    word.chars().count() >= 3
        && word.starts_with(|character: char| character.is_alphabetic() || character == '_')
        && !KEYWORDS.contains(&word)
}

fn useful(line: &str) -> bool {
    line.chars()
        .filter(|character| !character.is_whitespace())
        .count()
        >= 12
        && words(line).iter().any(|(_, word)| !KEYWORDS.contains(word))
}

fn language(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next()?;
    if name == "Makefile" {
        return Some("make");
    }
    let languages = ["rs", "py", "md", "go", "json", "ps1", "sh", "cmd"];
    languages
        .into_iter()
        .find(|extension| name.ends_with(&format!(".{extension}")))
}

fn walk(root: &Path, directory: &Path, recursive: bool, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                walk(root, &path, true, out);
            }
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `src/**/*.rs`, `tests/**/*.rs`, `scripts/**/*.py`, `eval/**/*.py`,
/// `docs/*.md`, `README.md`, and every file under `eval/tasks/*/fixture/`.
fn corpus(root: &Path) -> Vec<File> {
    let mut paths = BTreeSet::new();
    for (directory, recursive) in [("src", true), ("tests", true), ("scripts", true)] {
        walk(root, &root.join(directory), recursive, &mut paths);
    }
    walk(root, &root.join("eval"), true, &mut paths);
    walk(root, &root.join("docs"), false, &mut paths);
    paths.insert("README.md".into());
    let wanted = |path: &str| {
        let fixture = path.starts_with("eval/tasks/") && path.contains("/fixture/");
        match language(path) {
            _ if path.contains("/expected/") => false,
            Some("rs") => path.starts_with("src/") || path.starts_with("tests/"),
            Some("py") => path.starts_with("scripts/") || path.starts_with("eval/"),
            Some("md") => fixture || path == "README.md" || path.starts_with("docs/"),
            Some(_) => fixture,
            None => false,
        }
    };
    paths
        .into_iter()
        .filter(|path| wanted(path))
        .filter_map(|path| {
            let text = fs::read_to_string(root.join(&path)).ok()?;
            Some(load(language(&path)?, path, text))
        })
        .collect()
}

fn load(language: &'static str, path: String, text: String) -> File {
    let mut start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let lines: Vec<Range<usize>> = text[start..]
        .split_inclusive('\n')
        .map(|line| {
            let body = line
                .strip_suffix('\n')
                .map_or(line, |body| body.strip_suffix('\r').unwrap_or(body));
            let range = start..start + body.len();
            start += line.len();
            range
        })
        .collect();
    let useful = (0..lines.len())
        .filter(|index| useful(&text[lines[*index].clone()]))
        .collect();
    let vocabulary: BTreeSet<String> = words(&text)
        .into_iter()
        .filter(|(_, word)| is_identifier(word))
        .map(|(_, word)| word.to_owned())
        .collect();
    let base = snapshot(path.clone(), text);
    File {
        language,
        path,
        bases: BTreeMap::from([(base.id.clone(), base.clone())]),
        base,
        lines,
        useful,
        vocabulary: vocabulary.into_iter().collect(),
    }
}

/// Every other file of a language, joined into one large file.
fn joined(group: &[&File], half: usize) -> File {
    let mut text = String::new();
    for file in group.iter().skip(half).step_by(2) {
        text.push_str(file.base.text.trim_start_matches('\u{feff}'));
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    load(
        group[0].language,
        format!("{} half {half}", group[0].language),
        text,
    )
}

/// Candidates for a needle that the similar tier searched for, or `None` when
/// it occurs exactly or whitespace-equal, so no similar search runs.
fn search(file: &File, needle: &str) -> Option<Vec<Candidate>> {
    let change = Change {
        id: "c".into(),
        target: Target::Exact {
            old: needle.into(),
            scope: None,
        },
        text: String::new(),
    };
    let request = EditRequest {
        request_id: "quality".into(),
        files: vec![FileRequest {
            base: file.base.id.clone(),
            changes: vec![change],
        }],
    };
    let mut errors = compile(&request, &file.bases).err()?;
    let error = errors.remove(0);
    let whitespace = error
        .candidates
        .first()
        .is_some_and(|candidate| candidate.kind != CandidateKind::Similar);
    (error.code == "TARGET_NOT_FOUND" && !whitespace).then_some(error.candidates)
}

fn replace_range(line: &str, range: Range<usize>, with: &str) -> String {
    format!("{}{with}{}", &line[..range.start], &line[range.end..])
}

fn typo(rng: &mut Rng, line: &str) -> Option<String> {
    let mut chars: Vec<char> = line.chars().collect();
    for _ in 0..1 + rng.below(2) {
        let letters: Vec<usize> = (0..chars.len())
            .filter(|index| is_word(chars[*index]))
            .collect();
        let at = *rng.pick(&letters)?;
        let letter = char::from(b'a' + rng.below(26) as u8);
        match rng.below(4) {
            0 => {
                chars.remove(at);
            }
            1 => chars.insert(at, letter),
            2 if at + 1 < chars.len() && is_word(chars[at + 1]) => chars.swap(at, at + 1),
            _ => chars[at] = letter,
        }
    }
    Some(chars.into_iter().collect())
}

/// The word parts of an identifier: snake_case pieces, or camelCase humps.
fn parts(name: &str) -> Vec<String> {
    if name.contains('_') {
        return name
            .split('_')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut previous_lower = false;
    for character in name.chars() {
        if character.is_uppercase() && previous_lower || parts.is_empty() {
            parts.push(String::new());
        }
        if let Some(last) = parts.last_mut() {
            last.push(character);
        }
        previous_lower = character.is_lowercase();
    }
    parts
}

/// A related name: one word part dropped, replaced, or added, in the same style.
fn variant(rng: &mut Rng, name: &str, vocabulary: &[String]) -> Option<String> {
    let mut pieces = parts(name);
    let donor_name = rng.pick(vocabulary)?;
    let donor = rng.pick(&parts(donor_name))?.to_lowercase();
    let at = rng.below(pieces.len());
    match rng.below(3) {
        0 if pieces.len() > 1 => {
            pieces.remove(at);
        }
        1 => pieces[at] = donor,
        _ => pieces.insert(rng.below(pieces.len() + 1), donor),
    }
    let upper = name
        .chars()
        .filter(|character| character.is_alphabetic())
        .all(char::is_uppercase);
    if name.contains('_') || pieces.len() == 1 {
        let joined = pieces.join("_");
        return Some(if upper { joined.to_uppercase() } else { joined });
    }
    let capitalized = name.starts_with(char::is_uppercase);
    Some(
        pieces
            .iter()
            .enumerate()
            .map(|(index, piece)| {
                let lower = piece.to_lowercase();
                let mut characters = lower.chars();
                match characters.next() {
                    Some(first) if index > 0 || capitalized => {
                        first.to_uppercase().chain(characters).collect()
                    }
                    _ => lower,
                }
            })
            .collect(),
    )
}

fn rename(rng: &mut Rng, line: &str, vocabulary: &[String]) -> Option<String> {
    let names: Vec<&str> = words(line)
        .into_iter()
        .map(|(_, word)| word)
        .filter(|word| is_identifier(word))
        .collect();
    let old = *rng.pick(&names)?;
    let new = if rng.below(2) == 0 {
        variant(rng, old, vocabulary)?
    } else {
        rng.pick(vocabulary)?.clone()
    };
    let mut renamed = line.to_owned();
    for (range, _) in words(line)
        .into_iter()
        .rev()
        .filter(|(_, word)| *word == old)
    {
        renamed = replace_range(&renamed, range, &new);
    }
    Some(renamed)
}

/// Contents of double- or single-quoted strings on the line.
fn strings(line: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for (offset, character) in line.char_indices() {
        match open {
            Some((quote, start)) if character == quote => {
                if offset > start {
                    found.push(start..offset);
                }
                open = None;
            }
            None if character == '"' || character == '\'' => open = Some((character, offset + 1)),
            _ => {}
        }
    }
    found
}

fn literal(rng: &mut Rng, line: &str, vocabulary: &[String]) -> Option<String> {
    let numbers: Vec<Range<usize>> = words(line)
        .into_iter()
        .filter(|(_, word)| word.len() <= 9 && word.chars().all(|digit| digit.is_ascii_digit()))
        .map(|(range, _)| range)
        .collect();
    let strings = strings(line);
    if !numbers.is_empty() && (strings.is_empty() || rng.below(2) == 0) {
        let range = rng.pick(&numbers)?.clone();
        let value: u64 = line[range.clone()].parse().ok()?;
        let changed = match rng.below(3) {
            0 => value + 1,
            1 if value > 1 => value - 1,
            1 => value + 2,
            _ => value * 10 + rng.below(10) as u64,
        };
        return Some(replace_range(line, range, &changed.to_string()));
    }
    let content = rng.pick(&strings)?.clone();
    let inner = &line[content.clone()];
    let replacement = rng.pick(vocabulary)?.to_lowercase();
    Some(match rng.pick(&words(inner)) {
        Some((range, _)) => replace_range(
            line,
            content.start + range.start..content.start + range.end,
            &replacement,
        ),
        None => replace_range(line, content.end..content.end, &replacement),
    })
}

/// Words, and single characters that are neither word characters nor spaces.
fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = words(line).into_iter().map(|(range, _)| range).collect();
    found.extend(
        line.char_indices()
            .filter(|(_, character)| !is_word(*character) && !character.is_whitespace())
            .map(|(offset, character)| offset..offset + character.len_utf8()),
    );
    found.sort_by_key(|range| range.start);
    found
}

fn token(rng: &mut Rng, line: &str, vocabulary: &[String]) -> Option<String> {
    let tokens = tokens(line);
    let range = rng.pick(&tokens)?.clone();
    if rng.below(2) == 0 {
        // Deleting a word between spaces also deletes one of them.
        let mut range = range;
        if line[..range.start].ends_with(' ') && line[range.end..].starts_with(' ') {
            range.end += 1;
        }
        return Some(replace_range(line, range, ""));
    }
    let inserted = if rng.below(2) == 0 {
        format!("{} ", rng.pick(vocabulary)?)
    } else {
        ["&", "!", "*", "?", ".", "-", ","][rng.below(7)].to_owned()
    };
    Some(replace_range(line, range.start..range.start, &inserted))
}

/// Top-level arguments of each parenthesized list on the line.
fn argument_lists(line: &str) -> Vec<Vec<Range<usize>>> {
    let mut lists = Vec::new();
    let mut stack: Vec<Vec<Range<usize>>> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for (offset, character) in line.char_indices() {
        match character {
            '(' => {
                stack.push(Vec::new());
                starts.push(offset + 1);
            }
            ',' | ')' if !stack.is_empty() => {
                let start = starts.pop().unwrap_or(offset);
                let span = &line[start..offset];
                let trimmed = span.trim_start();
                let begin = start + span.len() - trimmed.len();
                let end = begin + trimmed.trim_end().len();
                let mut list = stack.pop().unwrap_or_default();
                if begin < end {
                    list.push(begin..end);
                }
                if character == ',' {
                    stack.push(list);
                    starts.push(offset + 1);
                } else if !list.is_empty() {
                    lists.push(list);
                }
            }
            _ => {}
        }
    }
    lists
}

fn argument(rng: &mut Rng, line: &str, vocabulary: &[String]) -> Option<String> {
    let lists = argument_lists(line);
    let list = rng.pick(&lists)?;
    let index = rng.below(list.len());
    let range = list[index].clone();
    match rng.below(3) {
        0 if list.len() > 1 => {
            // Drops the argument with the separator after it, or before the last.
            let removed = if index + 1 < list.len() {
                range.start..list[index + 1].start
            } else {
                list[index - 1].end..range.end
            };
            Some(replace_range(line, removed, ""))
        }
        1 => {
            let end = list[list.len() - 1].end;
            let added = format!(", {}", rng.pick(vocabulary)?);
            Some(replace_range(line, end..end, &added))
        }
        _ => {
            let with = if rng.below(2) == 0 {
                rng.pick(vocabulary)?.clone()
            } else {
                rng.below(100).to_string()
            };
            Some(replace_range(line, range, &with))
        }
    }
}

fn mutate(rng: &mut Rng, mutation: &str, line: &str, vocabulary: &[String]) -> Option<String> {
    let mutated = match mutation {
        "typo" => typo(rng, line),
        "rename" => rename(rng, line, vocabulary),
        "literal" => literal(rng, line, vocabulary),
        "token" => token(rng, line, vocabulary),
        _ => argument(rng, line, vocabulary),
    }?;
    (mutated != line).then_some(mutated)
}

/// Mutates one useful line of `lines`, or, for a stale version, makes two edits
/// of different kinds, possibly on different lines.
fn mutated_needle(
    rng: &mut Rng,
    file: &File,
    lines: Range<usize>,
    mutation: &str,
) -> Option<String> {
    let mut texts: Vec<String> = lines
        .clone()
        .map(|index| file.base.text[file.lines[index].clone()].to_owned())
        .collect();
    let targets: Vec<usize> = (0..texts.len())
        .filter(|offset| file.useful.binary_search(&(lines.start + offset)).is_ok())
        .collect();
    let edits: Vec<&str> = if mutation == "stale" {
        let kinds = ["rename", "literal", "token", "argument"];
        let first = rng.below(kinds.len());
        let second = (first + 1 + rng.below(kinds.len() - 1)) % kinds.len();
        vec![kinds[first], kinds[second]]
    } else {
        vec![mutation]
    };
    for edit in edits {
        let target = *rng.pick(&targets)?;
        texts[target] = mutate(rng, edit, &texts[target], &file.vocabulary)?;
    }
    // Keep the source's line endings between lines.
    let mut needle = String::new();
    for (offset, text) in texts.iter().enumerate() {
        if offset > 0 {
            let index = lines.start + offset;
            needle.push_str(&file.base.text[file.lines[index - 1].end..file.lines[index].start]);
        }
        needle.push_str(text);
    }
    Some(needle)
}

/// A line, or a block of two or three lines starting at a useful line.
fn needle_lines(rng: &mut Rng, file: &File, shape: Shape) -> Option<Range<usize>> {
    let first = *rng.pick(&file.useful)?;
    let count = if shape.block { 2 + rng.below(2) } else { 1 };
    let last = first + count;
    (last <= file.lines.len()).then_some(first..last)
}

/// Picks a file with probability proportional to its useful lines.
fn weighted<'a>(rng: &mut Rng, files: &[&'a File]) -> &'a File {
    let total: usize = files.iter().map(|file| file.useful.len()).sum();
    let mut left = rng.below(total.max(1));
    for file in files {
        if left < file.useful.len() {
            return file;
        }
        left -= file.useful.len();
    }
    files[0]
}

fn quoted(text: &str) -> String {
    let flat: String = text.escape_debug().collect();
    if flat.chars().count() > 110 {
        format!("\"{}…\"", flat.chars().take(110).collect::<String>())
    } else {
        format!("\"{flat}\"")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Shape {
    block: bool,
    trimmed: bool,
}

impl Shape {
    fn label(self) -> &'static str {
        match (self.block, self.trimmed) {
            (false, false) => "line",
            (false, true) => "line, trimmed",
            (true, false) => "block",
            (true, true) => "block, trimmed",
        }
    }

    fn apply(self, needle: &str) -> &str {
        if self.trimmed {
            needle.trim_start_matches([' ', '\t'])
        } else {
            needle
        }
    }
}

#[derive(Default)]
struct Tally {
    searched: usize,
    skipped: usize,
    hits: usize,
    top3: usize,
    none: usize,
    scores: [usize; 6],
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.searched += other.searched;
        self.skipped += other.skipped;
        self.hits += other.hits;
        self.top3 += other.top3;
        self.none += other.none;
        for (sum, count) in self.scores.iter_mut().zip(other.scores) {
            *sum += count;
        }
    }
}

struct Report {
    rng: Rng,
    examples: usize,
    digest: u64,
    shown: Vec<String>,
}

impl Report {
    fn record(&mut self, file: &File, needle: &str, found: &[Candidate]) {
        for byte in format!("{}|{needle}|{found:?}", file.path).bytes() {
            self.digest = (self.digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// Needles from `sources` searched in another file of `targets`; any
    /// candidate is wrong.
    fn false_positives(
        &mut self,
        sources: &[&File],
        targets: &[&File],
        shape: Shape,
        count: usize,
    ) -> Tally {
        let mut tally = Tally::default();
        let mut shown = 0;
        let mut attempts = 0;
        while tally.searched < count && attempts < 20 * count {
            attempts += 1;
            let source = weighted(&mut self.rng, sources);
            let others: Vec<&File> = targets
                .iter()
                .copied()
                .filter(|file| file.path != source.path)
                .collect();
            let Some(lines) = needle_lines(&mut self.rng, source, shape) else {
                continue;
            };
            let target = others[self.rng.below(others.len())];
            let needle = shape.apply(source.text(lines));
            let Some(found) = search(target, needle) else {
                tally.skipped += 1;
                continue;
            };
            tally.searched += 1;
            self.record(target, needle, &found);
            let Some(first) = found.first() else {
                continue;
            };
            let score = first.similarity.unwrap_or_default();
            tally.hits += 1;
            tally.scores[(usize::from(score.saturating_sub(70)) / 5).min(5)] += 1;
            if shown < self.examples {
                shown += 1;
                self.shown.push(format!(
                    "  {} {}: {score}% {} -> {}:{}\n    needle {}\n    found  {}",
                    target.language,
                    shape.label(),
                    source.path,
                    target.path,
                    first.line,
                    quoted(needle),
                    quoted(first.text.as_deref().unwrap_or("<long>"))
                ));
            }
        }
        tally
    }

    /// Mutated needles searched in their own file. A candidate is right when
    /// it spans the origin lines, or lines with the same text.
    fn recall(&mut self, files: &[&File], mutation: &str, shape: Shape) -> Tally {
        let mut tally = Tally::default();
        let mut shown = 0;
        let mut attempts = 0;
        while tally.searched < MUTATED && attempts < 50 * MUTATED {
            attempts += 1;
            let file = weighted(&mut self.rng, files);
            let Some(lines) = needle_lines(&mut self.rng, file, shape) else {
                continue;
            };
            let Some(mutated) = mutated_needle(&mut self.rng, file, lines.clone(), mutation) else {
                continue;
            };
            let needle = shape.apply(&mutated);
            let Some(found) = search(file, needle) else {
                tally.skipped += 1;
                continue;
            };
            tally.searched += 1;
            self.record(file, needle, &found);
            let origin = file.text(lines.clone());
            let right = |candidate: &Candidate| {
                let span = candidate.line - 1..candidate.end_line;
                span == lines || (span.end <= file.lines.len() && file.text(span) == origin)
            };
            match found.iter().position(right) {
                Some(0) => {
                    tally.hits += 1;
                    tally.top3 += 1;
                    continue;
                }
                Some(_) => tally.top3 += 1,
                None if found.is_empty() => tally.none += 1,
                None => {}
            }
            if shown < self.examples {
                shown += 1;
                let first = found.first().map_or_else(
                    || "nothing".to_owned(),
                    |first| {
                        format!(
                            "{}% line {}: {}",
                            first.similarity.unwrap_or_default(),
                            first.line,
                            quoted(first.text.as_deref().unwrap_or("<long>"))
                        )
                    },
                );
                self.shown.push(format!(
                    "  {mutation} {}: {}:{}\n    origin {}\n    needle {}\n    first  {first}",
                    shape.label(),
                    file.path,
                    lines.start + 1,
                    quoted(origin),
                    quoted(needle)
                ));
            }
        }
        tally
    }
}

fn percent(part: usize, whole: usize) -> f64 {
    100.0 * part as f64 / whole.max(1) as f64
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut examples = 0;
    while let Some(argument) = arguments.next() {
        if argument == "--examples" {
            examples = arguments
                .next()
                .and_then(|count| count.parse().ok())
                .unwrap_or(5);
        } else {
            root = PathBuf::from(argument);
        }
    }
    let files = corpus(&root);
    let mut by_language: BTreeMap<&str, Vec<&File>> = BTreeMap::new();
    for file in &files {
        by_language.entry(file.language).or_default().push(file);
    }
    println!(
        "Corpus: {} files, {} lines, {} worth searching for",
        files.len(),
        files.iter().map(|file| file.lines.len()).sum::<usize>(),
        files.iter().map(|file| file.useful.len()).sum::<usize>()
    );
    let mut report = Report {
        rng: Rng(SEED),
        examples,
        digest: 0xcbf2_9ce4_8422_2325,
        shown: Vec::new(),
    };

    println!("\nFalse positives: needles from one file searched in another of its language");
    println!(
        "{:<9}{:<15}{:>9}{:>8}{:>8}{:>8}   similarity 70-74 75-79 80-84 85-89 90-94 95+",
        "language", "shape", "searched", "skipped", "similar", "rate"
    );
    let mut totals: BTreeMap<Shape, Tally> = BTreeMap::new();
    let groups: Vec<(&str, &Vec<&File>)> = by_language
        .iter()
        .filter(|(_, group)| group.len() > 1)
        .map(|(language, group)| (*language, group))
        .collect();
    for (language, group) in &groups {
        for shape in SHAPES {
            let tally = report.false_positives(group, group, shape, UNRELATED);
            print_false_positives(language, shape, &tally);
            totals.entry(shape).or_default().add(&tally);
        }
    }
    for shape in SHAPES {
        print_false_positives("all", shape, &totals[&shape]);
    }

    println!("\nRecall: mutated needles searched in their own file");
    println!(
        "{:<9}{:<15}{:>9}{:>8}{:>8}{:>8}{:>8}",
        "mutation", "shape", "searched", "skipped", "top-1", "top-3", "none"
    );
    let all: Vec<&File> = files.iter().collect();
    let mut totals: BTreeMap<Shape, Tally> = BTreeMap::new();
    for mutation in MUTATIONS {
        for shape in SHAPES {
            let tally = report.recall(&all, mutation, shape);
            print_recall(mutation, shape, &tally);
            totals.entry(shape).or_default().add(&tally);
        }
    }
    for shape in SHAPES {
        print_recall("all", shape, &totals[&shape]);
    }

    println!(
        "\nFalse positives in large files: needles from every other file of a language searched in the rest, joined"
    );
    println!(
        "{:<9}{:<15}{:>9}{:>8}{:>8}{:>8}   similarity 70-74 75-79 80-84 85-89 90-94 95+",
        "language", "shape", "searched", "skipped", "similar", "rate"
    );
    report.rng = Rng(SEED ^ 0x1a26e);
    let mut totals: BTreeMap<Shape, Tally> = BTreeMap::new();
    for (language, group) in &groups {
        let halves = [joined(group, 0), joined(group, 1)];
        for shape in SHAPES {
            let mut tally = Tally::default();
            for half in 0..2 {
                let sources: Vec<&File> = group.iter().skip(half).step_by(2).copied().collect();
                let target = [&halves[1 - half]];
                tally.add(&report.false_positives(&sources, &target, shape, LARGE / 2));
            }
            print_false_positives(language, shape, &tally);
            totals.entry(shape).or_default().add(&tally);
        }
    }
    for shape in SHAPES {
        print_false_positives("all", shape, &totals[&shape]);
    }
    if !report.shown.is_empty() {
        println!("\nExamples: false positives, then recall misses");
        for example in &report.shown {
            println!("{example}");
        }
    }
    println!("\nDigest of every search result: {:016x}", report.digest);
}

fn print_false_positives(language: &str, shape: Shape, tally: &Tally) {
    let scores: Vec<String> = tally
        .scores
        .iter()
        .map(|count| format!("{count:>5}"))
        .collect();
    println!(
        "{language:<9}{:<15}{:>9}{:>8}{:>8}{:>7.1}%              {}",
        shape.label(),
        tally.searched,
        tally.skipped,
        tally.hits,
        percent(tally.hits, tally.searched),
        scores.join(" ")
    );
}

fn print_recall(mutation: &str, shape: Shape, tally: &Tally) {
    println!(
        "{mutation:<9}{:<15}{:>9}{:>8}{:>7.1}%{:>7.1}%{:>7.1}%",
        shape.label(),
        tally.searched,
        tally.skipped,
        percent(tally.hits, tally.searched),
        percent(tally.top3, tally.searched),
        percent(tally.none, tally.searched)
    );
}
