use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::{
    Sha256Hash,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot, PresentEvidence, SlotEvidence},
    hash::serialized_hash,
    options::WorktreeOptions,
};

use super::types::{UndoId, UndoReceipt, UndoRetention, WorktreeSnapshotFingerprint};
use crate::operation::PreparedWorktreeTransaction;

mod store;

pub(in crate::operation) use store::{discard, inspect, read_exact, store_usage};

const SCHEMA: &str = "weavatrix.worktree-undo.v1";
const SNAPSHOT_DOMAIN: &str = "weavatrix.worktree-snapshot.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReceiptPath {
    pub(super) index: u32,
    pub(super) path: String,
    pub(super) before: SlotEvidence,
    pub(super) after: SlotEvidence,
    pub(super) backup_name: Option<String>,
    pub(super) backup: Option<PresentEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptBody {
    schema: String,
    transaction_id: String,
    plan_fingerprint: Sha256Hash,
    before_fingerprint: Sha256Hash,
    after_fingerprint: Sha256Hash,
    retained_bytes: u64,
    paths: Vec<ReceiptPath>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptEnvelope {
    body: ReceiptBody,
    checksum: Sha256Hash,
}

pub(in crate::operation) struct StoredReceipt {
    body: ReceiptBody,
    pub(super) checksum: Sha256Hash,
    pub(super) stored_bytes: u64,
}

impl StoredReceipt {
    pub(super) fn id(&self) -> &str {
        &self.body.transaction_id
    }

    pub(super) fn paths(&self) -> &[ReceiptPath] {
        &self.body.paths
    }

    pub(super) const fn retained_bytes(&self) -> u64 {
        self.body.retained_bytes
    }

    pub(in crate::operation) fn public(&self) -> UndoReceipt {
        UndoReceipt {
            id: UndoId::from_transaction(self.body.transaction_id.clone()),
            plan_fingerprint: self.body.plan_fingerprint,
            before: WorktreeSnapshotFingerprint::new(self.body.before_fingerprint),
            after: WorktreeSnapshotFingerprint::new(self.body.after_fingerprint),
            touched_paths: self.body.paths.len(),
            retained_bytes: self.body.retained_bytes,
        }
    }
}

pub(in crate::operation) fn prepare_retention(
    transaction: &PreparedWorktreeTransaction,
    policy: UndoRetention,
) -> Result<StoredReceipt, WorktreeError> {
    validate_policy(policy, transaction.options)?;
    let usage = store_usage(&transaction.control, transaction.options)?;
    let body = body_from(transaction)?;
    let checksum = serialized_hash(&body).map_err(encode_error)?;
    let bytes = encode(&ReceiptEnvelope {
        body: body.clone(),
        checksum,
    })?;
    let stored_bytes = bytes.len() as u64;
    let added = body.retained_bytes.saturating_add(stored_bytes);
    if usage.receipts.saturating_add(1) > policy.max_receipts()
        || usage.receipts.saturating_add(1) > transaction.options.limits.max_undo_receipts
        || usage.bytes.saturating_add(added) > policy.max_bytes() as u64
        || usage.bytes.saturating_add(added)
            > transaction.options.limits.max_total_undo_bytes as u64
    {
        return Err(undo_error(
            WorktreeErrorCode::UndoStoreFull,
            TransactionPhase::Commit,
            "retained undo capacity is exhausted",
        ));
    }
    write_new(&transaction.control, &body.transaction_id, &bytes)?;
    Ok(StoredReceipt {
        body,
        checksum,
        stored_bytes,
    })
}

/// Idempotently centralizes every retained backup named by a durable receipt.
/// Finished-commit recovery calls this while the operation journal still
/// protects a crash between individual cross-directory renames.
pub(in crate::operation) fn relocate_backups(
    control: &ControlDir,
    root: &FsRoot,
    receipt: &StoredReceipt,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    for (position, path) in receipt.paths().iter().enumerate() {
        let (Some(name), Some(expected)) = (path.backup_name.as_deref(), path.backup) else {
            continue;
        };
        let access = root.open_target(&path.path).map_err(|error| {
            undo_io(
                TransactionPhase::Recover,
                "failed to reopen a retained backup parent",
                error,
            )
            .at_path(path.path.clone())
            .at_file(position)
            .in_transaction(receipt.id().to_owned())
            .requiring_recovery()
        })?;
        control
            .retain_backup_from(
                &access,
                name,
                expected,
                options.limits.max_source_bytes_per_file,
            )
            .map_err(|error| {
                undo_io(
                    TransactionPhase::Recover,
                    "failed to move retained backup into the state directory",
                    error,
                )
                .at_path(path.path.clone())
                .at_file(position)
                .in_transaction(receipt.id().to_owned())
                .requiring_recovery()
            })?;
    }
    Ok(())
}

fn body_from(transaction: &PreparedWorktreeTransaction) -> Result<ReceiptBody, WorktreeError> {
    let paths = transaction
        .paths
        .iter()
        .map(|path| ReceiptPath {
            index: path.stable_index,
            path: path.path.clone(),
            before: path.before,
            after: path.after,
            backup_name: path.backup_name.clone(),
            backup: path.backup,
        })
        .collect::<Vec<_>>();
    let retained_bytes =
        paths
            .iter()
            .filter_map(|path| path.backup)
            .try_fold(0_u64, |total, value| {
                total
                    .checked_add(value.bytes)
                    .ok_or_else(|| corrupt("undo artifact bytes overflow"))
            })?;
    Ok(ReceiptBody {
        schema: SCHEMA.to_owned(),
        transaction_id: transaction.transaction_id.clone(),
        plan_fingerprint: transaction.contract_hash,
        before_fingerprint: snapshot(&paths, true)?,
        after_fingerprint: snapshot(&paths, false)?,
        retained_bytes,
        paths,
    })
}

fn snapshot(paths: &[ReceiptPath], before: bool) -> Result<Sha256Hash, WorktreeError> {
    let states = paths
        .iter()
        .map(|path| {
            (
                path.path.as_str(),
                if before { &path.before } else { &path.after },
            )
        })
        .collect::<Vec<_>>();
    serialized_hash(&(SNAPSHOT_DOMAIN, states)).map_err(encode_error)
}

fn write_new(control: &ControlDir, id: &str, bytes: &[u8]) -> Result<(), WorktreeError> {
    let mut file = control.create_undo_receipt(id).map_err(store_io)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all());
    if let Err(error) = result {
        drop(file);
        let _ = control.remove_undo_receipt(id);
        return Err(store_io(error));
    }
    control.sync().map_err(store_io)
}

