use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use weavatrix_edit::{EditPlan, FileEdit, Position, Provenance, TextEdit, TextRange};
use weavatrix_worktree::{Worktree, WorktreeOptions};

const ADAPTER_SCHEMA: &str = "weavatrix.worktree-benchmark-adapter.v1";
const MANIFEST_SCHEMA: &str = "weavatrix.worktree-benchmark-manifest.v1";

#[derive(Debug)]
struct Args {
    workspace: PathBuf,
    manifest: PathBuf,
    mode: String,
    workers: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    operation: String,
    file_count: usize,
    file_bytes: usize,
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    sha256: String,
    expected_sha256: String,
    bytes_before: usize,
    bytes_after: usize,
    edits: Vec<ManifestEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEdit {
    start: ManifestPosition,
    end: ManifestPosition,
    expected: String,
    replacement: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPosition {
    line: u32,
    character: u32,
}

fn parse_args() -> Result<Option<Args>, Box<dyn Error>> {
    let mut values = env::args().skip(1);
    let mut workspace = None;
    let mut manifest = None;
    let mut mode = None;
    let mut workers = None;
    while let Some(flag) = values.next() {
        if flag == "--version" {
            println!(
                "weavatrix-worktree-bench-adapter {} (weavatrix-worktree {})",
                env!("CARGO_PKG_VERSION"),
                weavatrix_worktree::VERSION
            );
            return Ok(None);
        }
        let value = values
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--manifest" => manifest = Some(PathBuf::from(value)),
            "--mode" => mode = Some(value),
            "--workers" => workers = Some(value.parse::<usize>()?),
            _ => return Err(format!("unknown argument: {flag}").into()),
        }
    }
    let args = Args {
        workspace: workspace.ok_or("--workspace is required")?,
        manifest: manifest.ok_or("--manifest is required")?,
        mode: mode.ok_or("--mode is required")?,
        workers: workers.ok_or("--workers is required")?,
    };
    if args.workers == 0 {
        return Err("--workers must be positive".into());
    }
    if args.mode != "dry-run" && args.mode != "durable-apply" {
        return Err(format!("unsupported mode: {}", args.mode).into());
    }
    Ok(Some(args))
}

fn load_plan(path: &PathBuf) -> Result<(EditPlan, usize, usize), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(format!("unsupported manifest schema: {}", manifest.schema).into());
    }
    if manifest.file_count != manifest.files.len() || manifest.files.is_empty() {
        return Err("manifest file_count is inconsistent".into());
    }
    if manifest.file_bytes == 0 {
        return Err("manifest file_bytes must be positive".into());
    }
    let mut total_edits = 0usize;
    let mut files = Vec::with_capacity(manifest.files.len());
    for file in manifest.files {
        if file.bytes_before == 0 || file.bytes_after == 0 {
            return Err(format!("manifest byte counts are invalid for {}", file.path).into());
        }
        if file.expected_sha256.len() != 64 {
            return Err(format!("invalid expected output SHA-256 for {}", file.path).into());
        }
        let mut edits = Vec::with_capacity(file.edits.len());
        for edit in file.edits {
            edits.push(TextEdit::replace(
                TextRange::new(
                    Position::new(edit.start.line, edit.start.character),
                    Position::new(edit.end.line, edit.end.character),
                ),
                edit.expected,
                edit.replacement,
                Provenance::EXACT_LSP,
            ));
        }
        total_edits = total_edits
            .checked_add(edits.len())
            .ok_or("edit count overflow")?;
        files.push(FileEdit::new(file.path, file.sha256, edits));
    }
    let file_count = files.len();
    Ok((
        EditPlan::new(manifest.operation, files),
        file_count,
        total_edits,
    ))
}

fn run(args: Args) -> Result<serde_json::Value, Box<dyn Error>> {
    let (plan, file_count, expected_edits) = load_plan(&args.manifest)?;
    let options = WorktreeOptions::default().with_parallelism(args.workers);
    let workers_effective = options.worker_count(file_count);
    let started = Instant::now();
    let worktree = Worktree::open_with(&args.workspace, options)?;
    let (reported_files, reported_edits, transaction_id) = if args.mode == "dry-run" {
        let report = worktree.dry_run(&plan)?;
        (report.files().len(), report.total_edits(), None)
    } else {
        let report = worktree.apply(&plan)?;
        (
            report.files().len(),
            report.total_edits(),
            Some(report.transaction_id().to_owned()),
        )
    };
    if reported_files != file_count || reported_edits != expected_edits {
        return Err(format!(
            "report mismatch: files {reported_files}/{file_count}, edits {reported_edits}/{expected_edits}"
        )
        .into());
    }
    let elapsed = started.elapsed().as_nanos();
    Ok(json!({
        "schema": ADAPTER_SCHEMA,
        "ok": true,
        "adapter": "weavatrix-worktree-rust",
        "adapter_version": env!("CARGO_PKG_VERSION"),
        "worktree_version": weavatrix_worktree::VERSION,
        "mode": args.mode,
        "files": reported_files,
        "edits": reported_edits,
        "workers_requested": args.workers,
        "workers_effective": workers_effective,
        "adapter_elapsed_ns": elapsed.to_string(),
        "transaction_id": transaction_id,
        "durability_contract": if transaction_id.is_some() {
            "CRASH_RECOVERABLE_ALL_OR_RESTORED_WITH_PER_FILE_ATOMIC_REPLACE"
        } else {
            "READ_ONLY_HASH_AND_EDIT_VALIDATION"
        },
        "equivalent_to_weavatrix_recoverable_batch": transaction_id.is_some(),
    }))
}

fn emit(value: &serde_json::Value) {
    println!("{value}");
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(args)) => match run(args) {
            Ok(result) => {
                emit(&result);
                ExitCode::SUCCESS
            }
            Err(error) => {
                emit(&json!({
                    "schema": ADAPTER_SCHEMA,
                    "ok": false,
                    "adapter": "weavatrix-worktree-rust",
                    "error": error.to_string(),
                    "durability_contract": "UNKNOWN_BECAUSE_ADAPTER_FAILED",
                    "equivalent_to_weavatrix_recoverable_batch": false,
                }));
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            emit(&json!({
                "schema": ADAPTER_SCHEMA,
                "ok": false,
                "adapter": "weavatrix-worktree-rust",
                "error": error.to_string(),
                "durability_contract": "UNKNOWN_BECAUSE_ADAPTER_FAILED",
                "equivalent_to_weavatrix_recoverable_batch": false,
            }));
            ExitCode::FAILURE
        }
    }
}
