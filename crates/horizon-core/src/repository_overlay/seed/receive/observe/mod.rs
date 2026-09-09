pub(super) mod layout;

use super::{
    ExpectedGitPack, MAX_OBJECTS, PackReceiveLimits, ReceivedGitPack, SeedError, native, output, staging, view,
};
use std::{
    io::{self, Read},
    path::Path,
};

/// Reopen one explicit existing received pack without creating, repairing, writing,
/// renaming or removing named state. Revalidate the fixed private layout, encoded
/// identity, existing native index and exact shallow commit closure. Missing or
/// unverified input never authorizes another receive, setup or task execution.
/// This is not source-export approval, immutable publication, synchronization or
/// a ready checkout. Existing namespace/seed/setup verification remains required.
/// Requires trusted Git/prlimit and stable exclusive ancestry, mounts and input
/// throughout verification and consumption. Run off the UI thread; filesystem and
/// blocking reads are outside native pipe deadlines. Drop retains all data.
/// # Errors
/// Invalid expectations/limits, missing, unsafe, changed, corrupt or incomplete
/// input, cancellation and native failures return redacted errors without repair.
pub fn observe_git_base_pack(
    path: &Path,
    expected: ExpectedGitPack<'_>,
    limits: PackReceiveLimits,
    cancelled: impl Fn() -> bool,
) -> Result<ReceivedGitPack, SeedError> {
    limits.validate(expected)?;
    staging::check_cancel(&cancelled)?;
    let mut layout = layout::Layout::open(path, expected, &cancelled)?;
    let summary = output::copy(
        &mut |bytes| {
            if cancelled() {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            layout.pack.read(bytes)
        },
        &mut io::sink(),
        expected.encoded_bytes,
        MAX_OBJECTS,
    )?;
    if summary.bytes != expected.encoded_bytes || &summary.sha256 != expected.sha256 {
        return Err(SeedError::Object);
    }
    layout.check_pack_identity(summary.objects)?;
    layout.recheck(&cancelled)?;
    let decoded = path.join("decoded");
    native::verify(
        view::verify_command(
            &decoded,
            &layout.pack_path,
            &layout.index_path,
            limits.source,
            expected.encoded_bytes,
        ),
        limits.source,
        &cancelled,
    )?;
    layout.recheck(&cancelled)?;
    native::enumerate(
        view::stored_closure_command(&decoded, limits.source),
        expected.base_commit,
        summary.objects,
        limits.source,
        &cancelled,
    )?;
    layout.recheck(&cancelled)?;
    Ok(ReceivedGitPack {
        path: path.to_path_buf(),
        objects_directory: decoded.join("objects"),
        base_commit: expected.base_commit,
        sha256: summary.sha256,
        encoded_bytes: summary.bytes,
        objects: summary.objects,
    })
}

#[cfg(test)]
pub(super) mod tests;
