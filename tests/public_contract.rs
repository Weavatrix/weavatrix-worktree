use weavatrix_worktree::{
    CreateFile, REFACTOR_PLAN_SCHEMA, RefactorOperation, RefactorPlan, RefactorPlanLimits,
    WORKTREE_PLAN_SCHEMA, WorktreeOperation, WorktreePlan, WorktreePlanLimits,
};

#[test]
fn compatibility_names_are_aliases_of_the_shared_refactor_contract() {
    let operation: RefactorOperation =
        WorktreeOperation::Create(CreateFile::new("generated.rs", "generated"));
    let plan: RefactorPlan = WorktreePlan::new("generate", vec![operation]);
    let limits: RefactorPlanLimits = WorktreePlanLimits::default();

    assert_eq!(plan.schema_version, REFACTOR_PLAN_SCHEMA);
    assert_eq!(WORKTREE_PLAN_SCHEMA, REFACTOR_PLAN_SCHEMA);
    assert!(plan.validate_with(limits).is_ok());
}
