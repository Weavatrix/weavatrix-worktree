use std::io;

use cap_std::fs::Dir;

pub(crate) fn sync_directory(dir: &Dir) -> io::Result<()> {
    match dir.try_clone()?.into_std_file().sync_all() {
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
