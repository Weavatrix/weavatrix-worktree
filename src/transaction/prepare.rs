use weavatrix_edit::EditPlan;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FsRoot,
    hash::Sha256Hash,
    journal::{FinishOutcome, JournalRecord, JournalWriter},
    options::WorktreeOptions,
};

use super::{
    PreparedTransaction,
    lock::acquire,
    plan::{dry_run_report, project_plan},
    stage::stage_all,
    util::journal_error,
};

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn prepare_transaction(
    root: &FsRoot,
    options: WorktreeOptions,
    plan: &EditPlan,
) -> Result<PreparedTransaction, WorktreeError> {
    let locked = acquire(root)?;
    if locked
        .control
        .open_journal()
        .map_err(open_journal_error)?
        .is_some()
    {
        return Err(WorktreeError::new(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Lock,
            "a previous transaction journal must be recovered first",
        )
        .requiring_recovery());
    }
    let projected = project_plan(root, options, plan)?;
    let preview = dry_run_report(plan, &projected);
    let transaction_id = random_id()?;
    let journal_file = locked.control.create_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Prepare,
            "failed to create an exclusive transaction journal",
            error,
        )
    })?;
    locked.control.sync().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::DurabilityFailed,
            TransactionPhase::Prepare,
            "failed to synchronize the journal directory",
            error,
        )
    })?;
    let mut journal = JournalWriter::new(journal_file, options.limits.max_journal_bytes as u64)
        .map_err(|error| journal_error(TransactionPhase::Prepare, "invalid new journal", error))?;
    append_header(&mut journal, plan, &transaction_id)
        .map_err(|error| error.in_transaction(transaction_id.clone()))?;
    let staged = match stage_all(projected, &transaction_id, options, &mut journal) {
        Ok(staged) => staged,
        Err(error) if error.recovery_required() => return Err(error),
        Err(error) => {
            finish_aborted(&mut journal, &locked.control)?;
            return Err(error);
        }
    };
    journal
        .append(&JournalRecord::Prepared {
            file_count: u32::try_from(staged.len()).map_err(|_| {
                WorktreeError::new(
                    WorktreeErrorCode::TransactionTooLarge,
                    TransactionPhase::Prepare,
                    "file count does not fit the journal contract",
                )
            })?,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Prepare,
                "failed to record durable preparation",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(PreparedTransaction {
        transaction_id,
        operation: plan.operation.clone(),
        preview,
        files: staged,
        options,
        journal,
        control: locked.control,
        _lock: locked.file,
    })
}

fn append_header(
    journal: &mut JournalWriter,
    plan: &EditPlan,
    transaction_id: &str,
) -> Result<(), WorktreeError> {
    let encoded = blazingly_json::to_vec(plan).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Prepare,
            "failed to encode the validated plan contract",
            error,
        )
    })?;
    journal
        .append(&JournalRecord::Header {
            transaction_id: transaction_id.to_owned(),
            contract_hash: Sha256Hash::compute(&encoded).to_string(),
            file_count: u32::try_from(plan.files.len()).map_err(|_| {
                WorktreeError::new(
                    WorktreeErrorCode::TransactionTooLarge,
                    TransactionPhase::Prepare,
                    "file count does not fit the journal contract",
                )
            })?,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Prepare,
                "failed to synchronize the journal header",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(())
}

fn finish_aborted(
    journal: &mut JournalWriter,
    control: &crate::filesystem::ControlDir,
) -> Result<(), WorktreeError> {
    journal
        .append(&JournalRecord::Finished {
            outcome: FinishOutcome::Aborted,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Cleanup,
                "failed to record aborted preparation",
                error,
            )
            .requiring_recovery()
        })?;
    control.remove_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Cleanup,
            "failed to remove the aborted journal",
            error,
        )
        .requiring_recovery()
    })
}

fn random_id() -> Result<String, WorktreeError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Prepare,
            "failed to generate a transaction identifier",
            error,
        )
    })?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(32), |mut id, byte| {
            id.push(char::from(HEX[(byte >> 4) as usize]));
            id.push(char::from(HEX[(byte & 15) as usize]));
            id
        }))
}

fn open_journal_error(error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::Io,
        TransactionPhase::Lock,
        "failed to inspect pending recovery state",
        error,
    )
}
