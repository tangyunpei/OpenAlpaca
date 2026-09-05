//! The filesystem primitives the two content writers share.
//!
//! [`crate::artifacts::ArtifactStore`] and [`crate::uploads::UploadStore`] place
//! bytes under the same store roots with the same durability rules, so hashing,
//! directory `fsync` and best-effort cleanup live here once rather than being
//! copied per writer. Nothing here decides *where* a file goes — that is
//! [`crate::store`]'s job — and nothing here touches the database.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

/// The lowercase hex SHA-256 of `bytes` — the `file_assets.sha256` spelling.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// `fsync` a directory so a rename that just happened is durable.
///
/// An addition to the plan's literal §4.2 protocol, which forces only the tmp
/// file's *data*: a rename is a directory-metadata change, and without this two
/// renames could persist out of order under power loss. Best-effort — the bytes
/// are already durable, so a filesystem that will not let us open or sync a
/// directory must not fail the write.
pub(crate) fn fsync_dir(dir: &Path) {
    match fs::File::open(dir) {
        Ok(handle) => {
            if let Err(e) = handle.sync_all() {
                tracing::debug!("Failed to fsync {}: {e}", dir.display());
            }
        }
        Err(e) => tracing::debug!("Failed to open {} for fsync: {e}", dir.display()),
    }
}

/// Deletes a file that may not be there. A stray file is a leak, never a reason
/// to fail a write whose bytes are already in place.
pub(crate) fn remove_best_effort(path: &Path) {
    if let Err(e) = fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("Failed to remove {}: {e}", path.display());
    }
}
