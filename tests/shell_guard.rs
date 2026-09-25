use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;
use ultra_edit::shell_guard::{
    ESCAPE_HATCH, Finding, Pattern, SCOPE_VARIABLES, Scope, classify, classify_bash,
    classify_powershell, deny_reason, is_absolute,
};

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
            "backslashes, line endings, or encoding",
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

/// Runs the hook with the escape hatch and the scope variables unset, apart
/// from those in `environment`.
fn hook(input: &[u8], environment: &[(&str, &str)]) -> Output {
    let directory = TempDir::new().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ultra-edit-mcp"));
    command
        .args(["--claude-hook", "PreToolUse"])
        .current_dir(directory.path())
        .env_remove(ESCAPE_HATCH)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in SCOPE_VARIABLES {
        command.env_remove(name);
    }
    command.envs(environment.iter().copied());
    let mut child = command.spawn().unwrap();
    // The guard may exit before reading everything; that is not a failure here.
    let _ = child.stdin.take().unwrap().write_all(input);
    let output = child.wait_with_output().unwrap();
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
    output
}

/// A `PreToolUse` event for a shell tool, as Claude Code sends it.
fn event(tool: &str, command: &str, cwd: Option<&str>) -> Vec<u8> {
    let mut event = json!({
        "session_id": "abc123",
        "transcript_path": "/tmp/transcript.jsonl",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": {
            "command": command,
            "description": "Write notes",
            "timeout": 120000,
            "run_in_background": false,
        },
        "tool_use_id": "toolu_01",
    });
    if let Some(cwd) = cwd {
        event["cwd"] = json!(cwd);
    }
    event.to_string().into_bytes()
}

fn bash_event(command: &str) -> Vec<u8> {
    event("Bash", command, Some("/tmp/project"))
}

/// Whether the hook's output is a deny decision.
fn denied(output: &Output) -> bool {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    if output.stdout.is_empty() {
        return false;
    }
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "deny");
    true
}

fn assert_silent_success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn hook_prints_a_documented_deny_decision() {
    let output = hook(&bash_event("cat > notes.txt <<'EOF'\nC:\\temp\nEOF"), &[]);
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
    let mut inputs = vec![
        bash_event("cargo test > log.txt 2>&1"),
        bash_event("git commit -F - <<'EOF'\nSubject\nEOF"),
        event(
            "PowerShell",
            "Get-Content a.txt | Select-Object -First 5",
            None,
        ),
        other_tool.to_string().into_bytes(),
    ];
    // Tool names match exactly; only Bash and PowerShell are classified.
    for tool in ["bash", "powershell", "Monitor", "Edit", ""] {
        inputs.push(event(tool, "echo x > out.txt", Some("/tmp/project")));
    }
    for input in inputs {
        assert_silent_success(&hook(&input, &[]));
    }
}

#[test]
fn hook_fails_open_on_malformed_input() {
    let inputs: [&[u8]; 14] = [
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
        b"{\"tool_name\":\"PowerShell\"}",
        b"{\"tool_name\":\"PowerShell\",\"tool_input\":{\"command\":null}}",
        b"{\"tool_name\":[\"PowerShell\"],\"tool_input\":{\"command\":\"'x' > a.txt\"}}",
        b"{\"tool_name\":\"PowerShell\",\"tool_input\":{\"command\":\"\\\"unterminated > a.txt\"}}",
    ];
    for input in inputs {
        assert_silent_success(&hook(input, &[]));
    }
    let mut truncated = bash_event("echo x > out.txt");
    truncated.pop();
    assert_silent_success(&hook(&truncated, &[]));
    let mut truncated = event("PowerShell", "'x' > out.txt", Some("/tmp/project"));
    truncated.pop();
    assert_silent_success(&hook(&truncated, &[]));
}

