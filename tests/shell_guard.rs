use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::shell_guard::{ESCAPE_HATCH, Finding, Pattern, classify, deny_reason};

use Pattern::{AppliedContent, GeneratedToFile, HeredocToFile, InPlaceEdit, InlineScriptWrite};

const DENIED: &[(&str, Pattern, &str)] = &[
    // Heredocs and here-strings whose content reaches a file.
    (
        "cat > notes.txt <<'EOF'\nC:\\temp\\new\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat <<EOF > src/config.rs\nconst A: u8 = 1;\nEOF\n",
        HeredocToFile,
        "cat",
    ),
    (
        "cat << \"EOF\" >> CHANGELOG.md\n- entry\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat <<-EOF >| out.txt\n\tindented\n\tEOF",
        HeredocToFile,
        "cat",
    ),
    ("cat <<\\EOF 1> out.txt\n$HOME\nEOF", HeredocToFile, "cat"),
    (
        "cat >out.txt 2>/dev/null <<'EOF'\nx\nEOF",
        HeredocToFile,
        "cat",
    ),
    ("cat <<'EOF' &> out.log\nx\nEOF", HeredocToFile, "cat"),
    ("cat <<'EOF' &>> out.log\nx\nEOF", HeredocToFile, "cat"),
    (
        "tee src/lib.rs <<'EOF' > /dev/null\npub fn f() {}\nEOF",
        HeredocToFile,
        "tee",
    ),
    (
        "cat <<'EOF' | tee -a notes.md\nline\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat <<'EOF' | sudo tee /etc/app.conf > /dev/null\nkey=value\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat <<'EOF' | sed 's/a/b/' > out.txt\nabc\nEOF",
        HeredocToFile,
        "cat",
    ),
    ("cat <<< 'content' > file.txt", HeredocToFile, "cat"),
    ("tee out.txt <<< \"$value\"", HeredocToFile, "tee"),
    (
        "cd src && cat > mod.rs <<'EOF'\nmod a;\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat > notes.txt <<'EOF'\r\nC:\\path\r\nEOF\r\ngit status\r\n",
        HeredocToFile,
        "cat",
    ),
    (
        "mkdir -p out; cat > out/a.txt <<EOF\none\nEOF\ncat > out/b.txt <<EOF\ntwo\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "if true; then cat > x.txt <<'EOF'\nbody\nEOF\nfi",
        HeredocToFile,
        "cat",
    ),
    (
        "envsubst <<'EOF' > config.yaml\nname: $NAME\nEOF",
        HeredocToFile,
        "envsubst",
    ),
    (
        "base64 -d <<'EOF' > logo.png\niVBORw0KGgo=\nEOF",
        HeredocToFile,
        "base64",
    ),
    // Generated content redirected or tee'd into a file.
    ("echo 'fn main() {}' > src/main.rs", GeneratedToFile, "echo"),
    (
        "printf '%s\\n' 'a\\b' >> notes.txt",
        GeneratedToFile,
        "printf",
    ),
    (
        "echo \"export PATH=$PATH:~/bin\" >> ~/.bashrc",
        GeneratedToFile,
        "echo",
    ),
    ("echo done > \"$OUT\"", GeneratedToFile, "echo"),
    (
        "echo '127.0.0.1 app' | sudo tee -a /etc/hosts",
        GeneratedToFile,
        "echo",
    ),
    ("FOO=bar echo x >out.txt", GeneratedToFile, "echo"),
    (
        "ls && echo ok > status.txt || true",
        GeneratedToFile,
        "echo",
    ),
    ("echo x >& out.txt", GeneratedToFile, "echo"),
    // Inline interpreter code that writes files.
    (
        "python3 -c \"open('a.txt', 'w').write('x')\"",
        InlineScriptWrite,
        "python3",
    ),
    (
        "python - <<'PY'\nfrom pathlib import Path\nPath('a.txt').write_text('C:\\\\x')\nPY",
        InlineScriptWrite,
        "python",
    ),
    (
        "python3 <<'EOF'\nwith open(path, mode='a', encoding='utf-8') as f:\n    f.write(line)\nEOF",
        InlineScriptWrite,
        "python3",
    ),
    (
        "py -3 -c \"import io; io.open('x', 'wb')\"",
        InlineScriptWrite,
        "py",
    ),
    (
        "python3 -c \"$(cat <<'EOF'\nopen('out.txt', 'x').write('y')\nEOF\n)\"",
        InlineScriptWrite,
        "python3",
    ),
    (
        "node -e \"require('fs').writeFileSync('a.json', '{}')\"",
        InlineScriptWrite,
        "node",
    ),
    (
        "node <<'EOF'\nconst fs = require('fs');\nfs.appendFileSync('log.txt', 'x');\nEOF",
        InlineScriptWrite,
        "node",
    ),
    (
        "node --eval=\"fs.createWriteStream('o.txt')\"",
        InlineScriptWrite,
        "node",
    ),
    (
        "perl -e 'open(my $f, \">\", \"out.txt\") or die; print $f \"x\"'",
        InlineScriptWrite,
        "perl",
    ),
    (
        "perl -e 'open F, \">>log.txt\"; print F 1'",
        InlineScriptWrite,
        "perl",
    ),
    (
        "ruby -e 'File.write(\"a.txt\", \"x\")'",
        InlineScriptWrite,
        "ruby",
    ),
    (
        "ruby -e 'File.open(\"a.txt\", \"w\") { |f| f.puts 1 }'",
        InlineScriptWrite,
        "ruby",
    ),
    (
        "php -r 'file_put_contents(\"a.txt\", \"x\");'",
        InlineScriptWrite,
        "php",
    ),
    (
        "pwsh -NoProfile -Command \"Set-Content -Path a.txt -Value 'C:\\x'\"",
        InlineScriptWrite,
        "pwsh",
    ),
    (
        "powershell.exe -c \"'x' | Out-File a.txt\"",
        InlineScriptWrite,
        "powershell",
    ),
    (
        "pwsh -c '[IO.File]::WriteAllText(\"a.txt\", \"x\")'",
        InlineScriptWrite,
        "pwsh",
    ),
    (
        "pwsh -cwa 'Set-Content a.txt $args[0]' x",
        InlineScriptWrite,
        "pwsh",
    ),
    (
        "cat <<'EOF' | python3\nopen('a.txt','w').write('x')\nEOF",
        InlineScriptWrite,
        "python3",
    ),
    // In-place editors, including through wrappers.
    ("sed -i 's/old/new/' src/lib.rs", InPlaceEdit, "sed"),
    ("sed -i.bak -e 's/a/b/' file.txt", InPlaceEdit, "sed"),
    ("sed -ni 's/a/b/p' file.txt", InPlaceEdit, "sed"),
    ("sed -E --in-place=.orig 's/a/b/' f", InPlaceEdit, "sed"),
    ("sed 's/a/b/' -i f.txt", InPlaceEdit, "sed"),
    ("sed -i '' 's/a/b/' f.txt", InPlaceEdit, "sed"),
    ("perl -pi -e 's/foo/bar/g' *.rs", InPlaceEdit, "perl"),
    ("perl -i.bak -pe 's/a/b/' f", InPlaceEdit, "perl"),
    ("perl -0777 -pi -e 's/a\\nb/c/' f", InPlaceEdit, "perl"),
    ("ruby -pi -e 'gsub(/a/, \"b\")' f.txt", InPlaceEdit, "ruby"),
    (
        "gawk -i inplace '{ print toupper($0) }' f.txt",
        InPlaceEdit,
        "gawk",
    ),
    (
        "sudo -u app env LC_ALL=C sed -i 's/a/b/' /srv/app.conf",
        InPlaceEdit,
        "sed",
    ),
    ("/usr/bin/sed -i 's/a/b/' f", InPlaceEdit, "sed"),
    (
        "grep -rl foo src | xargs sed -i 's/foo/bar/g'",
        InPlaceEdit,
        "sed",
    ),
    (
        "find . -name '*.rs' -exec sed -i 's/a/b/' {} +",
        InPlaceEdit,
        "sed",
    ),
    ("timeout 5 nice -n 10 sed -i 's/a/b/' f", InPlaceEdit, "sed"),
    ("sed.exe -i s/a/b/ f", InPlaceEdit, "sed"),
    // Scripts run through a shell, eval, or a substitution.
    (
        "bash -c 'cat > out.txt <<EOF\nx\nEOF'",
        HeredocToFile,
        "cat",
    ),
    ("sh -c \"sed -i 's/a/b/' f\"", InPlaceEdit, "sed"),
    ("bash -lc 'echo x > y.txt'", GeneratedToFile, "echo"),
    (
        "bash -euo pipefail -c 'printf x > y'",
        GeneratedToFile,
        "printf",
    ),
    (
        "bash <<'EOF'\nset -e\nsed -i 's/a/b/' f\nEOF",
        InPlaceEdit,
        "sed",
    ),
    ("eval \"echo x > out.txt\"", GeneratedToFile, "echo"),
    ("result=$(sed -i 's/a/b/' f && echo ok)", InPlaceEdit, "sed"),
    ("echo `sed -i s/a/b/ f`", InPlaceEdit, "sed"),
    // Embedded patches and edit requests applied to files.
    (
        "git apply <<'EOF'\ndiff --git a/f b/f\nEOF",
        AppliedContent,
        "git",
    ),
    (
        "patch -p1 <<'EOF'\n--- a/f\n+++ b/f\nEOF",
        AppliedContent,
        "patch",
    ),
    (
        "ultra-edit edit <<'EOF'\n{\"request_id\":\"r\"}\nEOF",
        AppliedContent,
        "ultra-edit",
    ),
    (
        "echo '{}' | ultra-edit --root . prepare",
        AppliedContent,
        "ultra-edit",
    ),
];

