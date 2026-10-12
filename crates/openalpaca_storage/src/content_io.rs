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

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    /// FIPS 180-2's first known answer, as a literal. `file_assets.sha256`,
    /// every artifact and upload name, a memory note's revision and the
    /// partition keys are this spelling — lower-case hex of SHA-256 — so a
    /// digest API change that compiles but prints differently (sha2 0.11,
    /// deps plan P10) is caught here, not in a store nothing can read back.
    #[test]
    fn sha256_hex_of_abc_is_the_known_answer() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
