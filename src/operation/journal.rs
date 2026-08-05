use std::{
    error::Error,
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

use crate::{
    filesystem::{FileIdentity, PortablePermissions},
    journal::FinishOutcome,
};
use serde::{Deserialize, Serialize};

mod codec;
mod replay;
mod schema;

use codec::{checksum, encode};
pub(super) use replay::read;
use replay::read_with_schema;
use schema::JournalSchema;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum StateRecord {
    Absent,
    Present {
        sha256: String,
        bytes: u64,
        permissions: PortablePermissions,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<FileIdentity>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Record {
    Header {
        transaction_id: String,
        contract_hash: String,
        operation: String,
        operation_count: u32,
        path_count: u32,
    },
    Operation {
        index: u32,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_sha256: Option<String>,
        bytes_before: u64,
        bytes_after: u64,
        edit_count: u32,
    },
    PathIntent {
        index: u32,
        path: String,
        before: StateRecord,
        after: StateRecord,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backup_name: Option<String>,
    },
    PathStaged {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage_identity: Option<FileIdentity>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backup_identity: Option<FileIdentity>,
    },
    Prepared {
        operation_count: u32,
        path_count: u32,
    },
    CommitIntent {
        index: u32,
    },
    Committed {
        index: u32,
    },
    RollbackIntent {
        index: u32,
    },
    RolledBack {
        index: u32,
    },
    Finished {
        outcome: FinishOutcome,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Entry {
    pub(super) seq: u64,
    pub(super) record: Record,
}

#[derive(Debug)]
pub(super) struct JournalError(String);

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for JournalError {}

impl From<std::io::Error> for JournalError {
    fn from(error: std::io::Error) -> Self {
        Self(format!("operation journal I/O failed: {error}"))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema: String,
    seq: u64,
    record: Record,
    checksum: String,
}

pub(super) struct Writer {
    file: File,
    schema: JournalSchema,
    next_seq: u64,
    bytes: u64,
    max_bytes: u64,
    poisoned: bool,
}

impl Writer {
    pub(super) fn new(file: File, max_bytes: u64) -> Result<Self, JournalError> {
        require_limit(max_bytes)?;
        let bytes = file.metadata()?.len();
        if bytes != 0 {
            return Err(invalid(format!(
                "new operation journal is not empty ({bytes} bytes)"
            )));
        }
        Ok(Self {
            file,
            schema: JournalSchema::current(),
            next_seq: 0,
            bytes,
            max_bytes,
            poisoned: false,
        })
    }

    pub(super) fn append(&mut self, record: &Record) -> Result<u64, JournalError> {
        if self.poisoned {
            return Err(invalid("operation journal writer is poisoned"));
        }
        validate_order(self.next_seq, record, self.next_seq.saturating_add(1))?;
        let seq = self.next_seq;
        let stored = Stored {
            schema: self.schema.as_str().to_owned(),
            seq,
            record: record.clone(),
            checksum: checksum(self.schema.as_str(), seq, record)?,
        };
        let mut line = encode(&stored, 0)?;
        line.push(b'\n');
        let actual = self.bytes.saturating_add(line.len() as u64);
        if actual > self.max_bytes {
            return Err(too_large(self.max_bytes, actual));
        }
        self.poisoned = true;
        self.file.write_all(&line)?;
        self.file.flush()?;
        self.file.sync_all()?;
        self.bytes = actual;
        self.next_seq = seq
            .checked_add(1)
            .ok_or_else(|| invalid("operation journal sequence overflow"))?;
        self.poisoned = false;
        Ok(seq)
    }

    pub(super) fn resume(mut file: File, max_bytes: u64) -> Result<Self, JournalError> {
        let (schema, entries) = read_with_schema(&file, max_bytes)?;
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        (&mut file)
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(too_large(max_bytes, bytes.len() as u64));
        }
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        file.set_len(complete_len as u64)?;
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            schema,
            next_seq: entries.len() as u64,
            bytes: complete_len as u64,
            max_bytes,
            poisoned: false,
        })
    }

    #[cfg(test)]
    pub(super) fn new_legacy_fixture(file: File, max_bytes: u64) -> Result<Self, JournalError> {
        let mut writer = Self::new(file, max_bytes)?;
        writer.schema = JournalSchema::V2;
        Ok(writer)
    }
}

fn validate_order(seq: u64, record: &Record, line: u64) -> Result<(), JournalError> {
    match (seq, record) {
        (0, Record::Header { .. }) => Ok(()),
        (0, _) => Err(invalid(format!(
            "first operation record must be header at line {line}"
        ))),
        (_, Record::Header { .. }) => Err(invalid(format!(
            "duplicate operation header at line {line}"
        ))),
        _ => Ok(()),
    }
}

fn require_limit(max_bytes: u64) -> Result<(), JournalError> {
    if max_bytes == 0 {
        Err(invalid("operation journal byte limit must be positive"))
    } else {
        Ok(())
    }
}

fn too_large(max: u64, actual: u64) -> JournalError {
    invalid(format!(
        "operation journal is {actual} bytes; limit is {max}"
    ))
}

fn invalid(message: impl Into<String>) -> JournalError {
    JournalError(message.into())
}