const ALLOWED: &[&str] = &[
    // Ordinary output, devices, and descriptor duplication.
    "cargo test > log.txt 2>&1",
    "git diff > x.patch",
    "git log --format='%H > %s' > log.txt",
    "cat file1.txt file2.txt > combined.txt",
    "cat < in.txt > out.txt",
    "echo hi >&2",
    "echo hi 1>&2",
    "echo hi > /dev/null",
    "echo hi >/dev/stderr",
    "echo hi > NUL",
    "echo hi 2>nul",
    "echo hi > >(logger -t build)",
    "ls > /dev/null 2>&1",
    "npm test 2>&1 | tee test.log",
    "docker build . 2>&1 | tee build.log",
    "echo hi | tee /dev/null",
    "cat <<EOF >&2\nwarning\nEOF",
    "time cargo build > build.log",
    "env | sort > env.txt",
    "exec 3>&1",
    ": > empty.txt",
    "touch a.txt && cp a.txt b.txt",
    "curl -sS -o page.html https://example.com",
    "git status\r\ncargo test > log.txt 2>&1\r\n",
    // Heredocs to commands that do not write their content to files.
    "git commit -F - <<'EOF'\nSubject\n\nBody with C:\\path\nEOF",
    "git commit -m \"$(cat <<'EOF'\nFix: don't break (things)\nEOF\n)\"",
    "gh pr create --title t --body \"$(cat <<'EOF'\n## Summary\n- a > b\nEOF\n)\"",
    "cat <<EOF | kubectl apply -f -\napiVersion: v1\nEOF",
    "cat <<'EOF' | kubectl apply -f -\nsed -i 's/a/b/' f > out.txt\nEOF",
    "cat <<'EOF'\njust display\nEOF",
    "cat <<< \"$x\" | wc -l",
    "psql <<'SQL' > result.txt\nselect 1;\nSQL",
    "git apply --check <<'EOF'\ndiff\nEOF",
    "git apply fix.patch",
    "patch --dry-run -p1 < fix.patch",
    "ultra-edit read src/lib.rs",
    // Interpreters without file-write calls.
    "python3 - <<'EOF'\nprint(open('data.txt').read())\nEOF",
    "python3 - <<'EOF' > report.txt\nprint('summary')\nEOF",
    "python3 -c \"import json,sys; print(json.load(open('a.json')))\"",
    "python -m json.tool <<'EOF'\n{\"a\": 1}\nEOF",
    "python3 script.py <<'EOF'\nopen('x', 'w')\nEOF",
    "python -i script.py",
    "cat <<'EOF' | python3\nprint('hi')\nEOF",
    "echo 'print(1)' | python3",
    "node -e \"console.log(fs.readFileSync('a.txt', 'utf8'))\"",
    "node -i",
    "perl -ne 'print if />/' file.txt",
    "perl -e 'open(my $f, \"<\", \"in.txt\") or die'",
    "perl -v",
    "ruby -e 'puts File.read(\"a.txt\")'",
    "ruby -e 'puts 1' > out.txt",
    "pwsh -NoProfile -Command \"Get-Content a.txt\"",
    "pwsh -NoProfile -ec UwBlAHQALQBDAG8AbgB0AGUAbgB0AA==",
    // Stream editors without in-place flags.
    "sed -n '1,20p' src/lib.rs",
    "sed -e 's/i/I/' -e 's/in-place/x/' file",
    "sed --expression='s/-i/x/' file",
    "sed -es/i/x/ file",
    "sed 's/a/b/' input.txt > output.txt",
    "gawk -f script.awk data.txt",
    "awk -F: '{print $1}' /etc/passwd",
    "awk '$1 > 5 { print }' data.txt",
    // Syntax inside quotes, comments, escapes, and arithmetic.
    "grep -n '<<' file.txt",
    "grep '>' file.txt",
    "echo \"a > b\"",
    "echo \"unterminated > quote",
    "echo 'x' # > file.txt",
    "echo a\\>b",
    "echo $((1 << 4))",
    "(( x = 1 << 4 )); echo $x",
    "[[ \"$a\" > \"$b\" ]] && echo sorted",
    "rg 'sed -i' src/",
    "echo \"sed -i s/a/b/ f\"",
    "echo \"$(git rev-parse HEAD)\"",
    "diff <(sort a.txt) <(sort b.txt)",
    // Wrappers and nested shells that do not write files.
    "bash -c 'echo hi'",
    "bash script.sh > run.log 2>&1",
    "sh -c \"cargo test > /dev/null\"",
    "ssh host 'sed -i s/a/b/ f'",
    "find . -name '*.log' -exec rm {} \\;",
    "xargs -n1 echo < list.txt",
    "sudo -v",
    "printf '%s\\n' a b | sort",
    "echo y | apt-get install foo > install.log",
];

