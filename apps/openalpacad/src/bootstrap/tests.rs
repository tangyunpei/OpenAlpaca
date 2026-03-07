use super::*;
use super::persona::{ensure_soul_file, ensure_soul_template_file};
use openalpaca_core::middleware::prompt::SystemPersona;
use std::path::PathBuf;

fn make_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
    dir
}

#[test]
fn test_bootstrap_system_persona_creates_template_and_soul() {
    let dir = make_temp_dir("openalpaca-soul-bootstrap");
    let (persona, soul_path) = bootstrap_system_persona(&dir);

    assert_eq!(persona.name, "OpenAlpaca");
    assert!(soul_path.exists());
    assert!(
        dir.join("orchestrator")
            .join("templates")
            .join("SOUL_temp.md")
            .exists()
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_bootstrap_system_persona_falls_back_on_invalid_soul() {
    let dir = make_temp_dir("openalpaca-soul-invalid");
    let template_path = ensure_soul_template_file(&dir).expect("template should bootstrap");
    let soul_path = ensure_soul_file(&dir, &template_path).expect("soul file should bootstrap");

    std::fs::write(&soul_path, "invalid").expect("test should write invalid soul");
    let (persona, loaded_path) = bootstrap_system_persona(&dir);

    assert_eq!(loaded_path, soul_path);
    assert_eq!(persona.name, SystemPersona::default().name);
    assert_eq!(persona.core_values, SystemPersona::default().core_values);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_is_same_file_path_matches_identical_files() {
    let dir = make_temp_dir("openalpaca-soul-path");
    let file = dir.join("SOUL.md");
    std::fs::write(&file, "x").expect("test file should be writable");

    let canonical = std::fs::canonicalize(&file).expect("file should canonicalize");
    assert!(is_same_file_path(&file, &canonical));

    let _ = std::fs::remove_dir_all(dir);
}
