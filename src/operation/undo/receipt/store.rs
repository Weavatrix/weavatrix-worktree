use std::io::{Read, Seek, SeekFrom};

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot, SlotEvidence},
    hash::serialized_hash,
    options::WorktreeOptions,
};

use super::{
    ReceiptBody, ReceiptEnvelope, StoredReceipt, corrupt, encode_error, snapshot, store_io,
    undo_error, undo_io,
};
use crate::operation::undo::types::{UndoId, UndoStoreUsage};

pub(in crate::operation) fn inspect(
    control: &ControlDir,
    id: &UndoId,
    options: WorktreeOptions,
) -> Result<StoredReceipt, WorktreeError> {
    read_exact(control, id.as_str(), options)?.ok_or_else(|| {
        undo_error(
            WorktreeErrorCode::UndoNotFound,
            TransactionPhase::Validate,
            "the exact undo receipt does not exist",
        )
    })
}

pub(in crate::operation) fn store_usage(
    control: &ControlDir,
    options: WorktreeOptions,
) -> Result<UndoStoreUsage, WorktreeError> {
    let ids = control.undo_receipt_ids().map_err(store_io)?;
    if ids.len() > options.limits.max_undo_receipts {
        return Err(corrupt("undo receipt count exceeds the configured bound"));
    }
    let mut bytes = 0_u64;
    for id in &ids {
        let receipt = read_exact(control, id, options)?
            .ok_or_else(|| corrupt("an enumerated undo receipt vanished"))?;
        bytes = bytes
            .checked_add(receipt.retained_bytes())
            .and_then(|value| value.checked_add(receipt.stored_bytes))
            .ok_or_else(|| corrupt("undo store byte accounting overflowed"))?;
        if bytes > options.limits.max_total_undo_bytes as u64 {
            return Err(corrupt("undo store exceeds the configured byte bound"));
        }
    }
    Ok(UndoStoreUsage {
        receipts: ids.len(),
        bytes,
    })
}

pub(in crate::operation) fn discard(
    control: &ControlDir,
    receipt: &StoredReceipt,
    root: &FsRoot,
    options: WorktreeOptions,
) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for (position, path) in receipt.paths().iter().enumerate() {
        let Some(name) = path.backup_name.as_deref() else {
            continue;
        };
        let expected = path
            .backup
            .ok_or_else(|| corrupt("receipt backup evidence is missing"))?;
        let access = root.open_target(&path.path).map_err(|error| {
            undo_io(
                TransactionPhase::Cleanup,
                "failed to reopen undo artifact parent",
                error,
            )
            .at_path(path.path.clone())
            .at_file(position)
        })?;
        let actual = access
            .artifact_evidence(name, options.limits.max_source_bytes_per_file)
            .map_err(|error| {
                undo_io(
                    TransactionPhase::Cleanup,
                    "failed to verify undo artifact",
                    error,
                )
                .at_path(path.path.clone())
                .at_file(position)
            })?;
        if actual != expected {
            return Err(corrupt("undo artifact does not match receipt evidence")
                .at_path(path.path.clone())
                .at_file(position));
        }
        removed += usize::from(access.remove_artifact(name).map_err(|error| {
            undo_io(
                TransactionPhase::Cleanup,
                "failed to remove exact undo artifact",
                error,
            )
            .at_path(path.path.clone())
            .at_file(position)
        })?);
    }
    control
        .remove_undo_receipt(receipt.id())
        .map_err(store_io)?;
    Ok(removed)
}

pub(in crate::operation) fn read_exact(
    control: &ControlDir,
    id: &str,
    options: WorktreeOptions,
) -> Result<Option<StoredReceipt>, WorktreeError> {
    let Some(mut file) = control.open_undo_receipt(id).map_err(store_io)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(store_io)?;
    if !metadata.is_file() || metadata.len() > options.limits.max_journal_bytes as u64 {
        return Err(corrupt("undo receipt is not a bounded regular file"));
    }
    file.seek(SeekFrom::Start(0)).map_err(store_io)?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(options.limits.max_journal_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(store_io)?;
    if bytes.len() > options.limits.max_journal_bytes {
        return Err(corrupt("undo receipt exceeds the configured byte bound"));
    }
    let envelope: ReceiptEnvelope =
        blazingly_json::from_slice(&bytes).map_err(|_| corrupt("undo receipt JSON is invalid"))?;
    if envelope.body.schema != super::SCHEMA
        || envelope.body.transaction_id != id
        || serialized_hash(&envelope.body).map_err(encode_error)? != envelope.checksum
    {
        return Err(corrupt("undo receipt identity or checksum is invalid"));
    }
    validate_body(&envelope.body, options)?;
    Ok(Some(StoredReceipt {
        body: envelope.body,
        checksum: envelope.checksum,
        stored_bytes: bytes.len() as u64,
    }))
}

fn validate_body(body: &ReceiptBody, options: WorktreeOptions) -> Result<(), WorktreeError> {
    if body.paths.is_empty() || body.paths.len() > options.limits.max_files {
        return Err(corrupt("undo receipt path count is invalid"));
    }
    let mut previous = None;
    let mut retained = 0_u64;
    for (position, path) in body.paths.iter().enumerate() {
        if path.index as usize != position
            || weavatrix_refactor_plan::validate_plan_path(&path.path, 4_096).is_err()
        {
            return Err(corrupt("undo receipt contains an invalid path or index"));
        }
        let key = weavatrix_refactor_plan::portable_path_key(&path.path);
        if previous
            .as_ref()
            .is_some_and(|value: &String| value >= &key)
        {
            return Err(corrupt("undo receipt paths alias or are not ordered"));
        }
        previous = Some(key);
        match (path.before, path.backup_name.as_deref(), path.backup) {
            (SlotEvidence::Present(before), Some(name), Some(backup))
                if name
                    == format!(
                        ".weavatrix-{}-{:04}.backup",
                        body.transaction_id, path.index
                    )
                    && before.sha256 == backup.sha256
                    && before.bytes == backup.bytes
                    && before.permissions == backup.permissions =>
            {
                retained = retained
                    .checked_add(backup.bytes)
                    .ok_or_else(|| corrupt("undo retained bytes overflow"))?;
            }
            (SlotEvidence::Absent, None, None) => {}
            _ => return Err(corrupt("undo receipt backup contract is inconsistent")),
        }
    }
    if retained != body.retained_bytes
        || snapshot(&body.paths, true)? != body.before_fingerprint
        || snapshot(&body.paths, false)? != body.after_fingerprint
    {
        return Err(corrupt("undo receipt aggregate evidence is inconsistent"));
    }
    Ok(())
}