#[test]
fn classify_denies_file_writes_through_the_shell() {
    for (command, pattern, program) in DENIED {
        let expected = Finding {
            pattern: *pattern,
            program: (*program).to_owned(),
        };
        assert_eq!(classify(command), Some(expected), "{command:?}");
    }
}

#[test]
fn classify_allows_commands_without_embedded_file_writes() {
    for command in ALLOWED {
        assert_eq!(classify(command), None, "{command:?}");
    }
}

#[test]
fn deny_reasons_name_the_pattern_and_alternatives_briefly() {
    for pattern in [
        HeredocToFile,
        GeneratedToFile,
        InlineScriptWrite,
        InPlaceEdit,
        AppliedContent,
    ] {
        let finding = Finding {
            pattern,
            program: "a-very-long-program-name".to_owned(),
        };
        let reason = deny_reason(&finding);
        assert!(
            reason.chars().count() <= 400,
            "{} chars: {reason}",
            reason.len()
        );
        for expected in [
            "`a-very-long-prog`",
            "ultra_edit",
            "Write",
            "report that the Ultra Edit guard blocked it",
            "ULTRA_EDIT_SHELL_WRITES=allow",
        ] {
            assert!(reason.contains(expected), "{expected}: {reason}");
        }
    }
}

#[test]
fn classify_finishes_on_pathological_input() {
    let nested = format!("echo {}x{}", "$(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(classify(&nested), None);
    let calls = format!("python3 -c \"{}\"", "open(".repeat(50_000));
    assert_eq!(classify(&calls), None);
    let unterminated = format!("cat <<EOF > out.txt\n{}", "line\n".repeat(50_000));
    assert_eq!(
        classify(&unterminated).map(|finding| finding.pattern),
        Some(HeredocToFile)
    );
    for command in [
        "", "\\", "'", "\"$(", "$'\\", "<<", "2>", "`", "${", "&>", "|&",
    ] {
        assert_eq!(classify(command), None, "{command:?}");
    }
}

fn hook(input: &[u8], escape_hatch: Option<&str>) -> Output {
    let directory = TempDir::new().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"));
    command
        .args(["--claude-hook", "PreToolUse"])
        .current_dir(directory.path())
        .env_remove(ESCAPE_HATCH)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(value) = escape_hatch {
        command.env(ESCAPE_HATCH, value);
    }
    let mut child = command.spawn().unwrap();
    // The guard may exit before reading everything; that is not a failure here.
    let _ = child.stdin.take().unwrap().write_all(input);
    let output = child.wait_with_output().unwrap();
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
    output
}

fn bash_event(command: &str) -> Vec<u8> {
    json!({
        "session_id": "abc123",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp/project",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": command,
            "description": "Write notes",
            "timeout": 120000,
            "run_in_background": false,
        },
        "tool_use_id": "toolu_01",
    })
    .to_string()
    .into_bytes()
}

