use std::io;

use cap_std::fs::Dir;

pub(crate) fn sync_directory(dir: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    let result = {
        let file = dir.open(".")?;
        rustix::fs::fsync(&file).map_err(io::Error::from)
    };
    #[cfg(not(unix))]
    let result = dir.try_clone()?.into_std_file().sync_all();

    match result {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use cap_std::{ambient_authority, fs::Dir};

    use super::sync_directory;

    #[test]
    fn syncs_directory_opened_as_capability() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();

        sync_directory(&dir).unwrap();
    }
}