fn validate_policy(policy: UndoRetention, options: WorktreeOptions) -> Result<(), WorktreeError> {
    if policy.max_receipts() == 0
        || policy.max_bytes() == 0
        || policy.max_receipts() > options.limits.max_undo_receipts
        || policy.max_bytes() > options.limits.max_total_undo_bytes
    {
        return Err(undo_error(
            WorktreeErrorCode::InvalidOptions,
            TransactionPhase::Validate,
            "undo retention policy is zero or exceeds worktree limits",
        ));
    }
    Ok(())
}

fn encode(value: &ReceiptEnvelope) -> Result<Vec<u8>, WorktreeError> {
    blazingly_json::to_vec(value).map_err(encode_error)
}

fn encode_error(error: blazingly_json::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::UndoCorrupt,
        TransactionPhase::Prepare,
        "failed to encode retained undo evidence",
        error,
    )
}

fn store_io(error: std::io::Error) -> WorktreeError {
    undo_io(TransactionPhase::Recover, "undo store I/O failed", error)
}

fn undo_io(phase: TransactionPhase, message: &str, error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(WorktreeErrorCode::UndoFailed, phase, message, error)
}

fn corrupt(message: &str) -> WorktreeError {
    undo_error(
        WorktreeErrorCode::UndoCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}

fn undo_error(code: WorktreeErrorCode, phase: TransactionPhase, message: &str) -> WorktreeError {
    WorktreeError::new(code, phase, message)
}
