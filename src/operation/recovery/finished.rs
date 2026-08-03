use crate::{error::WorktreeError, options::WorktreeOptions};

use super::evidence::{current_state, foreign_state, matches_after, matches_before, path_io};
use crate::operation::recovery_model::ParsedJournal;

pub(super) fn verify_finished(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    committed: bool,
) -> Result<(), WorktreeError> {
    for path in &parsed.paths {
        let actual = current_state(path, options)?;
        if !(if committed {
            matches_after(path, actual)
        } else {
            matches_before(path, actual)
        }) {
            return Err(foreign_state(
                parsed,
                path,
                if committed {
                    "finished commit no longer matches exact after evidence"
                } else {
                    "finished rollback no longer matches exact before evidence"
                },
            ));
        }
        path.access.sync_parent().map_err(|error| {
            path_io(
                parsed,
                path,
                "finished operation path directory did not synchronize",
                error,
            )
        })?;
    }
    Ok(())
}
