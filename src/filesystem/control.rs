use std::io;

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

use super::sync_directory;

mod retained;

const LEGACY_JOURNAL: &str = "active.jsonl";
const LEGACY_OPERATION_JOURNAL: &str = "active-v2.jsonl";
const OPERATION_JOURNAL: &str = "active-v3.jsonl";
const UNDO_JOURNAL: &str = "active-undo.jsonl";
const UNDO_PREFIX: &str = "undo-";
const UNDO_SUFFIX: &str = ".json";

pub(crate) struct ControlDir {
    dir: Dir,
    device: u64,
}

impl ControlDir {
    pub(crate) const fn new(dir: Dir, device: u64) -> Self {
        Self { dir, device }
    }

    pub(crate) fn open_lock(&self) -> io::Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        options.follow(FollowSymlinks::No);
        self.dir
            .open_with("lock", &options)
            .map(cap_std::fs::File::into_std)
    }

    pub(crate) fn create_journal(&self) -> io::Result<std::fs::File> {
        self.create_named_journal(LEGACY_JOURNAL)
    }

    pub(crate) fn open_journal(&self) -> io::Result<Option<std::fs::File>> {
        self.open_named_journal(LEGACY_JOURNAL)
    }

    pub(crate) fn remove_journal(&self) -> io::Result<()> {
        self.remove_named_journal(LEGACY_JOURNAL)
    }

    pub(crate) fn create_operation_journal(&self) -> io::Result<std::fs::File> {
        self.create_named_journal(OPERATION_JOURNAL)
    }

    pub(crate) fn open_operation_journal(&self) -> io::Result<Option<std::fs::File>> {
        let current = self.open_named_journal(OPERATION_JOURNAL)?;
        let legacy = self.open_named_journal(LEGACY_OPERATION_JOURNAL)?;
        match (current, legacy) {
            (Some(_), Some(_)) => Err(invalid_journal_state(
                "both V2 and V3 operation journals exist",
            )),
            (Some(file), None) | (None, Some(file)) => Ok(Some(file)),
            (None, None) => Ok(None),
        }
    }

    pub(crate) fn remove_operation_journal(&self) -> io::Result<()> {
        let current = self.open_named_journal(OPERATION_JOURNAL)?.is_some();
        let legacy = self.open_named_journal(LEGACY_OPERATION_JOURNAL)?.is_some();
        match (current, legacy) {
            (true, true) => Err(invalid_journal_state(
                "both V2 and V3 operation journals exist",
            )),
            (true, false) => self.remove_named_journal(OPERATION_JOURNAL),
            (false, true) => self.remove_named_journal(LEGACY_OPERATION_JOURNAL),
            (false, false) => Ok(()),
        }
    }

    pub(crate) fn create_undo_journal(&self) -> io::Result<std::fs::File> {
        self.create_named_journal(UNDO_JOURNAL)
    }

    pub(crate) fn open_undo_journal(&self) -> io::Result<Option<std::fs::File>> {
        self.open_named_journal(UNDO_JOURNAL)
    }

    pub(crate) fn remove_undo_journal(&self) -> io::Result<()> {
        self.remove_named_journal(UNDO_JOURNAL)
    }

    pub(crate) fn create_undo_receipt(&self, id: &str) -> io::Result<std::fs::File> {
        self.create_named_journal(&undo_name(id)?)
    }

    pub(crate) fn open_undo_receipt(&self, id: &str) -> io::Result<Option<std::fs::File>> {
        self.open_named_journal(&undo_name(id)?)
    }

    pub(crate) fn remove_undo_receipt(&self, id: &str) -> io::Result<()> {
        self.remove_named_journal(&undo_name(id)?)
    }

    pub(crate) fn undo_receipt_ids(&self) -> io::Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in self.dir.entries()? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid_journal_state("undo receipt name is not UTF-8"))?;
            let Some(id) = name
                .strip_prefix(UNDO_PREFIX)
                .and_then(|value| value.strip_suffix(UNDO_SUFFIX))
            else {
                continue;
            };
            validate_undo_id(id)?;
            ids.push(id.to_owned());
        }
        Ok(ids)
    }

    fn create_named_journal(&self, name: &str) -> io::Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        options.follow(FollowSymlinks::No);
        self.dir
            .open_with(name, &options)
            .map(cap_std::fs::File::into_std)
    }

    fn open_named_journal(&self, name: &str) -> io::Result<Option<std::fs::File>> {
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        options.follow(FollowSymlinks::No);
        match self.dir.open_with(name, &options) {
            Ok(file) => Ok(Some(file.into_std())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn remove_named_journal(&self, name: &str) -> io::Result<()> {
        match self.dir.remove_file(name) {
            Ok(()) => self.sync(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        sync_directory(&self.dir)
    }
}

fn undo_name(id: &str) -> io::Result<String> {
    validate_undo_id(id)?;
    Ok(format!("{UNDO_PREFIX}{id}{UNDO_SUFFIX}"))
}

fn validate_undo_id(id: &str) -> io::Result<()> {
    if id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(invalid_journal_state(
            "undo id is not 32 lowercase hex characters",
        ))
    }
}

fn invalid_journal_state(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
