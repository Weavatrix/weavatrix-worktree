use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
pub(crate) const SCHEMA: &str = "weavatrix.worktree-journal.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinishOutcome {
    Committed,
    RolledBack,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum JournalRecord {
    Header {
        transaction_id: String,
        contract_hash: String,
        file_count: u32,
    },
    PreparedFile {
        index: u32,
        path: String,
        old_sha256: String,
        new_sha256: String,
        bytes_before: u64,
        bytes_after: u64,
        edit_count: u32,
        stage_name: String,
        backup_name: String,
    },
    Prepared {
        file_count: u32,
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
pub(crate) struct JournalEntry {
    pub(crate) seq: u64,
    pub(crate) record: JournalRecord,
}

#[derive(Debug)]
pub(crate) struct JournalError(String);

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for JournalError {}

impl From<std::io::Error> for JournalError {
    fn from(error: std::io::Error) -> Self {
        Self(format!("journal I/O failed: {error}"))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema: String,
    seq: u64,
    record: JournalRecord,
    checksum: String,
}

pub(crate) struct JournalWriter {
    file: File,
    next_seq: u64,
    bytes: u64,
    max_bytes: u64,
    poisoned: bool,
}

impl JournalWriter {
    pub(crate) fn new(file: File, max_bytes: u64) -> Result<Self, JournalError> {
        require_limit(max_bytes)?;
        let bytes = file.metadata()?.len();
        if bytes != 0 {
            return Err(invalid(format!("new journal is not empty ({bytes} bytes)")));
        }
        Ok(Self {
            file,
            next_seq: 0,
            bytes,
            max_bytes,
            poisoned: false,
        })
    }

    pub(crate) fn append(&mut self, record: &JournalRecord) -> Result<u64, JournalError> {
        if self.poisoned {
            return Err(invalid("journal writer is poisoned"));
        }
        validate_order(self.next_seq, record, self.next_seq.saturating_add(1))?;
        let seq = self.next_seq;
        let stored = Stored {
            schema: SCHEMA.into(),
            seq,
            record: record.clone(),
            checksum: checksum(seq, record)?,
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
            .ok_or_else(|| invalid("sequence overflow"))?;
        self.poisoned = false;
        Ok(seq)
    }

    pub(crate) fn resume(mut file: File, max_bytes: u64) -> Result<Self, JournalError> {
        let entries = read_journal(&file, max_bytes)?;
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
            next_seq: entries.len() as u64,
            bytes: complete_len as u64,
            max_bytes,
            poisoned: false,
        })
    }
}

pub(crate) fn read_journal(file: &File, max_bytes: u64) -> Result<Vec<JournalEntry>, JournalError> {
    require_limit(max_bytes)?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        return Err(too_large(max_bytes, size));
    }
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large(max_bytes, bytes.len() as u64));
    }
    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return Ok(Vec::new());
    };
    parse_complete(&bytes[..last_newline])
}

fn parse_complete(bytes: &[u8]) -> Result<Vec<JournalEntry>, JournalError> {
    let mut entries = Vec::new();
    for (offset, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        let line_number = offset + 1;
        if line.is_empty() {
            return Err(invalid(format!("empty record at line {line_number}")));
        }
        let stored: Stored = blazingly_json::from_slice(line)
            .map_err(|error| invalid(format!("invalid JSON at line {line_number}: {error}")))?;
        let expected = entries.len() as u64;
        if stored.schema != SCHEMA {
            return Err(invalid(format!("unknown schema at line {line_number}")));
        }
        if stored.seq != expected {
            return Err(invalid(format!(
                "sequence gap at line {line_number}: expected {expected}, got {}",
                stored.seq
            )));
        }
        validate_order(stored.seq, &stored.record, line_number as u64)?;
        if stored.checksum != checksum(stored.seq, &stored.record)? {
            return Err(invalid(format!("checksum mismatch at line {line_number}")));
        }
        entries.push(JournalEntry {
            seq: stored.seq,
            record: stored.record,
        });
    }
    Ok(entries)
}

fn validate_order(seq: u64, record: &JournalRecord, line: u64) -> Result<(), JournalError> {
    match (seq, record) {
        (0, JournalRecord::Header { .. }) => Ok(()),
        (0, _) => Err(invalid(format!(
            "first record must be header at line {line}"
        ))),
        (_, JournalRecord::Header { .. }) => {
            Err(invalid(format!("duplicate header at line {line}")))
        }
        _ => Ok(()),
    }
}

fn checksum(seq: u64, record: &JournalRecord) -> Result<String, JournalError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(SCHEMA.as_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(encode(record, 0)?);
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut value, byte| {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 15)]));
            value
        }))
}

fn encode<T: Serialize>(value: &T, line: usize) -> Result<Vec<u8>, JournalError> {
    blazingly_json::to_vec(value)
        .map_err(|error| invalid(format!("JSON encoding failed at line {line}: {error}")))
}

fn require_limit(max_bytes: u64) -> Result<(), JournalError> {
    if max_bytes == 0 {
        Err(invalid("journal byte limit must be positive"))
    } else {
        Ok(())
    }
}

fn too_large(max: u64, actual: u64) -> JournalError {
    invalid(format!("journal is {actual} bytes; limit is {max}"))
}

fn invalid(message: impl Into<String>) -> JournalError {
    JournalError(message.into())
}

#[cfg(test)]
mod tests;
