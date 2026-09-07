use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ultra_edit::compiler;
use ultra_edit::workspace::EditResult;
use ultra_edit::{Change, CommitStatus, EditRequest, FileRequest, Snapshot, Target, Workspace};

const WARMUPS: usize = 3;
const SAMPLES: usize = 31;

fn source(lines: usize, changed: &[usize]) -> String {
    (1..=lines)
        .map(|line| {
            let value = if changed.contains(&line) { 250 } else { 100 };
            format!("const RETRY_{line:06}: u32 = {value}; // request timeout\n")
        })
        .collect()
}

fn request(snapshots: &[Snapshot], changed: &[usize]) -> EditRequest {
    EditRequest {
        request_id: "benchmark-edit".into(),
        files: snapshots
            .iter()
            .enumerate()
            .map(|(file, snapshot)| FileRequest {
                base: snapshot.id.clone(),
                changes: changed
                    .iter()
                    .map(|line| Change {
                        id: format!("file-{file}-retry-{line}"),
                        target: Target::Exact {
                            old: format!("RETRY_{line:06}: u32 = 100"),
                            scope: None,
                        },
                        text: format!("RETRY_{line:06}: u32 = 250"),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn measure(
    label: &str,
    mut operation: impl FnMut() -> Result<Duration, Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..WARMUPS {
        operation()?;
    }
    let mut samples = (0..SAMPLES)
        .map(|_| operation())
        .collect::<Result<Vec<_>, _>>()?;
    samples.sort_unstable();
    let median = samples[SAMPLES / 2].as_secs_f64() * 1_000.0;
    let p95 = samples[(SAMPLES * 95).div_ceil(100) - 1].as_secs_f64() * 1_000.0;
    println!("{label}: median {median:.3} ms; p95 {p95:.3} ms");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let parent = arguments.next().map_or_else(env::temp_dir, PathBuf::from);
    if arguments.next().is_some() {
        return Err("Usage: cargo run --release --example benchmark -- [temporary-parent]".into());
    }
    let directory = tempfile::tempdir_in(&parent)?;
    println!(
        "ultra-edit {}; {} {}; {} warmups + {} samples; nearest-rank p95",
        env!("CARGO_PKG_VERSION"),
        env::consts::OS,
        env::consts::ARCH,
        WARMUPS,
        SAMPLES
    );
    let parent = fs::canonicalize(parent)?;
    println!(
        "Temporary parent: {}",
        ultra_edit::report::path_for_display(&parent.to_string_lossy())
    );
    println!(
        "Setup, output verification, and cleanup are outside timers; source reads are warm-cache."
    );

    let large = source(100_000, &[]);
    let selected = large
        .lines()
        .skip(49_999)
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");
    println!(
        "Focused read: {} bytes, 100000 lines; lines 50000..50009, {} returned bytes.",
        large.len(),
        selected.len()
    );
    println!("Timer: Workspace::read_range, including hashing and persisted full-byte snapshot.");
    measure("Focused read", || {
        let iteration = tempfile::tempdir_in(directory.path())?;
        let path = iteration.path().join("large.rs");
        fs::write(&path, &large)?;
        let workspace = Workspace::open(iteration.path())?;
        let start = Instant::now();
        let view = workspace.read_range("large.rs", 50_000, 50_009)?;
        let elapsed = start.elapsed();
        assert_eq!(view.text, selected);
        assert_eq!(view.total_bytes, large.len());
        assert_eq!(view.total_lines, 100_000);
        assert_eq!(view.spans.len(), 11);
        assert_eq!(fs::read(&path)?, large.as_bytes());
        Ok(elapsed)
    })?;

    let changed = [100, 200, 300, 400, 500, 600, 700, 800];
    let original = source(1_000, &[]);
    let expected = source(1_000, &changed);
    let snapshots: Vec<_> = (0..8)
        .map(|file| compiler::snapshot(format!("module_{file}.rs"), original.clone()))
        .collect();
    let batch = request(&snapshots, &changed);
    let snapshots: BTreeMap<_, _> = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.id.clone(), snapshot))
        .collect();
    println!(
        "Planning: 8 files x {} bytes / 1000 lines; {} total bytes; 64 exact changes.",
        original.len(),
        original.len() * 8
    );
    println!(
        "Timer: compiler::compile on preloaded snapshots; includes validation and plan creation, no file I/O."
    );
    measure("Batch planning", || {
        let start = Instant::now();
        let plan = black_box(compiler::compile(black_box(&batch), black_box(&snapshots)))
            .expect("valid benchmark batch");
        let elapsed = start.elapsed();
        assert_eq!(plan.files.len(), 8);
        for file in &plan.files {
            assert_eq!(file.output, expected);
            assert_eq!(file.base.text, original);
            assert_eq!(file.replacements.len(), changed.len());
        }
        Ok(elapsed)
    })?;

    let changed = [40, 80, 120, 160];
    let original = source(200, &[]);
    let expected = source(200, &changed);
    println!(
        "Complete edit: 2 files x {} bytes / 200 lines; {} total bytes; 8 exact changes.",
        original.len(),
        original.len() * 2
    );
    println!(
        "Timer: two Workspace::read calls, request construction, and Workspace::edit; all normal validation, journaling, and sync calls intact."
    );
    measure("Snapshots + batch edit", || {
        let iteration = tempfile::tempdir_in(directory.path())?;
        let names = ["client.rs", "server.rs"];
        for name in names {
            fs::write(iteration.path().join(name), &original)?;
        }
        let workspace = Workspace::open(iteration.path())?;
        let start = Instant::now();
        let snapshots = [workspace.read(names[0])?, workspace.read(names[1])?];
        let result = workspace.edit(request(&snapshots, &changed))?;
        let elapsed = start.elapsed();
        let EditResult::Completed { receipt, .. } = result else {
            panic!("valid benchmark batch was rejected");
        };
        assert_eq!(receipt.commit, CommitStatus::Committed);
        assert_eq!(receipt.files.len(), names.len());
        for name in names {
            assert_eq!(fs::read(iteration.path().join(name))?, expected.as_bytes());
        }
        Ok(elapsed)
    })?;
    println!("All warmup and measured outputs verified byte-for-byte.");
    println!(
        "Measures the Rust library used by the CLI/MCP server; excludes process startup, transport, and model latency."
    );
    Ok(())
}
