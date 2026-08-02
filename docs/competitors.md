# Competitor and reference audit

Snapshot date: 2026-08-02.

This document compares the filesystem layer needed by `weavatrix-worktree`, not
the quality of the text-edit algorithms that produce new file contents. The
important distinction is between validating a set of edits, replacing one file
atomically, recovering a failed batch, and making an entire multi-file batch
atomically visible. Those are different guarantees.

## Guarantee vocabulary

- **Preflight atomicity**: all known edit and precondition errors are detected
  before the first visible filesystem mutation.
- **Per-file atomic replacement**: a reader sees either the old or new contents
  at one destination path, normally through a same-filesystem rename.
- **Rollback**: operations already made visible are compensated after a later
  operation fails. Rollback can itself fail.
- **Crash recovery**: persistent transaction state is sufficient to finish or
  reverse an interrupted operation after process restart.
- **Durability**: the implementation requests the required file and directory
  synchronization. This is separate from atomic visibility and is still subject
  to the operating system, filesystem, mount, and storage device contract.
- **Multi-file atomic visibility**: no observer can see a mixture of old and new
  files. Ordinary independent file renames do not provide this guarantee for a
  normal source worktree.

## Capability matrix

`Not specified` means the cited layer delegates the behavior to another layer;
it does not mean that every application built on it is unsafe.

