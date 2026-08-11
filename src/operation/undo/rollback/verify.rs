use crate::{
    error::WorktreeError,
    filesystem::{ControlDir, FsRoot, TargetAccess},
    options::WorktreeOptions,
};

use super::{ReceiptPath, StoredReceipt, conflict, state_bytes, verify_io};

/// Compares every retained path and artifact with the receipt's exact
/// committed evidence before any mutation is allowed to start.
pub(super) fn verify_receipt_state(
    control: &ControlDir,
    root: &FsRoot,
    receipt: &StoredReceipt,
    options: WorktreeOptions,
) -> Result<Vec<TargetAccess>, WorktreeError> {
    let mut accesses = Vec::with_capacity(receipt.paths().len());
    for path in receipt.paths() {
        let access = root
            .open_target(&path.path)
            .map_err(|error| verify_io(path, "failed to reopen a retained path", error))?;
        let actual = access
            .slot_evidence(state_bytes(options))
            .map_err(|error| verify_io(path, "failed to inspect a retained path", error))?;
        if actual != path.after {
            return Err(conflict(
                path,
                "retained path no longer matches its exact committed state",
            ));
        }
        verify_backup(control, path, options)?;
        accesses.push(access);
    }
    Ok(accesses)
}

fn verify_backup(
    control: &ControlDir,
    path: &ReceiptPath,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    let (Some(name), Some(expected)) = (path.backup_name.as_deref(), path.backup) else {
        return Ok(());
    };
    let actual = control
        .backup_evidence(name, options.limits.max_source_bytes_per_file)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                conflict(path, "retained undo artifact is missing")
            } else {
                verify_io(path, "failed to verify a retained undo artifact", error)
            }
        })?;
    if actual == expected {
        Ok(())
    } else {
        Err(conflict(
            path,
            "retained undo artifact does not match its receipt evidence",
        ))
    }
}
