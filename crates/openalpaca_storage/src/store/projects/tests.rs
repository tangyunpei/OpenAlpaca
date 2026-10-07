//! `store::projects` (storage design rev 0.8, Stage 1, slice S1). Every test
//! that reaches a home store holds the crate's `HomeStoreGuard`, through
//! `World` or directly, so none reads or writes the developer's
//! `~/.openalpaca`.

use super::*;

/// A v4 UUID in its canonical spelling (third group starts with 4, fourth
/// with 9).
const ID: &str = "0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b";

#[test]
fn a_project_id_reads_only_its_canonical_spelling() {
    let id = ProjectId::parse(ID).unwrap();
    assert_eq!(id.to_string(), ID);
    assert_eq!(ID.parse::<ProjectId>().unwrap(), id);
    for other in [
        "0B0E0D4C-5A6F-4E3D-9C2B-1A0F9E8D7C6B",
        "{0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b}",
        "0b0e0d4c5a6f4e3d9c2b1a0f9e8d7c6b",
        "urn:uuid:0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b",
        "",
        "not-an-id",
    ] {
        assert!(
            ProjectId::parse(other).is_err(),
            "{other:?} must be refused"
        );
    }
}

#[test]
fn a_new_project_id_is_a_v4_uuid_in_its_canonical_spelling() {
    let id = ProjectId::new();
    assert_eq!(id.as_uuid().get_version_num(), 4);
    assert_eq!(ProjectId::parse(&id.to_string()).unwrap(), id);
    assert_ne!(ProjectId::new(), id);
}

#[test]
fn a_project_id_travels_through_json_as_its_canonical_string() {
    let id = ProjectId::parse(ID).unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{ID}\""));
    assert_eq!(
        serde_json::from_str::<ProjectId>(&format!("\"{ID}\"")).unwrap(),
        id
    );
    let upper = format!("\"{}\"", ID.to_uppercase());
    assert!(serde_json::from_str::<ProjectId>(&upper).is_err());
}
