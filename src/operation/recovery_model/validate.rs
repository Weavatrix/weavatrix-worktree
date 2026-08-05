use std::collections::{BTreeMap, BTreeSet};

use super::operation::ExpectedPath;
use super::{PresentSpec, RecoveryPath, StateSpec, operation::EndpointEvidence};
use crate::operation::journal::{Entry, Record, StateRecord};
use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FileIdentity,
    hash::Sha256Hash,
    journal::FinishOutcome,
    options::WorktreeOptions,
};

#[derive(Clone, Copy)]
pub(super) struct Header<'entry> {
    pub(super) transaction_id: &'entry str,
    pub(super) operation_count: u32,
    pub(super) path_count: u32,
}

pub(super) fn parse_header(
    entries: &[Entry],
    options: WorktreeOptions,
) -> Result<Header<'_>, WorktreeError> {
    let Some(Entry {
        record:
            Record::Header {
                transaction_id,
                contract_hash,
                operation,
                operation_count,
                path_count,
            },
        ..
    }) = entries.first()
    else {
        return Err(corrupt("operation journal does not begin with a header"));
    };
    if transaction_id.len() != 32
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || Sha256Hash::parse(contract_hash).is_err()
        || operation.is_empty()
        || operation.len() > options.limits.max_operation_bytes
        || *operation_count == 0
        || *path_count == 0
        || usize::try_from(*operation_count).map_or(true, |count| count > options.limits.max_files)
        || usize::try_from(*path_count).map_or(true, |count| count > options.limits.max_files)
    {
        return Err(corrupt("invalid or over-limit operation journal header"));
    }
    Ok(Header {
        transaction_id,
        operation_count: *operation_count,
        path_count: *path_count,
    })
}

pub(super) fn parse_state(
    record: &StateRecord,
    options: WorktreeOptions,
    require_identity: bool,
) -> Result<StateSpec, WorktreeError> {
    match record {
        StateRecord::Absent => Ok(StateSpec::Absent),
        StateRecord::Present {
            sha256,
            bytes,
            permissions,
            identity,
        } => {
            let limit = if require_identity {
                options.limits.max_source_bytes_per_file
            } else {
                options.limits.max_output_bytes_per_file
            };
            if *bytes > limit as u64 || (require_identity && identity.is_none()) {
                return Err(corrupt("invalid or over-limit operation path state"));
            }
            Ok(StateSpec::Present(PresentSpec {
                sha256: Sha256Hash::parse(sha256)
                    .map_err(|_| corrupt("invalid operation path state SHA-256"))?,
                bytes: *bytes,
                permissions: *permissions,
                identity: *identity,
            }))
        }
    }
}

pub(super) fn validate_artifact_contract(
    transaction_id: &str,
    index: u32,
    before: &StateSpec,
    after: &StateSpec,
    stage_name: Option<&str>,
    backup_name: Option<&str>,
) -> Result<(), WorktreeError> {
    let expected_stage = format!(".weavatrix-{transaction_id}-{index:04}.stage");
    let expected_backup = format!(".weavatrix-{transaction_id}-{index:04}.backup");
    if stage_name != after.is_present().then_some(expected_stage.as_str())
        || backup_name != before.is_present().then_some(expected_backup.as_str())
        || matches!((before, after), (StateSpec::Absent, StateSpec::Absent))
    {
        return Err(corrupt(
            "operation artifact names do not match their path transition",
        ));
    }
    Ok(())
}

pub(super) fn validate_staged_identity(
    path: &RecoveryPath,
    stage_identity: Option<FileIdentity>,
    backup_identity: Option<FileIdentity>,
) -> Result<(), WorktreeError> {
    if stage_identity.is_some() != path.after.is_present()
        || backup_identity.is_some() != path.before.is_present()
        || path
            .after
            .present()
            .and_then(|state| state.identity)
            .is_some_and(|identity| Some(identity) != stage_identity)
    {
        return Err(corrupt("PathStaged identities do not match PathIntent"));
    }
    Ok(())
}

pub(super) fn validate_expected_path(
    expected: &BTreeMap<String, ExpectedPath>,
    path: &str,
    before: &StateSpec,
    after: &StateSpec,
) -> Result<(), WorktreeError> {
    let Some(expected) = expected.get(&weavatrix_refactor_plan::portable_path_key(path)) else {
        return Err(corrupt("journal path is not owned by an operation"));
    };
    if expected.path != path
        || !matches_endpoint(expected.before, before)
        || !matches_endpoint(expected.after, after)
    {
        return Err(corrupt("journal path transition contradicts its operation"));
    }
    Ok(())
}

pub(super) fn validate_expected_paths(
    expected: &BTreeMap<String, ExpectedPath>,
    actual: &[RecoveryPath],
) -> Result<(), WorktreeError> {
    if expected.len() != actual.len() {
        return Err(corrupt("journal path intents do not match operations"));
    }
    for path in actual {
        validate_expected_path(expected, &path.path, &path.before, &path.after)?;
    }
    Ok(())
}

fn matches_endpoint(expected: Option<EndpointEvidence>, actual: &StateSpec) -> bool {
    match (expected, actual) {
        (None, StateSpec::Absent) => true,
        (Some(expected), StateSpec::Present(actual)) => {
            expected.sha256 == actual.sha256 && expected.bytes == actual.bytes
        }
        (None, StateSpec::Present(_)) | (Some(_), StateSpec::Absent) => false,
    }
}

pub(super) fn validate_finished(
    outcome: &FinishOutcome,
    prepared: bool,
    path_count: usize,
    intents: &BTreeSet<u32>,
    committed: &BTreeSet<u32>,
) -> Result<(), WorktreeError> {
    match outcome {
        FinishOutcome::Committed
            if prepared && committed.len() == path_count && intents == committed =>
        {
            Ok(())
        }
        FinishOutcome::Aborted if intents.is_empty() => Ok(()),
        FinishOutcome::RolledBack if prepared && !intents.is_empty() => Ok(()),
        _ => Err(corrupt(
            "Finished outcome contradicts the operation journal state",
        )),
    }
}

pub(super) fn validate_path_index(index: u32, paths: &[RecoveryPath]) -> Result<(), WorktreeError> {
    if usize::try_from(index).map_or(true, |index| index >= paths.len()) {
        Err(corrupt("journal record references an unknown path index"))
    } else {
        Ok(())
    }
}

fn corrupt(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}

#[cfg(test)]
mod tests {
    use super::parse_header;
    use crate::{
        operation::journal::{Entry, Record},
        options::WorktreeOptions,
    };

    fn header(operation_count: u32, path_count: u32) -> Entry {
        Entry {
            seq: 0,
            record: Record::Header {
                transaction_id: "0123456789abcdef0123456789abcdef".to_owned(),
                contract_hash: "0".repeat(64),
                operation: "fixture".to_owned(),
                operation_count,
                path_count,
            },
        }
    }

    #[test]
    fn strict_header_rejects_empty_and_over_limit_counts() {
        assert!(parse_header(&[header(1, 1)], WorktreeOptions::default()).is_ok());
        assert!(parse_header(&[header(0, 1)], WorktreeOptions::default()).is_err());
        assert!(parse_header(&[header(1, 0)], WorktreeOptions::default()).is_err());
        assert!(parse_header(&[header(65, 1)], WorktreeOptions::default()).is_err());
    }
}
