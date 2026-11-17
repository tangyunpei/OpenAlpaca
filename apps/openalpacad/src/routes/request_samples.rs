//! The request bodies the routes read and the responses only the CLI reads,
//! for the API contract's folder (CLI-PROTOCOL's amended half; *Rev 107*'s
//! request-fields and CLI-reads items, (b)).
//!
//! **Requests** (`requests/`): every JSON body a route deserializes, read
//! off its type ([`Body::of`]), named after its route — the floor the GUI's
//! wrappers and the CLI send, and the rest besides, since one daemon serves
//! both and a body neither sends today is still a request field the
//! PROTOCOL rule binds. A body the handler reads into `serde_json::Value`
//! is read through a type here too (`PluginSourceBody`, `McpDeclaration`).
//! The multipart upload (`POST /v1/files/upload`, its one `file` part) and
//! the query strings are not bodies and are not sampled.
//!
//! **The CLI's own reads** (`cli/`): the responses of the routes only the
//! CLI reads, each built from the type or the builder its handler answers
//! with, as the response producers are.

use crate::contract::Samples;
use crate::contract::requests::Body;

/// Every request body's sample, and every response only the CLI reads.
pub(crate) fn contract_samples(out: &mut Samples) {
    requests(out);
    cli_reads(out);
}

