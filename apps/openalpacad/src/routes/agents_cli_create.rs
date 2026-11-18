//! AGENT-CREATE-CLI: the body both of the CLI's interactive agent creations
//! send — `agent create` and the `openalpaca config` screen, one function,
//! `commands::agents::create_agent_body` — is taken by `POST /v1/agents`.
//!
//! The CLI's test pins that body whole in
//! `apps/openalpaca/src/commands/agents/create-bodies.json`; this test
//! posts each pinned body the way the handler does — read as the route's
//! own [`CreateAgentRequest`], then the agent config service's
//! `create_agent` — and finds the agent registered with what was asked.
//! Under the home-store guard, in temporary folders; an `AppState` is not
//! constructible in a unit test, and the handler is those two steps.

use std::sync::Arc;

use super::agents_types::CreateAgentRequest;
use crate::test_util::HomeStoreGuard;
use openalpaca_core::agent::{AgentConfigService, AgentRegistry};
use openalpaca_storage::{Database, SubAgentRepository};

const BODIES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca/src/commands/agents/create-bodies.json"
);

#[test]
fn the_cli_interactive_creation_body_is_taken_by_post_v1_agents() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = HomeStoreGuard::set(&dir.path().canonicalize().unwrap().join("home"));
    let agents = dir.path().join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let registry = Arc::new(AgentRegistry::new());
    let service = AgentConfigService::new(registry.clone(), agents.clone(), db.clone());

    let bodies: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(BODIES).expect("the CLI's pinned bodies")).unwrap();
    assert_eq!(bodies.len(), 2);
    for body in bodies {
        let request: CreateAgentRequest = serde_json::from_value(body.clone())
            .unwrap_or_else(|e| panic!("POST /v1/agents refuses the CLI's body ({e}): {body}"));
        let id = service
            .create_agent(request.config)
            .unwrap_or_else(|e| panic!("the agent is not created ({e}): {body}"));
        assert_eq!(id, body["config"]["agent"]["id"].as_str().unwrap());
        let agent = registry.get(&id).expect("registered");
        assert_eq!(
            agent.name,
            body["config"]["agent"]["name"].as_str().unwrap()
        );
        assert_eq!(
            agent.llm_config.model.as_deref(),
            body["config"]["llm"]["model"].as_str()
        );
        let assigned: Vec<String> = agent.capabilities.iter().map(|c| c.name.clone()).collect();
        assert_eq!(
            serde_json::json!(assigned),
            body["config"]["capabilities"]["assigned"]
        );
        let row = SubAgentRepository::new(&db)
            .get(&id)
            .unwrap()
            .expect("stored");
        assert_eq!(
            row.persona.as_deref(),
            body["config"]["preset"]["persona"].as_str()
        );
        assert!(agents.join(format!("{id}.toml")).exists());
    }
}