fn assert_silent_success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn hook_prints_a_documented_deny_decision() {
    let output = hook(&bash_event("cat > notes.txt <<'EOF'\nC:\\temp\nEOF"), None);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.ends_with('\n'));
    let value: Value = serde_json::from_str(&stdout).unwrap();
    let reason = value["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        value,
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }})
    );
    assert!(reason.starts_with("Ultra Edit guard blocked a heredoc written to a file by `cat`"));
    assert!(reason.chars().count() <= 400);
}

#[test]
fn hook_allows_other_commands_and_tools_silently() {
    let other_tool = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/a.txt", "content": "x", "command": "sed -i s/a/b/ f"},
    });
    let powershell = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "PowerShell",
        "tool_input": {"command": "echo x > out.txt"},
    });
    for input in [
        bash_event("cargo test > log.txt 2>&1"),
        bash_event("git commit -F - <<'EOF'\nSubject\nEOF"),
        other_tool.to_string().into_bytes(),
        powershell.to_string().into_bytes(),
    ] {
        assert_silent_success(&hook(&input, None));
    }
}

#[test]
fn hook_fails_open_on_malformed_input() {
    let inputs: [&[u8]; 10] = [
        b"",
        b"{",
        b"not json",
        b"[]",
        b"\"Bash\"",
        b"null",
        b"\xff\xfe\xfd",
        b"{\"tool_name\":\"Bash\"}",
        b"{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":42}}",
        b"{\"tool_name\":\"Bash\",\"tool_input\":\"echo x > out.txt\"}",
    ];
    for input in inputs {
        assert_silent_success(&hook(input, None));
    }
    let mut truncated = bash_event("echo x > out.txt");
    truncated.pop();
    assert_silent_success(&hook(&truncated, None));
}

#[test]
fn escape_hatch_allows_shell_writes() {
    let event = bash_event("sed -i 's/a/b/' src/lib.rs");
    assert_silent_success(&hook(&event, Some("allow")));
    for value in ["", "1", "yes", "ALLOW"] {
        let output = hook(&event, Some(value));
        assert!(output.status.success(), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "deny");
    }
}

#[test]
fn hook_mode_rejects_other_arguments_and_is_documented() {
    for args in [
        vec!["--claude-hook"],
        vec!["--claude-hook", "SessionStart"],
        vec!["--claude-hook", "PostToolUse"],
        vec!["--claude-hook", "PreToolUse", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
            .args(args)
            .output()
            .unwrap();
        // Exit code 2 would make Claude Code block every Bash call.
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--claude-hook PreToolUse"), "{help}");
    assert!(help.contains("ULTRA_EDIT_SHELL_WRITES=allow"), "{help}");
}
