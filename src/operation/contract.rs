use weavatrix_refactor_plan::{
    PlanError, PlanErrorCode, RefactorPlanLimits, ValidatedExecutorPlan, validate_executor_plan,
};

use crate::{
    WorktreePlan,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    options::WorktreeOptions,
};

pub(super) fn validate(
    plan: &WorktreePlan,
    options: WorktreeOptions,
) -> Result<ValidatedExecutorPlan<'_>, WorktreeError> {
    validate_executor_plan(plan, plan_limits(options)).map_err(|error| {
        let path = error.path().map(str::to_owned);
        let operation_index = error.operation_index();
        let mut mapped = WorktreeError::with_source(
            worktree_code(&error),
            TransactionPhase::Validate,
            "weavatrix-refactor-plan rejected the executable plan",
            error,
        );
        if let Some(path) = path {
            mapped = mapped.at_path(path);
        }
        if let Some(index) = operation_index {
            mapped = mapped.at_file(index);
        }
        mapped
    })
}

fn worktree_code(error: &PlanError) -> WorktreeErrorCode {
    match error.code() {
        PlanErrorCode::OperationConflict => WorktreeErrorCode::OperationConflict,
        PlanErrorCode::UnsafePath if error.path().is_some_and(is_reserved_path) => {
            WorktreeErrorCode::ReservedPath
        }
        PlanErrorCode::UnsafePath => WorktreeErrorCode::PathEscape,
        _ => WorktreeErrorCode::InvalidPlan,
    }
}

fn is_reserved_path(path: &str) -> bool {
    path.split('/').next().is_some_and(|root| {
        root.eq_ignore_ascii_case(".git") || root.eq_ignore_ascii_case(".weavatrix")
    })
}

fn plan_limits(options: WorktreeOptions) -> RefactorPlanLimits {
    let limits = options.limits;
    RefactorPlanLimits {
        max_operations: limits.max_files,
        max_paths: limits.max_files,
        max_path_bytes: 4_096,
        max_operation_bytes: limits.max_operation_bytes,
        max_edits_per_file: limits.max_edits_per_file,
        max_total_edits: limits.max_files.saturating_mul(limits.max_edits_per_file),
        max_create_bytes_per_file: limits.max_output_bytes_per_file,
        max_total_create_bytes: limits.max_total_output_bytes,
        max_total_text_bytes: limits.max_total_artifact_bytes,
        max_extension_bytes: limits.max_extension_bytes,
        max_extension_nodes: limits.max_extension_nodes,
        max_extension_depth: limits.max_extension_depth,
        max_evidence_entries: limits.max_evidence_entries,
        max_evidence_text_bytes: limits.max_evidence_text_bytes,
        max_code_bytes: limits.max_code_bytes,
    }
}
