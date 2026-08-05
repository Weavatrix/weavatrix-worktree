use weavatrix_refactor_plan::EditPlan;

use crate::{
    WorktreeLimits,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    metadata::{JsonBudget, JsonLimits},
};

pub(super) fn validate_metadata(
    plan: &EditPlan,
    limits: WorktreeLimits,
) -> Result<(), WorktreeError> {
    if plan.operation.len() > limits.max_operation_bytes
        || plan
            .completeness
            .as_ref()
            .is_some_and(|value| value.0.len() > limits.max_operation_bytes)
    {
        return Err(invalid_plan("plan metadata exceeds its byte limit"));
    }
    let mut budget = JsonBudget::new(JsonLimits {
        bytes: limits.max_extension_bytes,
        nodes: limits.max_extension_nodes,
        depth: limits.max_extension_depth,
    });
    visit_extensions(&mut budget, &plan.extensions)?;
    for file in &plan.files {
        visit_extensions(&mut budget, &file.extensions)?;
        for edit in &file.edits {
            visit_extensions(&mut budget, &edit.extensions)?;
        }
    }
    Ok(())
}

fn visit_extensions(
    budget: &mut JsonBudget,
    values: &std::collections::BTreeMap<String, blazingly_json::Value>,
) -> Result<(), WorktreeError> {
    budget.visit_map(values).map_err(|error| {
        let message = error.message().to_owned();
        WorktreeError::with_source(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Validate,
            message,
            error,
        )
    })
}

fn invalid_plan(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::InvalidPlan,
        TransactionPhase::Validate,
        message,
    )
}
