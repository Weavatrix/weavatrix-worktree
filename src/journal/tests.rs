use super::*;
use std::{error::Error, fs::OpenOptions, io::Write};

type TestResult = Result<(), Box<dyn Error>>;

fn assert_error<T, E>(result: Result<T, E>, expected: &str) -> TestResult
where
    E: Error,
{
    let Err(error) = result else {
        return Err(std::io::Error::other("operation unexpectedly succeeded").into());
    };
    assert!(
        error.to_string().contains(expected),
        "expected error containing {expected:?}, got {error}"
    );
    Ok(())
}

fn header() -> JournalRecord {
    JournalRecord::Header {
        transaction_id: "tx-1".into(),
        contract_hash: "a".repeat(64),
        file_count: 1,
    }
}

fn prepared_file() -> JournalRecord {
    JournalRecord::PreparedFile {
        index: 0,
        path: "src/lib.rs".into(),
        old_sha256: "b".repeat(64),
        new_sha256: "c".repeat(64),
        bytes_before: 1,
        bytes_after: 2,
        edit_count: 1,
        stage_name: ".stage".into(),
        backup_name: ".backup".into(),
    }
}

fn open(path: &std::path::Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
}

fn raw_line(seq: u64, record: &JournalRecord, newline: bool) -> Result<Vec<u8>, JournalError> {
    let stored = Stored {
        schema: SCHEMA.into(),
        seq,
        record: record.clone(),
        checksum: checksum(seq, record)?,
    };
    let mut line = encode(&stored, 0)?;
    if newline {
        line.push(b'\n');
    }
    Ok(line)
}

#[test]
fn round_trips_all_types_and_ignores_torn_tail() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let records = [
        header(),
        prepared_file(),
        JournalRecord::Prepared { file_count: 1 },
        JournalRecord::CommitIntent { index: 0 },
        JournalRecord::Committed { index: 0 },
        JournalRecord::RollbackIntent { index: 0 },
        JournalRecord::RolledBack { index: 0 },
        JournalRecord::Finished {
            outcome: FinishOutcome::RolledBack,
        },
    ];
    let mut writer = JournalWriter::new(open(&path)?, 64 * 1024)?;
    for record in &records {
        writer.append(record)?;
    }
    drop(writer);
    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"{\"schema\":")?;
    let entries = read_journal(&File::open(path)?, 64 * 1024)?;
    assert_eq!(
        entries
            .into_iter()
            .map(|entry| entry.record)
            .collect::<Vec<_>>(),
        records
    );
    Ok(())
}

#[test]
fn resume_truncates_a_torn_tail_before_append() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let mut writer = JournalWriter::new(open(&path)?, 64 * 1024)?;
    writer.append(&header())?;
    drop(writer);
    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"{\"torn\":")?;
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    let mut writer = JournalWriter::resume(file, 64 * 1024)?;
    writer.append(&JournalRecord::Prepared { file_count: 1 })?;
    assert_eq!(read_journal(&File::open(path)?, 64 * 1024)?.len(), 2);
    Ok(())
}

#[test]
fn rejects_complete_corruption_and_size() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let mut writer = JournalWriter::new(open(&path)?, 4096)?;
    writer.append(&header())?;
    drop(writer);
    let original = std::fs::read_to_string(&path)?;
    assert_error(read_journal(&File::open(&path)?, 1), "limit is 1")?;
    for (bad, expected) in [
        (
            original.replacen("\"seq\":0", "\"seq\":2", 1),
            "sequence gap",
        ),
        (original.replacen(SCHEMA, "bad.schema", 1), "unknown schema"),
        (original.replacen("tx-1", "tx-x", 1), "checksum mismatch"),
        ("{bad}\n".into(), "invalid JSON"),
        ("\n".into(), "empty record"),
    ] {
        std::fs::write(&path, bad)?;
        assert_error(read_journal(&File::open(&path)?, 4096), expected)?;
    }
    Ok(())
}

#[test]
fn enforces_header_transitions_on_write_and_replay() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    let mut writer = JournalWriter::new(open(&path)?, 4096)?;
    assert_error(
        writer.append(&JournalRecord::Prepared { file_count: 0 }),
        "first record must be header",
    )?;
    writer.append(&header())?;
    assert_error(writer.append(&header()), "duplicate header")?;
    drop(writer);

    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(&raw_line(1, &header(), true)?)?;
    assert_error(read_journal(&File::open(path)?, 4096), "duplicate header")?;
    Ok(())
}

#[test]
fn rejects_zero_nonempty_and_oversize_journals_without_partial_append() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    assert_error(JournalWriter::new(open(&path)?, 0), "must be positive")?;

    std::fs::write(&path, b"occupied")?;
    let nonempty = OpenOptions::new().read(true).write(true).open(&path)?;
    assert_error(JournalWriter::new(nonempty, 4096), "not empty")?;

    let encoded = raw_line(0, &header(), true)?;
    let limit = u64::try_from(encoded.len() - 1)?;
    let mut writer = JournalWriter::new(open(&path)?, limit)?;
    assert_error(writer.append(&header()), "limit is")?;
    assert_eq!(std::fs::metadata(&path)?.len(), 0);
    drop(writer);

    std::fs::write(&path, encoded)?;
    let resume = OpenOptions::new().read(true).write(true).open(&path)?;
    assert_error(JournalWriter::resume(resume, limit), "limit is")?;
    Ok(())
}

#[test]
fn treats_a_whole_non_newline_record_as_one_torn_tail() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("journal.jsonl");
    std::fs::write(&path, raw_line(0, &header(), false)?)?;
    assert!(read_journal(&File::open(path)?, 4096)?.is_empty());
    Ok(())
}
