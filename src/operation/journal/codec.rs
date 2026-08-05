use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{JournalError, Record, invalid};

pub(super) fn checksum(schema: &str, seq: u64, record: &Record) -> Result<String, JournalError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(schema.as_bytes());
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

pub(super) fn encode<T: Serialize>(value: &T, line: usize) -> Result<Vec<u8>, JournalError> {
    blazingly_json::to_vec(value).map_err(|error| {
        invalid(format!(
            "operation JSON encoding failed at line {line}: {error}"
        ))
    })
}
