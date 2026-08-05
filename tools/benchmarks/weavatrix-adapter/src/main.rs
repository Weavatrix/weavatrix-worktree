use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use weavatrix_refactor_plan::{FileEdit, Position, Provenance, TextEdit, TextRange};
use weavatrix_worktree::{
    CreateFile, DeleteFile, RenameFile, Worktree, WorktreeLimits, WorktreeOperation,
    WorktreeOptions, WorktreePlan,
};

const ADAPTER_SCHEMA: &str = "weavatrix.worktree-benchmark-adapter.v2";
const MANIFEST_SCHEMA: &str = "weavatrix.worktree-benchmark-manifest.v2";

#[derive(Debug)]
struct Args {
    workspace: PathBuf,
    manifest: PathBuf,
    mode: String,
    workers: usize,
}

struct LoadedPlan {
    plan: WorktreePlan,
    workload: String,
    operation_count: usize,
    touched_paths: usize,
    total_edits: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    fixture_generator: String,
    fixture_seed: Option<u64>,
    operation: String,
    workload: String,
    operation_count: usize,
    touched_path_count: usize,
    file_bytes: usize,
    operations: Vec<ManifestOperation>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ManifestOperation {
    Modify {
        path: String,
        source_sha256: String,
        output_sha256: String,
        bytes_before: usize,
        bytes_after: usize,
        edits: Vec<ManifestEdit>,
    },
    Create {
        path: String,
        content: String,
        output_sha256: String,
        bytes_after: usize,
    },
    Delete {
        path: String,
        source_sha256: String,
        bytes_before: usize,
    },
    Rename {
        source: String,
        target: String,
        source_sha256: String,
        bytes_before: usize,
    },
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

fn text_edits(edits: Vec<ManifestEdit>) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|edit| {
            TextEdit::replace(
                TextRange::new(
                    Position::new(edit.start.line, edit.start.character),
                    Position::new(edit.end.line, edit.end.character),
                ),
                edit.expected,
                edit.replacement,
                Provenance::EXACT_LSP,
            )
        })
        .collect()
}

fn require_hash(value: &str, label: &str) -> Result<(), Box<dyn Error>> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid SHA-256 for {label}").into());
    }
    Ok(())
}

fn load_plan(path: &PathBuf) -> Result<LoadedPlan, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(format!("unsupported manifest schema: {}", manifest.schema).into());
    }
    if manifest.fixture_generator != "weavatrix-fixed-markers-v2" || manifest.fixture_seed.is_some()
    {
        return Err("unsupported fixture generator or seed".into());
    }
    if manifest.operation_count != manifest.operations.len() || manifest.operations.is_empty() {
        return Err("manifest operation_count is inconsistent".into());
    }
    if manifest.touched_path_count == 0 || manifest.file_bytes == 0 {
        return Err("manifest counts must be positive".into());
    }
    let mut total_edits = 0usize;
    let mut operations = Vec::with_capacity(manifest.operations.len());
    for operation in manifest.operations {
        let converted = match operation {
            ManifestOperation::Modify {
                path,
                source_sha256,
                output_sha256,
                bytes_before,
                bytes_after,
                edits,
            } => {
                require_hash(&source_sha256, &path)?;
                require_hash(&output_sha256, &path)?;
                if bytes_before == 0 || bytes_after == 0 {
                    return Err(format!("invalid modify byte count for {path}").into());
                }
                let edits = text_edits(edits);
                total_edits = total_edits
                    .checked_add(edits.len())
                    .ok_or("edit count overflow")?;
                WorktreeOperation::Modify(FileEdit::new(path, source_sha256, edits))
            }
            ManifestOperation::Create {
                path,
                content,
                output_sha256,
                bytes_after,
            } => {
                require_hash(&output_sha256, &path)?;
                if content.len() != bytes_after {
                    return Err(format!("invalid create byte count for {path}").into());
                }
                WorktreeOperation::Create(CreateFile::new(path, content))
            }
            ManifestOperation::Delete {
                path,
                source_sha256,
                bytes_before,
            } => {
                require_hash(&source_sha256, &path)?;
                if bytes_before == 0 {
                    return Err(format!("invalid delete byte count for {path}").into());
                }
                WorktreeOperation::Delete(DeleteFile::new(path, source_sha256))
            }
            ManifestOperation::Rename {
                source,
                target,
                source_sha256,
                bytes_before,
            } => {
                require_hash(&source_sha256, &source)?;
                if bytes_before == 0 {
                    return Err(format!("invalid rename byte count for {source}").into());
                }
                WorktreeOperation::Rename(RenameFile::new(source, target, source_sha256))
            }
        };
        operations.push(converted);
    }
    let count = operations.len();
    Ok(LoadedPlan {
        plan: WorktreePlan::new(manifest.operation, operations),
        workload: manifest.workload,
        operation_count: count,
        touched_paths: manifest.touched_path_count,
        total_edits,
    })
}

fn run(args: Args) -> Result<serde_json::Value, Box<dyn Error>> {
    let LoadedPlan {
        plan,
        workload,
        operation_count,
        touched_paths,
        total_edits: expected_edits,
    } = load_plan(&args.manifest)?;
    let default_limits = WorktreeLimits::default();
    let effective_max_files = default_limits.max_files.max(touched_paths);
    let limits = WorktreeLimits {
        max_files: effective_max_files,
        ..default_limits
    };
    let options = WorktreeOptions::default()
        .with_limits(limits)
        .with_parallelism(args.workers);
    let workers_effective = options.worker_count(touched_paths);
    let started = Instant::now();
    let worktree = Worktree::open_with(&args.workspace, options)?;
    let (reported_operations, reported_touched_paths, reported_edits, transaction_id) =
        if args.mode == "dry-run" {
            let report = worktree.dry_run_plan(&plan)?;
            (
                report.files().len(),
                report.touched_paths(),
                report.total_edits(),
                None,
            )
        } else {
            let report = worktree.apply_plan(&plan)?;
            (
                report.files().len(),
                report.touched_paths(),
                report.files().iter().fold(0usize, |total, operation| {
                    total.saturating_add(operation.edits_applied())
                }),
                Some(report.transaction_id().to_owned()),
            )
        };
    if reported_operations != operation_count
        || reported_touched_paths != touched_paths
        || reported_edits != expected_edits
    {
        return Err(format!(
            "report mismatch: operations {reported_operations}/{operation_count}, touched paths {reported_touched_paths}/{touched_paths}, edits {reported_edits}/{expected_edits}"
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
        "workload": workload,
        "operations": reported_operations,
        "touched_paths": reported_touched_paths,
        "edits": reported_edits,
        "workers_requested": args.workers,
        "workers_effective": workers_effective,
        "effective_max_files": effective_max_files,
        "effective_max_paths": effective_max_files,
        "adapter_elapsed_ns": elapsed.to_string(),
        "transaction_id": transaction_id,
        "capability_class": "FULL_SHA256_CAS_MODIFY_CREATE_DELETE_RENAME_PLAN",
        "durability_contract": if transaction_id.is_some() {
            "CRASH_RECOVERABLE_ALL_OR_RESTORED_WITH_DETERMINISTIC_RESOURCE_OPERATIONS"
        } else {
            "READ_ONLY_FULL_PLAN_PREFLIGHT_WITH_FULL_FILE_SHA256_CAS"
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
