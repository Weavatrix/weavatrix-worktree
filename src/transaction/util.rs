use std::io;

use crate::error::{TransactionPhase, WorktreeError, WorktreeErrorCode};

pub(crate) fn fs_error(
    phase: TransactionPhase,
    path: &str,
    index: usize,
    message: &str,
    source: io::Error,
) -> WorktreeError {
    let detail = source.to_string();
    let code = if detail.contains("reserved") {
        WorktreeErrorCode::ReservedPath
    } else if detail.contains("symbolic link") || detail.contains("reparse") {
        WorktreeErrorCode::SymlinkNotAllowed
    } else if detail.contains("crosses") {
        WorktreeErrorCode::CrossFilesystem
    } else if detail.contains("not a regular file") {
        WorktreeErrorCode::NotRegularFile
    } else if detail.contains("hard-linked") {
        WorktreeErrorCode::HardlinkNotAllowed
    } else if detail.contains("read-only") {
        WorktreeErrorCode::ReadOnlyFile
    } else if detail.contains("byte limit") || detail.contains("exceeds") {
        WorktreeErrorCode::SourceTooLarge
    } else {
        WorktreeErrorCode::Io
    };
    WorktreeError::with_source(code, phase, message, source)
        .at_path(path.to_owned())
        .at_file(index)
}

pub(crate) fn journal_error(
    phase: TransactionPhase,
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> WorktreeError {
    WorktreeError::with_source(WorktreeErrorCode::JournalCorrupt, phase, message, source)
}

pub(crate) fn checked_usize(value: u64, message: &str) -> Result<usize, WorktreeError> {
    usize::try_from(value).map_err(|_| {
        WorktreeError::new(
            WorktreeErrorCode::TransactionTooLarge,
            TransactionPhase::Validate,
            message,
        )
    })
}
