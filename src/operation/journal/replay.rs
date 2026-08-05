use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use super::{
    Entry, JournalError, Stored, checksum, invalid, require_limit, schema::JournalSchema,
    too_large, validate_order,
};

pub(in crate::operation) fn read(file: &File, max_bytes: u64) -> Result<Vec<Entry>, JournalError> {
    read_with_schema(file, max_bytes).map(|(_, entries)| entries)
}

pub(super) fn read_with_schema(
    file: &File,
    max_bytes: u64,
) -> Result<(JournalSchema, Vec<Entry>), JournalError> {
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
        return Ok((JournalSchema::current(), Vec::new()));
    };
    parse_complete(&bytes[..last_newline])
}

fn parse_complete(bytes: &[u8]) -> Result<(JournalSchema, Vec<Entry>), JournalError> {
    let mut entries = Vec::new();
    let mut journal_schema = None;
    for (offset, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        let line_number = offset + 1;
        if line.is_empty() {
            return Err(invalid(format!(
                "empty operation record at line {line_number}"
            )));
        }
        let stored: Stored = blazingly_json::from_slice(line).map_err(|error| {
            invalid(format!(
                "invalid operation JSON at line {line_number}: {error}"
            ))
        })?;
        let expected = entries.len() as u64;
        let schema = JournalSchema::parse(&stored.schema)
            .map_err(|error| invalid(format!("{error} at line {line_number}")))?;
        if journal_schema.is_some_and(|expected| expected != schema) {
            return Err(invalid(format!(
                "operation journal schema changes at line {line_number}"
            )));
        }
        journal_schema = Some(schema);
        if stored.seq != expected {
            return Err(invalid(format!(
                "operation sequence gap at line {line_number}: expected {expected}, got {}",
                stored.seq
            )));
        }
        validate_order(stored.seq, &stored.record, line_number as u64)?;
        if stored.checksum != checksum(schema.as_str(), stored.seq, &stored.record)? {
            return Err(invalid(format!(
                "operation checksum mismatch at line {line_number}"
            )));
        }
        entries.push(Entry {
            seq: stored.seq,
            record: stored.record,
        });
    }
    Ok((
        journal_schema.unwrap_or_else(JournalSchema::current),
        entries,
    ))
}
