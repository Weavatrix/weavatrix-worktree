use std::io;

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

use super::sync_directory;

pub(crate) struct ControlDir {
    dir: Dir,
}

impl ControlDir {
    pub(crate) const fn new(dir: Dir) -> Self {
        Self { dir }
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
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        options.follow(FollowSymlinks::No);
        self.dir
            .open_with("active.jsonl", &options)
            .map(cap_std::fs::File::into_std)
    }

    pub(crate) fn open_journal(&self) -> io::Result<Option<std::fs::File>> {
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        options.follow(FollowSymlinks::No);
        match self.dir.open_with("active.jsonl", &options) {
            Ok(file) => Ok(Some(file.into_std())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn remove_journal(&self) -> io::Result<()> {
        match self.dir.remove_file("active.jsonl") {
            Ok(()) => self.sync(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        sync_directory(&self.dir)
    }
}