#[test]
fn escape_hatch_allows_shell_writes() {
    for event in [
        bash_event("sed -i 's/a/b/' src/lib.rs"),
        event(
            "PowerShell",
            "'x' | Set-Content a.txt",
            Some("/tmp/project"),
        ),
    ] {
        assert_silent_success(&hook(&event, &[(ESCAPE_HATCH, "allow")]));
        for value in ["", "1", "yes", "ALLOW"] {
            assert!(denied(&hook(&event, &[(ESCAPE_HATCH, value)])));
        }
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
    assert!(help.contains("PowerShell"), "{help}");
    assert!(help.contains("CLAUDE_PROJECT_DIR"), "{help}");
}

/// A project at `/work/repo`; the home and temporary directories lie outside.
fn unix_scope() -> Scope {
    Scope::new("/work/repo")
        .with_variable("CLAUDE_PROJECT_DIR", "/work/repo")
        .with_variable("HOME", "/home/dev")
        .with_variable("TMPDIR", "/tmp/")
}

/// A project at `C:\repo`; the profile and temporary directories lie outside.
fn windows_scope() -> Scope {
    Scope::new("C:\\repo")
        .with_variable("CLAUDE_PROJECT_DIR", "C:\\repo")
        .with_variable("USERPROFILE", "C:\\Users\\dev")
        .with_variable("TEMP", "C:\\Users\\dev\\AppData\\Local\\Temp")
        .with_variable("TMP", "C:\\Users\\dev\\AppData\\Local\\Temp")
}

/// Bash writes that `unix_scope` places certainly outside the project.
const OUTSIDE: &[&str] = &[
    // CI runner files.
    "echo \"k=v\" >> \"$GITHUB_OUTPUT\"",
    "echo k=v >> $GITHUB_ENV",
    "echo /opt/tool/bin >> ${GITHUB_PATH}",
    "printf '## Done\\n' >> \"${GITHUB_STEP_SUMMARY}\"",
    "echo k=v >> $GITHUB_STATE",
    // System, home, and temporary files.
    "echo x | sudo tee /etc/hosts",
    "echo '127.0.0.1 app' | sudo tee -a /etc/hosts > /dev/null",
    "echo 'export A=1' >> ~/.bashrc",
    "echo 'export A=1' >> \"$HOME/.bashrc\"",
    "echo 'export A=1' >> ${HOME}/.profile",
    "cat > /tmp/payload.json <<'EOF'\n{\"a\": 1}\nEOF",
    "cat > \"$TMPDIR/payload.json\" <<'EOF'\n{}\nEOF",
    "cat <<'EOF' > ${TMPDIR}request.json\n{}\nEOF",
    "cat > '/tmp/[x].json' <<'EOF'\n{}\nEOF",
    "echo x > /work/repo2/a.txt",
    "echo x > /work/repo/../other/a.txt",
    "echo x > \"$CLAUDE_PROJECT_DIR/../other/a.txt\"",
    // In-place editors whose every file operand is outside.
    "sed -i 's/a/b/' /etc/hosts",
    "sudo sed -i.bak -e 's/a/b/' /etc/hosts /etc/hostname",
    "perl -pi -e 's/a/b/' /etc/hosts",
    "gawk -i inplace '{ print }' ~/notes.txt",
    "sed --in-pl --expr='s/a/b/' /etc/hosts",
    "gawk --include inplace -f fix.awk /etc/hosts",
];

/// Bash writes that `windows_scope` places certainly outside the project.
const OUTSIDE_WINDOWS: &[&str] = &[
    "cat > 'D:\\scratch\\payload.json' <<'EOF'\n{}\nEOF",
    "cat > /d/scratch/payload.json <<'EOF'\n{}\nEOF",
    "echo x > 'C:\\Users\\dev\\notes.txt'",
    "echo x > C:/Users/dev/notes.txt",
    "echo x > '\\\\server\\share\\notes.txt'",
    "echo x > //server/share/notes.txt",
    "echo x > 'C:\\repo2\\notes.txt'",
    "echo x > /c/repo/../other/notes.txt",
    "sed -i 's/a/b/' 'C:\\Windows\\System32\\drivers\\etc\\hosts'",
];

/// Bash writes that `unix_scope` cannot place outside the project.
const INSIDE: &[(&str, Pattern, &str)] = &[
    (
        "cat > src/a.rs <<'EOF'\nfn a() {}\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat > /work/repo/src/a.rs <<'EOF'\nx\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat > \"$CLAUDE_PROJECT_DIR/src/a.rs\" <<'EOF'\nx\nEOF",
        HeredocToFile,
        "cat",
    ),
    (
        "cat > ${CLAUDE_PROJECT_DIR}/src/a.rs <<'EOF'\nx\nEOF",
        HeredocToFile,
        "cat",
    ),
    ("echo x > ../x.txt", GeneratedToFile, "echo"),
    ("echo x > /work/repo", GeneratedToFile, "echo"),
    (
        "echo x > /work/./repo/src/../a.txt",
        GeneratedToFile,
        "echo",
    ),
    ("echo x > /WORK/Repo/a.txt", GeneratedToFile, "echo"),
    ("echo x > ~/../../work/repo/a.txt", GeneratedToFile, "echo"),
    ("echo x > \"$UNSET_DIR/a.txt\"", GeneratedToFile, "echo"),
    ("echo x > $OUT", GeneratedToFile, "echo"),
    ("echo x > \"$(mktemp)\"", GeneratedToFile, "echo"),
    ("echo x > /tmp/*.txt", GeneratedToFile, "echo"),
    ("echo x > /tmp/{a,b}.txt", GeneratedToFile, "echo"),
    ("echo x > '~/notes.txt'", GeneratedToFile, "echo"),
    ("echo x > '$HOME/notes.txt'", GeneratedToFile, "echo"),
    ("echo x > \"$GITHUB_OUTPUT.bak\"", GeneratedToFile, "echo"),
    ("echo x | tee /etc/hosts src/a.txt", GeneratedToFile, "echo"),
    ("sed -i 's/a/b/' /etc/hosts src/lib.rs", InPlaceEdit, "sed"),
    ("sed -i 's/a/b/'", InPlaceEdit, "sed"),
    // An abbreviated `--expression` still leaves every operand a file.
    (
        "sed -i --expr='s/a/b/' src/lib.rs /etc/hosts",
        InPlaceEdit,
        "sed",
    ),
    (
        "gawk --incl=inplace '{ print }' src/a.txt",
        InPlaceEdit,
        "gawk",
    ),
    ("perl -pi -e 's/a/b/' /etc/hosts ./x", InPlaceEdit, "perl"),
    (
        "find /etc -name hosts -exec sed -i 's/a/b/' {} +",
        InPlaceEdit,
        "sed",
    ),
    // Inline interpreter writes are flagged whatever the path.
    (
        "python3 -c \"open('/tmp/x.txt', 'w').write('x')\"",
        InlineScriptWrite,
        "python3",
    ),
];

/// Bash writes that `windows_scope` cannot place outside the project.
const INSIDE_WINDOWS: &[&str] = &[
    "cat > 'C:\\repo\\src\\a.rs' <<'EOF'\nx\nEOF",
    "cat > C:\\\\repo\\\\src\\\\a.rs <<'EOF'\nx\nEOF",
    "cat > 'c:\\REPO\\Src\\a.rs' <<'EOF'\nx\nEOF",
    "cat > C:/repo/src/a.rs <<'EOF'\nx\nEOF",
    "cat > /c/repo/src/a.rs <<'EOF'\nx\nEOF",
    "cat > /C/Repo/src/a.rs <<'EOF'\nx\nEOF",
    "cat > '\\\\?\\C:\\repo\\a.rs' <<'EOF'\nx\nEOF",
    "cat > 'C:\\repo.\\a.rs' <<'EOF'\nx\nEOF",
    // Unquoted, Bash reads `\t` as `t`: `C:tempx.txt` is drive-relative.
    "cat > C:\\temp\\x.txt <<'EOF'\nx\nEOF",
    // Git Bash's mount table decides where `/tmp` is.
    "cat > /tmp/payload.json <<'EOF'\nx\nEOF",
    // No HOME was passed in.
    "echo x > ~/notes.txt",
];

#[test]
fn scoped_bash_allows_writes_certainly_outside_the_project() {
    for (scope, commands) in [(unix_scope(), OUTSIDE), (windows_scope(), OUTSIDE_WINDOWS)] {
        for command in commands {
            // Every one is a shell write that an unscoped guard blocks.
            assert!(classify(command).is_some(), "{command:?}");
            assert_eq!(classify_bash(command, &scope), None, "{command:?}");
        }
    }
    let temporary = Scope::new("/tmp/repo").with_variable("TMPDIR", "/tmp");
    assert_eq!(
        classify_bash("cat > /tmp/payload.json <<'EOF'\n{}\nEOF", &temporary),
        None
    );
    let unc = Scope::new("\\\\server\\share\\repo");
    assert_eq!(
        classify_bash("echo x > //SERVER/share/other/a.txt", &unc),
        None
    );
}

#[test]
fn scoped_bash_denies_writes_that_may_reach_the_project() {
    let scope = unix_scope();
    for (command, pattern, program) in INSIDE {
        let expected = Finding {
            pattern: *pattern,
            program: (*program).to_owned(),
        };
        assert_eq!(
            classify_bash(command, &scope),
            Some(expected),
            "{command:?}"
        );
    }
    let scope = windows_scope();
    for command in INSIDE_WINDOWS {
        assert!(classify_bash(command, &scope).is_some(), "{command:?}");
    }
    let temporary = Scope::new("/tmp/repo").with_variable("TMPDIR", "/tmp");
    for command in [
        "cat > /tmp/repo/src/a <<'EOF'\nx\nEOF",
        "cat > $TMPDIR/repo/src/a <<'EOF'\nx\nEOF",
    ] {
        assert!(classify_bash(command, &temporary).is_some(), "{command:?}");
    }
    let unc = Scope::new("\\\\server\\share\\repo");
    assert!(classify_bash("echo x > '\\\\SERVER\\Share\\repo\\a.txt'", &unc).is_some());
}

#[test]
fn scope_needs_an_absolute_root_and_known_values() {
    for root in [
        "/",
        "/work/repo",
        "C:\\repo",
        "c:/repo",
        "\\\\server\\share\\repo",
        "\\\\?\\C:\\repo",
    ] {
        assert!(is_absolute(root), "{root:?}");
    }
    for root in [
        "",
        "repo",
        "./repo",
        "C:repo",
        "C:",
        "\\\\server",
        "~/repo",
        "$HOME/repo",
    ] {
        assert!(!is_absolute(root), "{root:?}");
    }
    // Without an absolute root every file target counts, as before scoping;
    // discarded output never does.
    for scope in [Scope::default(), Scope::new("repo"), Scope::new("")] {
        for command in [
            "echo x >> \"$GITHUB_OUTPUT\"",
            "echo x | sudo tee /etc/hosts",
        ] {
            assert!(classify_bash(command, &scope).is_some(), "{command:?}");
        }
        assert!(classify_powershell("'x' >> $env:GITHUB_OUTPUT", &scope).is_some());
        assert_eq!(classify_bash("echo x > /dev/null", &scope), None);
        assert_eq!(classify_powershell("'x' > $null", &scope), None);
    }
    // Unset and empty values, and names outside `SCOPE_VARIABLES`, stay unknown.
    let scope = Scope::new("/work/repo")
        .with_variable("HOME", "")
        .with_variable("OUT", "/tmp/out");
    for command in [
        "echo x >> ~/.bashrc",
        "echo x >> $HOME/.bashrc",
        "echo x >> $TMPDIR/x",
        "echo x >> $OUT",
    ] {
        assert!(classify_bash(command, &scope).is_some(), "{command:?}");
    }
    // Under a POSIX root, PowerShell keeps environment names' case and
    // reads `\` as a separator.
    let scope = Scope::new("/work/repo").with_variable("HOME", "/home/dev");
    assert_eq!(
        classify_powershell("Set-Content $env:HOME/x.txt 'x'", &scope),
        None
    );
    assert_eq!(classify_powershell("'x' > /tmp/x.txt", &scope), None);
    for command in [
        "Set-Content $env:home/x.txt 'x'",
        "'x' > /tmp\\..\\work\\repo\\x.txt",
    ] {
        assert!(
            classify_powershell(command, &scope).is_some(),
            "{command:?}"
        );
    }
}

const DENIED_POWERSHELL: &[(&str, Pattern, &str)] = &[
    // Embedded content written by writers and redirections.
    (
        "@\"\nparam([string]$Name)\nWrite-Output \"C:\\temp\\$Name\"\n\"@ | Set-Content src/a.ps1",
        HeredocToFile,
        "Set-Content",
    ),
    (
        "@'\nline\n'@ | Out-File -FilePath notes.md -Encoding utf8",
        HeredocToFile,
        "Out-File",
    ),
    (
        "Set-Content -Path a.txt -Value 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content a.txt 'C:\\temp\\new'",
        GeneratedToFile,
        "Set-Content",
    ),
    ("sc a.txt -Value:\"x\"", GeneratedToFile, "Set-Content"),
    (
        "Add-Content -Path CHANGELOG.md -Value '- entry'",
        GeneratedToFile,
        "Add-Content",
    ),
    ("ac notes.txt line", GeneratedToFile, "Add-Content"),
    ("'x' > a.txt", GeneratedToFile, ">"),
    ("'x' >> a.txt", GeneratedToFile, ">>"),
    ("'a','b' 1> a.txt", GeneratedToFile, "1>"),
    ("@'\nx\n'@ *> a.txt", HeredocToFile, "*>"),
    (
        "Write-Output 'x' | Out-File a.txt",
        GeneratedToFile,
        "Write-Output",
    ),
    ("echo x > out.txt", GeneratedToFile, "Write-Output"),
    (
        "write 'x' | Out-File -Append a.txt",
        GeneratedToFile,
        "Write-Output",
    ),
    (
        "'x' | Tee-Object -FilePath a.txt | Out-Null",
        GeneratedToFile,
        "Tee-Object",
    ),
    ("'x' | tee a.txt", GeneratedToFile, "Tee-Object"),
    (
        "New-Item -Path a.txt -ItemType File -Value 'x'",
        GeneratedToFile,
        "New-Item",
    ),
    ("ni src/a.txt -Value @'\nx\n'@", HeredocToFile, "New-Item"),
    (
        "Out-File -FilePath a.txt -InputObject 'x'",
        GeneratedToFile,
        "Out-File",
    ),
    (
        "$text = @'\nx\n'@\nSet-Content -Path a.txt -Value $text",
        HeredocToFile,
        "Set-Content",
    ),
    (
        "$lines = 'a', 'b'; $lines | Set-Content a.txt",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "if ($ready) { \"done\" | Out-File status.txt }",
        GeneratedToFile,
        "Out-File",
    ),
    ("git status && 'x' > a.txt", GeneratedToFile, ">"),
    (
        "Set-Content `\n  -Path a.txt `\n  -Value 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    ("'x'\n| Set-Content a.txt", GeneratedToFile, "Set-Content"),
    (
        "Set-Content \u{2013}Path a.txt \u{2013}Value \u{2018}x\u{2019}",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Microsoft.PowerShell.Management\\Set-Content a.txt 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Invoke-Expression \"Set-Content a.txt 'x'\"",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "function Save { Set-Content a.txt $args[0] }",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content 'C:\\REPO\\a.txt' 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content /c/repo/a.txt 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content C:\\Users\\*.txt 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content -Path C:\\Users\\dev\\a.txt, src\\b.txt -Value 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    (
        "Set-Content $PSScriptRoot\\a.txt 'x'",
        GeneratedToFile,
        "Set-Content",
    ),
    // Read-transform-write rewrites.
    (
        "(Get-Content a.txt) -replace 'a','b' | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "(gc a.txt -Raw).Replace('a', 'b') | sc a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "Get-Content a.txt | ForEach-Object { $_ -creplace 'a','b' } | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "cat a.txt | % { $_ } | Out-File a.txt",
        InPlaceEdit,
        "Out-File",
    ),
    (
        "$c = Get-Content a.txt -Raw\n$c = $c -ireplace 'a', 'b'\nSet-Content a.txt $c -NoNewline",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "$lines = Get-Content a.txt; $lines[3] = 'x'; $lines | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "Set-Content a.txt -Value ((Get-Content a.txt) -replace 'a','b')",
        InPlaceEdit,
        "Set-Content",
    ),
    ("$(type a.txt) -replace 'a','b' > a.txt", InPlaceEdit, ">"),
    (
        "[IO.File]::ReadAllText('a.txt') -replace 'a','b' | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "Get-ChildItem *.cs | ForEach-Object { (Get-Content $_) -replace 'a','b' | Set-Content $_ }",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "(Get-Content a.txt) + 'new line' | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "$c = Get-Content a.txt -Raw; $c.Insert(0, \"// header`n\") | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    (
        "[regex]::Replace((Get-Content a.txt -Raw), 'a', 'b') | Set-Content a.txt",
        InPlaceEdit,
        "Set-Content",
    ),
    // .NET write APIs.
    (
        "[IO.File]::WriteAllText(\"a.txt\", \"x\")",
        InlineScriptWrite,
        "PowerShell",
    ),
    (
        "[System.IO.File]::AppendAllLines('a.txt', [string[]]@('x'))",
        InlineScriptWrite,
        "PowerShell",
    ),
    (
        "$w = [IO.File]::CreateText('a.txt'); $w.Write('x'); $w.Close()",
        InlineScriptWrite,
        "PowerShell",
    ),
    (
        "$w = New-Object System.IO.StreamWriter('a.txt')",
        InlineScriptWrite,
        "PowerShell",
    ),
    (
        "$w = New-Object -TypeName IO.StreamWriter -ArgumentList 'a.txt'",
        InlineScriptWrite,
        "PowerShell",
    ),
    (
        "$w = [IO.StreamWriter]::new(\"$PWD\\a.txt\")",
        InlineScriptWrite,
        "PowerShell",
    ),
    // Embedded content applied to files.
    (
        "@'\ndiff --git a/f b/f\n'@ | git apply",
        AppliedContent,
        "git",
    ),
    (
        "'{\"request_id\":\"r\"}' | ultra-edit edit",
        AppliedContent,
        "ultra-edit",
    ),
    (
        "@'\n--- a/f\n+++ b/f\n'@ | patch -p1",
        AppliedContent,
        "patch",
    ),
    // External programs get the Bash checks.
    (
        "python -c \"open('a','w').write('x')\"",
        InlineScriptWrite,
        "python",
    ),
    (
        "& 'C:\\Python312\\python.exe' -c \"open('a','w').write('x')\"",
        InlineScriptWrite,
        "python",
    ),
    (
        "@'\nopen('a.txt', 'w').write('x')\n'@ | python -",
        InlineScriptWrite,
        "python",
    ),
    (
        "node -e \"require('fs').writeFileSync('a.json', '{}')\"",
        InlineScriptWrite,
        "node",
    ),
    ("sed -i 's/a/b/' src/lib.rs", InPlaceEdit, "sed"),
    ("perl -pi -e 's/a/b/' src/lib.rs", InPlaceEdit, "perl"),
    ("bash -c 'cat > a.txt <<EOF\nx\nEOF'", HeredocToFile, "cat"),
    (
        "pwsh -NoProfile -Command \"Set-Content a.txt 'x'\"",
        InlineScriptWrite,
        "pwsh",
    ),
    // PowerShell may already have expanded `$` in a Bash script, so a
    // variable target there is unknown.
    (
        "bash -c 'echo x >> $GITHUB_OUTPUT'",
        GeneratedToFile,
        "echo",
    ),
];

const ALLOWED_POWERSHELL: &[&str] = &[
    // Output capture, discarded output, copies, and reads.
    "git diff > d.patch",
    "cargo test *> log.txt",
    "npm test 2>&1 | Tee-Object -FilePath test.log",
    "Get-ChildItem | Out-File list.txt",
    "Get-Process | Out-File -FilePath procs.txt -Width 200",
    "Get-Content a | Set-Content b",
    "Get-Content a.txt -Raw | Set-Content b.txt -NoNewline",
    "(Get-Content a.txt | Select-Object -First 5) | Set-Content b.txt",
    "(Get-Content a.txt) + (Get-Content b.txt) | Set-Content c.txt",
    "[regex]::Replace($name, '-', '_') | Set-Content name.txt",
    "$c = Get-Content a.txt -Raw; Set-Content b.txt \"$c\"",
    "Copy-Item a.txt b.txt",
    "echo x > $null",
    "'x' | Out-Null",
    "Write-Output 'x' 2> err.txt",
    "'x' | Out-File NUL",
    "Get-Content a.txt | Select-String 'x'",
    "Write-Host \"a > b; Set-Content x y\"",
    "# 'x' > a.txt",
    "<# 'x' > a.txt #> Get-Date",
    "Set-Content -Path a.txt -Value $data",
    "Set-Content -Path a.txt -Value (Get-Date)",
    "$data | ConvertTo-Json | Set-Content config.json",
    "1..10 | ForEach-Object { \"line $_\" } | Set-Content lines.txt",
    "New-Item -ItemType Directory -Force -Path out | Out-Null",
    "New-Item -ItemType SymbolicLink -Path link -Value target",
    "Tee-Object -Variable log",
    "@'\nSubject\n'@ | git commit -F -",
    "git apply --check fix.patch",
    "python -c \"print(open('a.txt').read())\"",
    "sed -n '1,20p' src/lib.rs",
    "Get-Help Set-Content",
    // Writes certainly outside the project.
    "Set-Content $env:TEMP\\x.txt 'x'",
    "Set-Content -Path \"$env:TMP\\payload.json\" -Value '{}'",
    "'x' > $env:USERPROFILE\\notes.txt",
    "Add-Content -Path ~\\.gitconfig -Value '[user]'",
    "Add-Content $HOME\\notes.txt 'x'",
    "Out-File -FilePath Temp:\\x.txt -InputObject 'x'",
    "Set-Content -Path D:\\scratch\\a.txt -Value 'x'",
    "Set-Content -LiteralPath '\\\\server\\share\\a.txt' -Value 'x'",
    "Set-Content -LiteralPath:$env:TEMP\\x.txt -Value 'x'",
    "\"k=v\" >> $env:GITHUB_OUTPUT",
    "Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value '## Done'",
    "Set-Content Env:\\GREETING 'hi'",
    "[IO.File]::WriteAllText(\"$env:TEMP\\x.txt\", 'x')",
    "sed -i 's/a/b/' C:\\Users\\dev\\notes.txt",
    // Anything the lexer cannot place.
    "\"unterminated > a.txt",
    "Set-Content @params",
    "& $tool 'x' > a.txt",
];

#[test]
fn powershell_denies_embedded_content_and_rewrites_written_into_the_project() {
    let scope = windows_scope();
    for (command, pattern, program) in DENIED_POWERSHELL {
        let expected = Finding {
            pattern: *pattern,
            program: (*program).to_owned(),
        };
        assert_eq!(
            classify_powershell(command, &scope),
            Some(expected),
            "{command:?}"
        );
    }
}

#[test]
fn powershell_allows_output_capture_copies_and_writes_outside_the_project() {
    let scope = windows_scope();
    for command in ALLOWED_POWERSHELL {
        assert_eq!(classify_powershell(command, &scope), None, "{command:?}");
    }
}

#[test]
fn powershell_run_from_bash_gets_the_powershell_checks() {
    let scope = unix_scope();
    for (command, program) in [
        // The substring check that preceded the PowerShell classifier missed
        // redirections.
        ("pwsh -c \"'x' > a.txt\"", "pwsh"),
        (
            "pwsh -c 'Get-Content a.txt | % { $_ -replace \"a\", \"b\" } | Set-Content a.txt'",
            "pwsh",
        ),
        (
            "powershell.exe -NoProfile -Command \"@('a','b') | Tee-Object -FilePath a.txt\"",
            "powershell",
        ),
        ("pwsh -cwa 'Set-Content a.txt $args[0]' x", "pwsh"),
        (
            "cat <<'EOF' | pwsh -Command -\nSet-Content a.txt 'x'\nEOF",
            "pwsh",
        ),
        // Bash may already have expanded `$env` inside double quotes.
        ("pwsh -c \"'x' > $env:GITHUB_OUTPUT\"", "pwsh"),
    ] {
        let expected = Finding {
            pattern: InlineScriptWrite,
            program: program.to_owned(),
        };
        assert_eq!(
            classify_bash(command, &scope),
            Some(expected),
            "{command:?}"
        );
    }
    for command in [
        "pwsh -c 'Get-ChildItem | Out-File list.txt'",
        "pwsh -c 'Get-Content a | Set-Content b'",
        "pwsh -c 'Get-Help Set-Content'",
        "pwsh -c \"Write-Output 'Set-Content a.txt x'\"",
        "pwsh -c '\"x\" > /tmp/x.txt'",
    ] {
        assert_eq!(classify_bash(command, &scope), None, "{command:?}");
    }
}

#[test]
fn powershell_finishes_on_pathological_input() {
    let scope = windows_scope();
    for command in [
        format!("{}'x' > a.txt{}", "(".repeat(10_000), ")".repeat(10_000)),
        format!("\"{}\"", "$(".repeat(10_000)),
        format!("{}x{}", "{".repeat(10_000), "}".repeat(10_000)),
        format!("x{}|y", "\n".repeat(100_000)),
        "@'\n".repeat(20_000),
        "[".repeat(50_000),
        "<#".repeat(50_000),
        "`".repeat(50_000),
        "\"".repeat(50_001),
        "$".repeat(50_000),
        "2>".repeat(50_000),
        "-".repeat(50_000),
    ] {
        assert_eq!(
            classify_powershell(&command, &scope),
            None,
            "{:?}",
            &command[..20]
        );
    }
    let long = format!("@'\n{}'@ | Set-Content a.txt", "line\n".repeat(50_000));
    assert_eq!(
        classify_powershell(&long, &scope).map(|finding| finding.pattern),
        Some(HeredocToFile)
    );
    let many = "'x' | Out-Null; ".repeat(20_000) + "'x' > a.txt";
    assert_eq!(
        classify_powershell(&many, &scope).map(|finding| finding.pattern),
        Some(GeneratedToFile)
    );
    for command in [
        "", "`", "'", "\"$(", "@'", "@\"\n", "<#", ">", "2>&", "*>", "|", "||", "&&", "&", ";",
        "(", ")", "{", "}", "[", "]", "$", "${", "-", "\u{2013}", ".", "::", "=", "@",
    ] {
        assert_eq!(classify_powershell(command, &scope), None, "{command:?}");
    }
}

#[test]
fn hook_scopes_writes_to_the_project() {
    let project = [
        ("CLAUDE_PROJECT_DIR", "/work/repo"),
        ("HOME", "/home/dev"),
        ("TMPDIR", "/tmp"),
    ];
    // CLAUDE_PROJECT_DIR takes precedence over the event's cwd.
    for command in [
        "echo \"k=v\" >> \"$GITHUB_OUTPUT\"",
        "echo x | sudo tee /etc/hosts",
        "echo 'export A=1' >> ~/.bashrc",
        "cat > /tmp/payload.json <<'EOF'\n{}\nEOF",
        "sed -i 's/a/b/' /etc/hosts",
        "cat > /elsewhere/a.rs <<'EOF'\nx\nEOF",
    ] {
        let output = hook(&event("Bash", command, Some("/elsewhere")), &project);
        assert!(!denied(&output), "{command:?}");
    }
    for command in [
        "cat > src/a.rs <<'EOF'\nx\nEOF",
        "cat > /work/repo/src/a.rs <<'EOF'\nx\nEOF",
        "cat > \"$CLAUDE_PROJECT_DIR/src/a.rs\" <<'EOF'\nx\nEOF",
        "echo x > ../x",
        "echo x > \"$UNSET/x\"",
    ] {
        let output = hook(&event("Bash", command, Some("/elsewhere")), &project);
        assert!(denied(&output), "{command:?}");
    }
    // Without an absolute CLAUDE_PROJECT_DIR, the event's cwd is the root.
    let home = ("HOME", "/home/dev");
    for environment in [
        &[home][..],
        &[("CLAUDE_PROJECT_DIR", "repo"), home][..],
        &[("CLAUDE_PROJECT_DIR", ""), home][..],
    ] {
        let outside = event(
            "Bash",
            "cat > /tmp/payload.json <<'EOF'\n{}\nEOF",
            Some("/work/repo"),
        );
        assert!(!denied(&hook(&outside, environment)), "{environment:?}");
        let inside = event(
            "Bash",
            "cat > /work/repo/a.json <<'EOF'\n{}\nEOF",
            Some("/work/repo"),
        );
        assert!(denied(&hook(&inside, environment)), "{environment:?}");
    }
    let nested = event(
        "Bash",
        "cat > /tmp/repo/src/a <<'EOF'\nx\nEOF",
        Some("/tmp/repo"),
    );
    assert!(denied(&hook(&nested, &[])));
    // Without any absolute root, every file target counts.
    for cwd in [None, Some("relative/dir"), Some("")] {
        let runner = event("Bash", "echo \"k=v\" >> \"$GITHUB_OUTPUT\"", cwd);
        assert!(denied(&hook(&runner, &[])), "{cwd:?}");
    }
    // A Windows root compares drives, shares, and Git Bash paths without case.
    for (command, inside) in [
        ("cat > /c/Repo/src/a.rs <<'EOF'\nx\nEOF", true),
        ("cat > 'C:\\REPO\\src\\a.rs' <<'EOF'\nx\nEOF", true),
        ("cat > 'D:\\scratch\\a.json' <<'EOF'\nx\nEOF", false),
        ("cat > /d/scratch/a.json <<'EOF'\nx\nEOF", false),
    ] {
        let output = hook(&event("Bash", command, Some("C:\\repo")), &[]);
        assert_eq!(denied(&output), inside, "{command:?}");
    }
}

#[test]
fn hook_guards_the_powershell_tool() {
    let windows = [
        ("CLAUDE_PROJECT_DIR", "C:\\repo"),
        ("USERPROFILE", "C:\\Users\\dev"),
        ("TEMP", "C:\\Users\\dev\\AppData\\Local\\Temp"),
    ];
    for (command, expected) in [
        ("@\"\nx\n\"@ | Set-Content src/a.ps1", true),
        ("Set-Content -Path a.txt -Value 'x'", true),
        (
            "(Get-Content a.txt) -replace 'a','b' | Set-Content a.txt",
            true,
        ),
        ("Set-Content C:\\repo\\a.txt 'x'", true),
        ("git diff > d.patch", false),
        ("Get-Content a | Set-Content b", false),
        ("Set-Content $env:TEMP\\x.txt 'x'", false),
        ("\"k=v\" >> $env:GITHUB_OUTPUT", false),
    ] {
        let output = hook(
            &event("PowerShell", command, Some("C:\\Users\\dev")),
            &windows,
        );
        assert_eq!(denied(&output), expected, "{command:?}");
    }
    let output = hook(
        &event("PowerShell", "'x' > a.txt", Some("C:\\repo")),
        &windows,
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let reason = value["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(
        reason.starts_with(
            "Ultra Edit guard blocked `>` output written to a file: shell writes can lose \
             backslashes, line endings, or encoding."
        ),
        "{reason}"
    );
}
