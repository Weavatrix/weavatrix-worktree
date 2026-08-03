use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    hash::Sha256Hash,
    options::WorktreeOptions,
};
use weavatrix_refactor_plan::validate_plan_path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EndpointEvidence {
    pub(super) sha256: Sha256Hash,
    pub(super) bytes: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ExpectedPath {
    pub(super) path: String,
    pub(super) before: Option<EndpointEvidence>,
    pub(super) after: Option<EndpointEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Modify,
    Create,
    Delete,
    Rename,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_operation(
    kind: &str,
    source: Option<&str>,
    destination: Option<&str>,
    old_sha256: Option<&str>,
    new_sha256: Option<&str>,
    bytes_before: u64,
    bytes_after: u64,
    edit_count: u32,
    options: WorktreeOptions,
    paths: &mut BTreeMap<String, ExpectedPath>,
    inputs: &mut BTreeSet<String>,
    outputs: &mut BTreeSet<String>,
) -> Result<(), WorktreeError> {
    let kind = parse_kind(kind)?;
    if !valid_shape(
        kind,
        source,
        destination,
        old_sha256,
        new_sha256,
        bytes_before,
        bytes_after,
        edit_count,
    ) || bytes_before > options.limits.max_source_bytes_per_file as u64
        || bytes_after > options.limits.max_output_bytes_per_file as u64
        || usize::try_from(edit_count)
            .map_or(true, |count| count > options.limits.max_edits_per_file)
        || old_sha256.is_some_and(|value| Sha256Hash::parse(value).is_err())
        || new_sha256.is_some_and(|value| Sha256Hash::parse(value).is_err())
    {
        return Err(corrupt("invalid or over-limit operation journal evidence"));
    }
    if let (Some(source), Some(hash)) = (source, old_sha256) {
        register_endpoint(source, true, hash, bytes_before, paths, inputs)?;
    }
    let output_path = if kind == Kind::Modify {
        source
    } else {
        destination
    };
    if let (Some(destination), Some(hash)) = (output_path, new_sha256) {
        register_endpoint(destination, false, hash, bytes_after, paths, outputs)?;
    }
    Ok(())
}

fn parse_kind(kind: &str) -> Result<Kind, WorktreeError> {
    match kind {
        "MODIFY" => Ok(Kind::Modify),
        "CREATE" => Ok(Kind::Create),
        "DELETE" => Ok(Kind::Delete),
        "RENAME" => Ok(Kind::Rename),
        _ => Err(corrupt("unknown operation journal kind")),
    }
}

#[allow(clippy::too_many_arguments)]
fn valid_shape(
    kind: Kind,
    source: Option<&str>,
    destination: Option<&str>,
    old_sha256: Option<&str>,
    new_sha256: Option<&str>,
    bytes_before: u64,
    bytes_after: u64,
    edit_count: u32,
) -> bool {
    match kind {
        Kind::Modify => {
            source.is_some()
                && destination.is_none()
                && old_sha256.is_some()
                && new_sha256.is_some()
        }
        Kind::Create => {
            source.is_none()
                && destination.is_some()
                && old_sha256.is_none()
                && new_sha256.is_some()
                && bytes_before == 0
                && edit_count == 0
        }
        Kind::Delete => {
            source.is_some()
                && destination.is_none()
                && old_sha256.is_some()
                && new_sha256.is_none()
                && bytes_after == 0
                && edit_count == 0
        }
        Kind::Rename => {
            source.is_some()
                && destination.is_some()
                && old_sha256.is_some()
                && new_sha256.is_some()
        }
    }
}

fn register_endpoint(
    path: &str,
    before: bool,
    sha256: &str,
    bytes: u64,
    paths: &mut BTreeMap<String, ExpectedPath>,
    roles: &mut BTreeSet<String>,
) -> Result<(), WorktreeError> {
    validate_plan_path(path, 4_096).map_err(|_| corrupt("unsafe operation journal path"))?;
    let key = weavatrix_refactor_plan::portable_path_key(path);
    if !roles.insert(key.clone()) {
        return Err(corrupt(
            "multiple journal operations use the same path role",
        ));
    }
    let expected = paths.entry(key).or_insert_with(|| ExpectedPath {
        path: path.to_owned(),
        before: None,
        after: None,
    });
    if expected.path != path {
        return Err(corrupt("operation journal paths alias portably"));
    }
    let evidence = EndpointEvidence {
        sha256: Sha256Hash::parse(sha256).map_err(|_| corrupt("invalid endpoint hash"))?,
        bytes,
    };
    if before {
        expected.before = Some(evidence);
    } else {
        expected.after = Some(evidence);
    }
    Ok(())
}

fn corrupt(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}