fn requests(out: &mut Samples) {
    use super::agents_types::*;
    use super::chat_types::{ChatSendRequest, ConfirmationBody, FeedbackRequest};
    use super::command::CommandRequest;
    use super::connectors::{ConnectorActionBody, ConnectorConfigBody, ConnectorSettingsBody};
    use super::extensions::{PluginSourceBody, SetConfigRequest};
    use super::followups::QueueFollowupRequest;
    use super::memories::{
        CreateMemoryBody, ForgetMemoriesBody, ForgetTargetBody, PatchBody, SourceBody,
        SupersedesBody, UpdateMemoryBody,
    };
    use super::persona::PersonaUserBody;
    use super::sessions::{CreateSessionRequest, PatchSessionRequest};
    use super::settings_types::{SetProviderEnabledRequest, UpdateWebSearchRequest};
    use super::tasks_types::{
        CreateTaskRequest, RerunTaskRequest, SteerTaskRequest, TaskActionRequest,
    };
    use super::workspaces::{ChooseWorkspaceRequest, PurgeRequest, RebaseRequest};
    use crate::managers::mcp::McpDeclaration;
    use openalpaca_llm::config::settings_types::{
        AddKeyRequest, KeyInput, ReorderKeysRequest, SetKeyPriorityRequest,
        UpdateOrchestratorRequest, ValidateKeyRequest,
    };
    use openalpaca_storage::AttachmentRef;

    out.request("post-command", Body::of::<CommandRequest>());
    // Connectors.
    out.request(
        "post-connectors-id-action",
        Body::of::<ConnectorActionBody>(),
    );
    out.request(
        "post-connectors-id-config",
        Body::of::<ConnectorConfigBody>(),
    );
    out.request(
        "put-connectors-id-settings",
        Body::of::<ConnectorSettingsBody>(),
    );
    // Tasks and follow-ups.
    out.request("post-tasks", Body::of::<CreateTaskRequest>());
    out.request("post-tasks-id-action", Body::of::<TaskActionRequest>());
    out.request("post-tasks-id-steer", Body::of::<SteerTaskRequest>());
    out.request("post-tasks-id-rerun", Body::of::<RerunTaskRequest>());
    out.request(
        "post-lanes-key-followups",
        Body::of::<QueueFollowupRequest>(),
    );
    // Agents and templates.
    out.request(
        "post-agents",
        Body::of::<CreateAgentRequest>().object("config", agent_config()),
    );
    out.request(
        "post-agents-from-toml",
        Body::of::<CreateAgentFromTomlRequest>(),
    );
    out.request(
        "put-agents-id-config",
        Body::of::<UpdateAgentConfigRequest>().object("config", agent_config()),
    );
    out.request("post-agents-id-action", Body::of::<AgentActionRequest>());
    out.request(
        "post-agent-templates",
        Body::of::<CreateTemplateRequest>().object("config", agent_config()),
    );
    out.request(
        "put-agent-templates-id",
        Body::of::<UpdateTemplateRequest>().object("config", agent_config()),
    );
    // Artifacts, workspaces, chat, sessions.
    out.request(
        "put-artifacts-id-pin",
        Body::of::<super::artifacts::PinRequest>(),
    );
    out.request("post-workspaces", Body::of::<ChooseWorkspaceRequest>());
    out.request("patch-workspaces", Body::of::<RebaseRequest>());
    out.request("post-workspaces-purge", Body::of::<PurgeRequest>());
    out.request(
        "post-chat",
        Body::of::<ChatSendRequest>().array("attachments", Body::of::<AttachmentRef>()),
    );
    out.request(
        "put-chat-messages-id-feedback",
        Body::of::<FeedbackRequest>(),
    );
    out.request("post-chat-confirmations-id", Body::of::<ConfirmationBody>());
    out.request("post-sessions", Body::of::<CreateSessionRequest>());
    out.request("patch-sessions-id", Body::of::<PatchSessionRequest>());
    // Settings.
    out.request(
        "put-settings-llm",
        Body::of::<AddKeyRequest>().object("key", Body::of::<KeyInput>()),
    );
    out.request(
        "put-settings-llm-keys-reorder",
        Body::of::<ReorderKeysRequest>(),
    );
    out.request(
        "put-settings-llm-keys-priority",
        Body::of::<SetKeyPriorityRequest>(),
    );
    out.request(
        "post-settings-llm-validate",
        Body::of::<ValidateKeyRequest>(),
    );
    out.request(
        "put-settings-llm-providers-provider-enabled",
        Body::of::<SetProviderEnabledRequest>(),
    );
    out.request(
        "put-orchestrator-config",
        Body::of::<UpdateOrchestratorRequest>(),
    );
    out.request(
        "put-daemon-config-providers-web-search",
        Body::of::<UpdateWebSearchRequest>(),
    );
    out.request("put-persona-user", Body::of::<PersonaUserBody>());
    // Memories.
    out.request(
        "post-memories",
        Body::of::<CreateMemoryBody>()
            .array("sources", Body::of::<SourceBody>())
            .object("supersedes", Body::of::<SupersedesBody>()),
    );
    out.request(
        "post-memories-id",
        Body::of::<UpdateMemoryBody>().object("patch", Body::of::<PatchBody>()),
    );
    out.request(
        "post-memories-forget",
        Body::of::<ForgetMemoriesBody>().array("targets", Body::of::<ForgetTargetBody>()),
    );
    // Extensions.
    out.request("post-extensions-mcp", Body::of::<McpDeclaration>());
    out.request("post-extensions-plugin", Body::of::<PluginSourceBody>());
    out.request(
        "post-extensions-plugin-validate",
        Body::of::<PluginSourceBody>(),
    );
    out.request("put-extensions-plugin-id", Body::of::<PluginSourceBody>());
    out.request(
        "post-extensions-kind-id-config",
        Body::of::<SetConfigRequest>(),
    );
}

/// An agent's config file, the body the template and config writes carry
/// under `config`, every section's keys named.
fn agent_config() -> Body {
    use openalpaca_core::agent::config::{
        AgentCapabilitiesConfig, AgentConfigFile, AgentConstraintsConfig, AgentLlmConfigFile,
        AgentMeta, AgentPresetConfig,
    };
    Body::of::<AgentConfigFile>()
        .object("agent", Body::of::<AgentMeta>())
        .object("capabilities", Body::of::<AgentCapabilitiesConfig>())
        .object("preset", Body::of::<AgentPresetConfig>())
        .object("constraints", Body::of::<AgentConstraintsConfig>())
        .object("llm", Body::of::<AgentLlmConfigFile>())
}

/// The CLI's own reads that need no fixture: `GET /v1/me`'s body, the type
/// its handler answers with. (The agents' and the memories' are their route
/// modules' producers, beside their fixtures.)
fn cli_reads(out: &mut Samples) {
    let me = super::auth::MeResponse {
        user_id: "user".to_string(),
        default_lane_key: "user:cli".to_string(),
        sources: vec!["cli".to_string()],
    };
    out.cli_response("get-me", serde_json::to_value(me).unwrap());
}
