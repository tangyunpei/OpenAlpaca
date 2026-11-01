//! SHA-256 in the one spelling OpenAlpaca writes: lower-case hex, two digits a
//! byte.
//!
//! `file_assets.sha256`, the hash in a by-product's name, a memory note's
//! revision, the owner and partition keys, a project folder's name suffix, a
//! session log's spill and pre-edit hashes, `USER.md`'s `ETag` and the
//! daemon's config-change rings are all this string, and the persisted ones
//! name files and rows nothing can find again if it changes. So it is spelled
//! here once: every digest is printed through [`sha256_hex`] or, for a hash
//! taken in pieces, [`lower_hex`]. Never print a digest with `{:?}`: on sha2
//! 0.11 it compiles and prints the byte array, a different string.

use sha2::{Digest, Sha256};

/// The lower-case hex SHA-256 of `bytes`: 64 characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// `bytes` in lower-case hex, two digits a byte. [`sha256_hex`] is this over
/// a SHA-256; a hash fed in pieces — a file streamed rather than read whole —
/// is finished here, so it reads the same as one taken at once.
pub fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{lower_hex, sha256_hex};

    /// FIPS 180-2's first known answer, as a literal. Every name and row
    /// listed in the module's header is this spelling, so a digest API change
    /// that compiles but prints differently (sha2 0.11, deps plan P10) is
    /// caught here, not in a store nothing can read back.
    #[test]
    fn sha256_of_abc_is_the_fips_known_answer() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Two digits a byte: a byte below 0x10 keeps its leading zero, which a
    /// hex format with no width would drop.
    #[test]
    fn every_byte_is_two_lower_case_digits() {
        assert_eq!(lower_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(lower_hex(&[]), "");
    }
}