The `git apply` row below is the installed system Git CLI. It is unrelated to
[`weavatrix-git`](https://github.com/sergii-ziborov/weavatrix-git), whose public
contract is read-only Git evidence with no checkout or repository mutation.

| System | Scope and preflight | Stale-source guard | Path and link safety | Concurrency | Failure and recovery | Durability contract |
| --- | --- | --- | --- | --- | --- | --- |
| [`textum::PatchSet` 0.4.0](https://docs.rs/textum/0.4.0/src/textum/composer.rs.html) | Groups patches by file, reads every target, resolves snippets, and rejects overlapping non-empty ranges before `write_to_files` starts writing. | No file version or whole-file hash. Snippet matching only protects the addressed text. | Target files are arbitrary strings passed to `std::fs`; there is no workspace jail in `PatchSet`. | Files are prepared and written sequentially. | The crate explicitly documents that a write error can leave some files written and others untouched. There is no batch rollback. | `write_to_files` uses `std::fs::write`; no atomic rename or file/directory sync is provided by this layer. |
| [`rustfix` 0.9.7](https://docs.rs/rustfix/0.9.7/src/rustfix/lib.rs.html) | Low-level, in-memory, single-source transformation of compiler suggestions. It deliberately performs no filesystem I/O. | Byte ranges apply to the supplied in-memory source; no filesystem precondition exists. | Out of scope. | Out of scope. | An individual in-memory suggestion is either applied or rejected; persistence belongs to the caller. | Out of scope. |
| [`cargo fix` 0.98.0](https://docs.rs/cargo/0.98.0/src/cargo/ops/fix/mod.rs.html) | Groups independent suggestions by file, rejects a single suggestion whose replacements span multiple files, iterates compiler/fix passes up to four times, and runs `rustc` again for validation. | Compiler diagnostics are regenerated iteratively, but there is no expected whole-file hash against an unrelated writer. Cargo checks VCS state before starting unless overridden. | Skips Cargo home/registry and sysroot source. This is not a general workspace-root jail. | Uses one global Cargo lock because shared `include!` and `#[path]` files make narrower locking unsafe. The source describes serialization as slower but necessary. | If final validation fails, Cargo writes saved original sources back unless broken-code mode is enabled. This is compensating rollback, not a crash-safe group commit; an I/O error or process termination can interrupt either write direction. | The fix path uses Cargo's ordinary path write helper and does not expose a group fsync/journal contract. |
| [LSP 3.18 `WorkspaceEdit`](https://github.com/microsoft/language-server-protocol/blob/b7f5132c95261c0898ae5124e7a91707abc48fcd/_specifications/lsp/3.18/types/workspaceEdit.md) | Represents changes to many resources and ordered create, rename, delete, and text operations. [`TextDocumentEdit`](https://github.com/microsoft/language-server-protocol/blob/b7f5132c95261c0898ae5124e7a91707abc48fcd/_specifications/lsp/3.18/types/textDocumentEdit.md) requires non-overlapping edits for one document version. | Can carry a document version so a client can reject a stale edit. The version is a client document version, not a filesystem content hash. | Not specified by the protocol; URI resolution and filesystem policy belong to the client. | Not specified. | The client advertises `abort`, `transactional`, `textOnlyTransactional`, or `undo`. `abort` retains earlier operations, and `undo` explicitly has no success guarantee. [`ApplyWorkspaceEditResult`](https://github.com/microsoft/language-server-protocol/blob/b7f5132c95261c0898ae5124e7a91707abc48fcd/_specifications/lsp/3.18/workspace/applyEdit.md) can identify the failed change. | Not specified. |
| [`git apply`](https://git-scm.com/docs/git-apply) | By default, verifies every hunk before modifying the worktree. `--check` is a no-write preflight. `--reject` deliberately opts into partial hunk application. | Unified-diff context is the default guard. `--index` additionally requires matching index/worktree content and metadata; `--3way` can use recorded blob identities. | Rejects paths outside the working area unless `--unsafe-paths` is explicitly used. Git also validates paths and symlink traversal in the apply implementation. | The worktree output loop is sequential in the audited implementation. | A non-applicable hunk leaves the worktree untouched by default. This guarantee covers applicability errors, not a power loss or I/O failure during the later sequential output loop. See the pinned [`check_patch_list`/`write_out_results` flow](https://github.com/git/git/blob/a97fcc37c2bc6340a8d7ce78dedf227aac4e9aa7/apply.c#L4814-L4979). | No multi-file worktree durability contract is documented by `git apply`. The index has separate locking semantics. |
| [`libgit2::git_apply` main](https://libgit2.org/docs/reference/main/apply/git_apply.html) | Can apply to the worktree, index, or both. It constructs postimages before checkout, and [`GIT_APPLY_CHECK`](https://libgit2.org/docs/reference/main/apply/git_apply_flags_t.html) performs a no-write applicability test. | Text hunks are matched exactly at their expected position in the audited implementation; worktree-and-index mode also checks their preimages. It does not expose an independent expected-source-hash parameter. | Uses libgit2 checkout and repository path handling rather than a caller-defined arbitrary workspace jail. | Controlled by the checkout implementation, not by the `git_apply` API contract. | The index writer has a lock/commit path, but worktree checkout has no group rollback contract in `git_apply`. Building every postimage before checkout prevents patch errors after mutation starts; it does not make filesystem output group-atomic. See the pinned [apply implementation](https://github.com/libgit2/libgit2/blob/d2ff991b49fee0ec1bc59e9da3da44e6efb0e779/src/libgit2/apply.c#L557-L842). | No worktree-wide fsync contract is exposed by this API. |
| [`gix-worktree-state::checkout` 0.33.0](https://docs.rs/gix-worktree-state/0.33.0/src/gix_worktree_state/checkout/mod.rs.html) | Materializes an index rather than applying arbitrary text patches. It reports files, bytes, collisions, and per-path errors. | Has stat/freshness and overwrite policies tied to index checkout. | Exposes path-component validation options. The [`gix::index::State` safety note](https://docs.rs/gix/latest/gix/index/struct.State.html#a-note-on-safety) warns that caller-created index paths must be validated before checkout. | Has `thread_limit`; otherwise it generally uses logical CPU count. | `keep_going` intentionally applies as much as possible and records errors, so partial output is part of that mode's contract. It is a useful scheduler/reporting reference, not a transaction implementation. | No batch transaction durability guarantee is documented by checkout. |
| [`atomic-write-file` 0.3.0](https://docs.rs/atomic-write-file/0.3.0/atomic_write_file/) | One destination file. New bytes stay in a same-directory temporary file until `commit`. | No expected-source guard; the destination can change between open and commit. | On Unix it uses directory descriptors and `openat`/`linkat`/`renameat`, keeping the operation attached to the originally opened directory. A destination symlink is replaced rather than followed. | One file per object; callers may schedule independent objects. | Before commit, failure preserves the old destination. It is not a multi-file rollback mechanism. Abrupt termination may leave a named temporary file. | Syncs the temporary file before replacement; since 0.2.0 it also syncs the parent directory on Unix, as recorded in the [changelog](https://docs.rs/crate/atomic-write-file/0.3.0/source/CHANGELOG.md). |
| [`atomwrite` 0.1.35](https://docs.rs/crate/atomwrite/0.1.35) | Direct agent-oriented competitor with read/write/edit/apply, BLAKE3 checksums, expected checksum, batch operations, backups, and transaction mode. | `--expect-checksum` is an explicit optimistic-lock precondition. | Implements a workspace jail, rejects final symlinks by default, checks intermediate symlink escapes, special files, and Windows-hostile names. | Non-transactional batch operations can fan out in parallel when raw target strings are unique; transaction operations stay sequential. | Transaction mode first creates backups, then executes ordered operations and restores backups/removes created files/reverses moves after an error. The pinned source shows [batch staging and scheduling](https://github.com/danilo-aguiar-br/atomwrite/blob/c5aedb669d38938848ce053f1476914429b5c554/src/commands/batch/run.inc.rs#L94-L142) and [rollback](https://github.com/danilo-aguiar-br/atomwrite/blob/c5aedb669d38938848ce053f1476914429b5c554/src/commands/batch/txn.inc.rs#L47-L97). This is compensating rollback, not an atomic multi-path visibility mechanism. | Individual writes advertise temp-file, fsync, rename, and directory fsync. WAL facilities exist, but the audited batch transaction path is backup/restore rather than a single atomic commit record for all paths. |

## Direct `atomwrite` gaps relevant to this design

These findings are based on the pinned source above, not only on package
marketing:

1. Transactional batch preparation creates backups in parallel, but visible
   operations and rollback are sequential. That is a reasonable recoverable
   batch design; it must not be described as atomic visibility across paths.
2. Parallel eligibility compares raw path strings. Aliases, Unicode or case
   normalization, and platform-specific equivalent paths need canonical
   identity checks before they can safely be treated as independent.
3. [`validate_path`](https://github.com/danilo-aguiar-br/atomwrite/blob/c5aedb669d38938848ce053f1476914429b5c554/src/path_safety.rs#L18-L85)
   canonicalizes and checks a path, then returns a normal `PathBuf` used by
   later filesystem calls. It closes straightforward symlink escapes, but the
   check and later operation are still separate path resolutions. Therefore a
   hostile concurrent namespace mutation remains a TOCTOU concern. This last
   point is an inference from the implementation flow.

## Required `weavatrix-worktree` target

This is an acceptance target, not a claim about the current implementation.

### Filesystem boundary

- Open the workspace root once and perform operations relative to that root
  handle where the platform permits it. A capability API such as
  [`cap_std::fs::Dir`](https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html)
  is a better model than `canonicalize` followed by an ambient absolute path.
- Reject absolute paths, `..`, workspace-root replacement, repository metadata
  such as `.git`, intermediate symlinks/reparse points, final symlinks, and
  special files by default.
- Detect duplicate and parent/child-conflicting targets after platform-aware
  normalization, including case-insensitive collisions on relevant filesystems.
- Reject hard-linked regular files by default or require an explicit policy.
  Rename-based replacement changes only one directory entry and can silently
  break the caller's expected hard-link topology.

### Preconditions and preparation

- Require `MustExist`, `MustNotExist`, or an expected content hash for every
  operation. Existing-file modification and deletion should use a hash by
  default.
- Read and hash the source before rendering, then revalidate it immediately
  before the first visible mutation.
- Treat cooperative locks as coordination, not as protection against an
  unrelated process that ignores those locks. Strict portable filesystem CAS
  against an uncooperative writer must not be promised.
- Validate the complete path graph, operation ordering, source states, edit
  overlaps, resource budgets, and staged output before commit.

### Parallelism

- Parallelize bounded read/hash, edit rendering, temporary-file writing, and
  temporary-file synchronization for independent files.
- Keep visible commit ordered and deterministic. Parallel renames do not create
  group atomicity and make rollback ordering harder to reason about.
- Bound workers, files, per-file bytes, total input bytes, total output bytes,
  open handles, and temporary disk usage. Preserve input order in reports even
  when preparation completes out of order.

### Commit, rollback, and recovery

- Stage replacements in destination filesystems before the first visible
  mutation.
- Persist a transaction journal before commit when recoverable mode is enabled.
- Commit in a deterministic order, record progress, and reverse completed steps
  after a later failure.
- Expose at least `Committed`, `RolledBack`, `RollbackIncomplete`, and
  `RecoveryRequired`. Never collapse an incomplete rollback into a generic
  success/failure boolean.
- Sync each staged file and every affected parent directory in durable mode.
  Document platform limitations and distinguish a successfully requested sync
  from proof against every storage-controller failure.

### Honest public wording

An appropriate guarantee is:

> Every operation is prepared and revalidated before commit. Each replacement
> is atomic at its destination path. Recoverable mode journals commit progress
> and attempts deterministic rollback or restart recovery after interruption.

Do not claim that ordinary source-code readers can never observe a mixture of
old and new files. True group visibility requires a generation directory plus
one atomically replaced manifest/pointer and requires all readers to resolve
through that indirection; normal compilers and editors do not do so.
