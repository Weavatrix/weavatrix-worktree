//! Pure bridge from one validated plan file to owned rendered bytes.

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    hash::{Sha256Hash, Sha256Hasher},
    limits::WorktreeLimits,
};
use weavatrix_refactor_plan::weavatrix_edit::{
    ApplyLimits, EditError, ErrorCode as EditErrorCode, FileEdit, WriteSummary,
    prepare_edits_with_limits,
};

/// Validated source plus exact output evidence without an output allocation.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ProjectedFile {
    pub(crate) source: String,
    pub(crate) source_hash: Sha256Hash,
    pub(crate) output_hash: Sha256Hash,
    pub(crate) bytes_before: usize,
    pub(crate) bytes_after: usize,
    pub(crate) edit_count: usize,
}

/// Verifies the expected source and hashes the zero-copy output chunks.
pub(crate) fn project_file(
    file: &FileEdit,
    source: String,
    limits: WorktreeLimits,
) -> Result<ProjectedFile, WorktreeError> {
    validate_local_limits(file, &source, limits)?;
    let expected_hash = Sha256Hash::parse(&file.sha256).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Validate,
            "file contains an invalid expected SHA-256",
            error,
        )
        .at_path(file.path.clone())
    })?;
    let source_hash = Sha256Hash::compute(source.as_bytes());
    if source_hash != expected_hash {
        return Err(WorktreeError::new(
            WorktreeErrorCode::SourceHashMismatch,
            TransactionPhase::Prepare,
            format!("expected {expected_hash}, found {source_hash}"),
        )
        .at_path(file.path.clone()));
    }

    let apply_limits = ApplyLimits {
        max_source_bytes: limits.max_source_bytes_per_file,
        max_edits: limits.max_edits_per_file,
        max_output_bytes: limits.max_output_bytes_per_file,
    };
    let prepared = prepare_edits_with_limits(&source, &file.edits, apply_limits)
        .map_err(|error| map_edit_error(error, &file.path))?;
    let bytes_before = prepared.bytes_before();
    let bytes_after = prepared.bytes_after();
    let edit_count = prepared.len();
    let mut output_hasher = Sha256Hasher::new();
    for chunk in prepared.chunks() {
        output_hasher.update(chunk.as_bytes());
    }
    let output_hash = output_hasher.finish();

    Ok(ProjectedFile {
        source,
        source_hash,
        output_hash,
        bytes_before,
        bytes_after,
        edit_count,
    })
}

/// Revalidates a projection and streams it into a caller-owned sink.
pub(crate) fn write_projected(
    file: &FileEdit,
    projected: &ProjectedFile,
    limits: WorktreeLimits,
    writer: &mut dyn std::io::Write,
) -> Result<WriteSummary, WorktreeError> {
    let prepared = prepare_edits_with_limits(
        &projected.source,
        &file.edits,
        ApplyLimits {
            max_source_bytes: limits.max_source_bytes_per_file,
            max_edits: limits.max_edits_per_file,
            max_output_bytes: limits.max_output_bytes_per_file,
        },
    )
    .map_err(|error| map_edit_error(error, &file.path))?;
    if prepared.bytes_after() != projected.bytes_after {
        return Err(WorktreeError::new(
            WorktreeErrorCode::ConcurrentModification,
            TransactionPhase::Stage,
            "prepared output size changed between validation and staging",
        )
        .at_path(file.path.clone()));
    }
    prepared.write_to(writer).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::StageFailed,
            TransactionPhase::Stage,
            "failed to stream prepared output",
            error,
        )
        .at_path(file.path.clone())
    })
}

fn validate_local_limits(
    file: &FileEdit,
    source: &str,
    limits: WorktreeLimits,
) -> Result<(), WorktreeError> {
    if source.len() > limits.max_source_bytes_per_file {
        return Err(WorktreeError::new(
            WorktreeErrorCode::SourceTooLarge,
            TransactionPhase::Prepare,
            format!(
                "source has {} bytes, exceeding the {}-byte per-file limit",
                source.len(),
                limits.max_source_bytes_per_file
            ),
        )
        .at_path(file.path.clone()));
    }
    if file.edits.is_empty() || file.edits.len() > limits.max_edits_per_file {
        return Err(WorktreeError::new(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Validate,
            format!(
                "file must contain between 1 and {} edits",
                limits.max_edits_per_file
            ),
        )
        .at_path(file.path.clone()));
    }
    Ok(())
}

fn map_edit_error(error: EditError, path: &str) -> WorktreeError {
    let code = match error.code() {
        EditErrorCode::PlanTooLarge | EditErrorCode::OutputTooLarge => {
            WorktreeErrorCode::TransactionTooLarge
        }
        _ => WorktreeErrorCode::EditRejected,
    };
    WorktreeError::with_source(
        code,
        TransactionPhase::Prepare,
        "weavatrix-edit rejected the file plan",
        error,
    )
    .at_path(path.to_owned())
}

#[cfg(test)]
mod tests {
    use crate::edit::{project_file, write_projected};
    use crate::{error::WorktreeErrorCode, hash::Sha256Hash, limits::WorktreeLimits};
    use weavatrix_refactor_plan::{FileEdit, Position, Provenance, TextEdit};

    #[test]
    fn renders_owned_output_and_hash_evidence() {
        let source = "value\n";
        let file = FileEdit::new(
            "src/lib.rs",
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![TextEdit::insert(
                Position::new(1, 5),
                "!",
                Provenance::EXACT_LSP,
            )],
        );

        let rendered = project_file(&file, source.to_owned(), WorktreeLimits::default()).unwrap();

        assert_eq!(rendered.source, source);
        assert_eq!(rendered.bytes_before, source.len());
        assert_eq!(rendered.bytes_after, b"value!\n".len());
        assert_eq!(rendered.edit_count, 1);
        assert_eq!(rendered.output_hash, Sha256Hash::compute(b"value!\n"));
        let mut output = Vec::new();
        let summary =
            write_projected(&file, &rendered, WorktreeLimits::default(), &mut output).unwrap();
        assert_eq!(output, b"value!\n");
        assert_eq!(summary.bytes_written, output.len());
    }

    #[test]
    fn stale_source_fails_before_edit_application() {
        let file = FileEdit::new(
            "src/lib.rs",
            Sha256Hash::compute(b"old").to_string(),
            vec![TextEdit::insert(
                Position::new(1, 0),
                "!",
                Provenance::EXACT_LSP,
            )],
        );

        let error = project_file(&file, "new".to_owned(), WorktreeLimits::default()).unwrap_err();

        assert_eq!(error.code(), WorktreeErrorCode::SourceHashMismatch);
        assert_eq!(error.path(), Some("src/lib.rs"));
    }
}
