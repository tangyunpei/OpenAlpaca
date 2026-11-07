//! The watcher's half of the agents directory (subagent design §2.4 L-3):
//! every event rescans the whole directory in `LoadMode::Reload`.

use super::*;
use crate::test_util::HomeStoreGuard;
use openalpaca_core::agent::AgentRegistry;
use openalpaca_core::agent::file_layer::{LoadMode, rescan_file_layer};
use openalpaca_core::tools::ToolRegistry;
use openalpaca_storage::Database;

const READER: &str =
    "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \"file_read\"\n---\n";

/// A file deleted and another edited to name a capability nothing knows:
/// one watcher reload takes the first out and withholds the second, and
/// the daemon goes on (the start would have refused the second).
#[test]
fn a_watcher_reload_rescans_and_withholds_an_unknown_capability() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&dir.path().canonicalize().unwrap().join("home"));
    let agents = dir.path().join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("reader.md"), READER).unwrap();
    std::fs::write(agents.join("writer.md"), READER.replace("reader", "writer")).unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let registry = AgentRegistry::new();
    let tool_registry = ToolRegistry::default();
    rescan_file_layer(&agents, LoadMode::Start, &registry, &tool_registry, &db).unwrap();

    std::fs::remove_file(agents.join("reader.md")).unwrap();
    std::fs::write(
        agents.join("writer.md"),
        READER
            .replace("reader", "writer")
            .replace("\"file_read\"", "\"annotation:bogus\""),
    )
    .unwrap();
    reload_agent_templates(&agents, &registry, &tool_registry, &db);

    assert!(registry.get_template("reader").is_none());
    let err = registry
        .spawn_instance("writer", "t1".to_string())
        .unwrap_err();
    assert!(err.contains("is withheld"), "{err}");
}
