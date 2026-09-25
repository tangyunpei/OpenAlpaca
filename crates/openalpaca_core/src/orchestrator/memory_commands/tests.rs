//! The chat memory commands (design 09 §12, MD-13), driven through the
//! orchestrator's own front door — and, where the transcript matters, through
//! a gateway that persists its turns and feeds a manual learning queue.
//!
//! Every model here is scripted. [`ScriptedLlm::forbidden`] is the provider
//! design 09 §12.4 asks for: a memory command never reaches a model, so a call
//! panics where it is made and is counted for the assertion at the end (a
//! call on a task the test does not await would not fail it otherwise). The
//! home store is a temp root; nothing reaches a network.

use super::*;
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::gateway::{HandleRequest, ResolvedAttachment};
use crate::lane::LaneManager;
use crate::memory::learning::{LearningDeps, LearningQueue, QueueMode};
use crate::memory::service::{
    Attribution, Capabilities, ExtractionProvenance, ListInput, MemoryAccessContext, MemoryService,
    StoreInput, StoreOutput, Surface, WorkspaceHint,
};
use crate::middleware::prompt::SystemPersona;
use crate::orchestrator::{skill_catalog::SkillCatalog, skill_router::SkillRouter};
use crate::runner::LoopConfig;
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::security::sandbox::SandboxManager;
use crate::test_util::HomeStoreGuard;
use crate::tools::ToolRegistry;
use arc_swap::ArcSwap;
use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmRouter, Usage};
use openalpaca_storage::memory_files::{
    self as files, NoteKind, NoteScope, Origin, Partition, ReplacePolicy,
};
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{ConversationRepository, Database, IdentityRepository};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const OWNER: &str = "u_memory_commands";

fn owner() -> Principal {
    Principal::User {
        global_id: OWNER.to_string(),
    }
}

/// The scripted provider. Forbidden: any call is a failure. Otherwise it
/// answers a turn with "noted" and the memory extractor with no items, and
/// keeps every request for the assertions.
struct ScriptedLlm {
    forbidden: bool,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    fn forbidden() -> Arc<Self> {
        Arc::new(Self {
            forbidden: true,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn answering() -> Arc<Self> {
        Arc::new(Self {
            forbidden: false,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    fn is_extractor_call(request: &ChatRequest) -> bool {
        request.messages.iter().any(|m| {
            matches!(m.role, openalpaca_llm::Role::System) && m.content.contains("memory extractor")
        })
    }

    fn extractor_calls(&self) -> Vec<ChatRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|r| Self::is_extractor_call(r))
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl openalpaca_llm::LlmProvider for ScriptedLlm {
    fn name(&self) -> &str {
        "scripted-memory-commands"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let last = request.messages.last().map(|m| m.content.clone());
        let extractor = Self::is_extractor_call(&request);
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(request);
        assert!(
            !self.forbidden,
            "design 09 §12: a memory command reached the model: {last:?}"
        );
        Ok(ChatResponse {
            content: if extractor {
                r#"{"items": []}"#.to_string()
            } else {
                "noted".to_string()
            },
            tool_calls: vec![],
            model: "scripted".to_string(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        })
    }
}

/// An owner, their empty home partition, the service over it with its bus,
/// and an orchestrator that reaches the memory service and `llm`.
struct Scene {
    _env: HomeStoreGuard,
    root: tempfile::TempDir,
    db: Database,
    service: Arc<MemoryService>,
    bus: EventBus,
    orch: Arc<Orchestrator>,
    llm: Arc<ScriptedLlm>,
    router: Arc<LlmRouter>,
}

impl Scene {
    fn new() -> Self {
        Self::with_llm(ScriptedLlm::forbidden())
    }

    fn with_llm(llm: Arc<ScriptedLlm>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home.canonicalize().unwrap());
        let db = Database::open(&root.path().join("t.db")).unwrap();
        IdentityRepository::new(&db)
            .create_global_user(OWNER, None)
            .unwrap();
        let bus = EventBus::new(256);
        let service = MemoryService::new(Some(db.clone()), MemoryIndex::open_in_memory().unwrap())
            .with_bus(bus.clone());
        service.rebuild_index().unwrap();
        let service = Arc::new(service);
        let router = Arc::new(LlmRouter::single_provider(
            llm.clone(),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        ));
        let registry = Arc::new(ToolRegistry::default());
        let gate = Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
            registry.clone(),
            bus.clone(),
        ))));
        let orch = Orchestrator::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            bus.clone(),
            SystemPersona::default(),
            Some(router.clone()),
            LoopConfig::default(),
            gate,
            registry,
            Some(db.clone()),
            Arc::new(SkillCatalog::new()),
            Arc::new(SkillRouter::new(0.65, 0.45)),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        );
        orch.set_memory_service(service.clone());
        Self {
            _env: env,
            root,
            db,
            service,
            bus,
            orch: Arc::new(orch),
            llm,
            router,
        }
    }

    /// A project directory with a `.git` marker, canonical.
    fn project(&self, name: &str) -> PathBuf {
        let dir = self.root.path().join("ws").join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir.canonicalize().unwrap()
    }

    /// The owner's user-directed context, for reading back what a command did.
    fn user_ctx(&self, project: Option<&Path>) -> MemoryAccessContext {
        let hint = match project {
            Some(p) => WorkspaceHint::Path(p),
            None => WorkspaceHint::None,
        };
        self.service
            .context(
                OWNER,
                hint,
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .unwrap()
    }

    fn save(&self, title: &str, body: &str, kind: NoteKind) -> StoreOutput {
        self.service
            .store(
                &self.user_ctx(None),
                StoreInput {
                    title: title.to_string(),
                    body: body.to_string(),
                    kind: Some(kind),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// Every active note, as `(title, kind, scope, origin)`.
    fn notes(&self, project: Option<&Path>) -> Vec<(String, NoteKind, NoteScope, Origin)> {
        let mut items: Vec<_> = self
            .service
            .list(&self.user_ctx(project), ListInput::default())
            .unwrap()
            .items
            .into_iter()
            .map(|i| (i.title, i.kind, i.scope, i.origin))
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
    }

    fn body_of(&self, title: &str) -> String {
        let ctx = self.user_ctx(None);
        let item = self
            .service
            .list(&ctx, ListInput::default())
            .unwrap()
            .items
            .into_iter()
            .find(|i| i.title == title)
            .expect("the note");
        self.service
            .read(
                &ctx,
                crate::memory::service::ReadInput {
                    id: item.id,
                    ..Default::default()
                },
            )
            .unwrap()
            .body
    }

    async fn say(&self, content: &str) -> String {
        self.say_as(turn(content)).await
    }

    async fn say_as(&self, request: HandleRequest) -> String {
        self.orch
            .handle_message(request)
            .await
            .expect("a memory command always answers")
    }
}

/// An attended turn of the app's chat, for the owner, with no project.
fn turn(content: &str) -> HandleRequest {
    HandleRequest {
        request_id: Uuid::new_v4(),
        source: "gui".to_string(),
        content: content.to_string(),
        principal: owner(),
        scope: Scope::Global,
        lane_key: format!("{OWNER}:gui"),
        workspace_path: None,
        stream_id: None,
        model_override: None,
        unattended: false,
        turn_sink: None,
    }
}

/// `(change, surface, origin)` of every `memory_changed` received so far.
fn memory_changes(
    rx: &mut tokio::sync::broadcast::Receiver<SystemEvent>,
) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::MemoryChanged {
            change,
            surface,
            origin,
            ..
        } = event
        {
            out.push((change, surface, origin));
        }
    }
    out
}

fn id8(id: files::NoteId) -> String {
    id.to_string()[..8].to_string()
}

/// The notes directory of the owner's home partition.
fn home_notes_dir(scene: &Scene) -> PathBuf {
    scene
        .root
        .path()
        .join("home")
        .canonicalize()
        .unwrap()
        .join("memory/owners")
        .join(files::owner_key(OWNER))
        .join("notes")
}

fn has_file(dir: &Path, id: files::NoteId) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(&id.to_string()))
}

// ── The daemon's `OrchestratorHandler`, and a gateway that learns ─────────

struct Turns(Arc<Orchestrator>);

#[async_trait::async_trait]
impl crate::gateway::MessageHandler for Turns {
    async fn handle(&self, request: HandleRequest) -> Result<crate::gateway::HandleResult, String> {
        self.0
            .handle_message(request)
            .await
            .map(crate::gateway::HandleResult::text)
    }

    async fn handle_with_attachments(
        &self,
        request: HandleRequest,
        attachments: Vec<ResolvedAttachment>,
    ) -> Result<crate::gateway::HandleResult, String> {
        self.0
            .handle_message_with_attachments(request, attachments)
            .await
            .map(crate::gateway::HandleResult::text)
    }
}

/// A gateway that persists every turn and feeds a manual learning queue, in
/// front of the scene's orchestrator.
struct Learning {
    gateway: crate::gateway::Gateway,
    queue: Arc<LearningQueue>,
}

impl Learning {
    fn new(scene: &Scene) -> Self {
        let queue = LearningQueue::new(
            LearningDeps {
                db: scene.db.clone(),
                router: scene.router.clone(),
                daemon_config: Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
                memory_service: scene.service.clone(),
            },
            QueueMode::Manual,
        );
        let gateway = crate::gateway::Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(Turns(scene.orch.clone())),
            scene.bus.clone(),
            Some(scene.db.clone()),
        )
        .with_learning(queue.clone());
        Self { gateway, queue }
    }

    async fn say(&self, content: &str) -> crate::gateway::GatewayResponse {
        self.gateway
            .handle_event(crate::gateway::GatewayRequest {
                source: openalpaca_api::events::EventSource::Gui {
                    connection_id: OWNER.to_string(),
                },
                content: content.to_string(),
                attachments: Vec::new(),
                principal: owner(),
                scope: Scope::Global,
                workspace_path: None,
                stream_id: None,
                lane_override: None,
                model_override: None,
                unattended: false,
                turn_sink: None,
            })
            .await
    }

    async fn flush(&self, scene: &Scene) -> crate::memory::learning::FlushReport {
        let session = ConversationRepository::new(&scene.db)
            .active_session_id(&format!("{OWNER}:gui"))
            .unwrap()
            .expect("a turn opened the session");
        self.queue.flush_session(&session).await
    }
}

// ── /remember ─────────────────────────────────────────────────────────────

/// **Design 09 §12.4, evidence 2.** `/remember …` on an attended app turn
/// writes one note of origin `user` through a `UserSurface` context, with
/// **no model call at all** — not to answer, and not to learn from it later.
/// One `memory_changed`, surface `user`; the intent is `memory_command`.
#[tokio::test]
async fn remember_on_an_attended_app_turn_saves_one_user_note_with_no_model_call() {
    let scene = Scene::new();
    let learning = Learning::new(&scene);
    let mut rx = scene.bus.subscribe();

    let reply = learning
        .say("/remember I prefer tabs over spaces.\nIn every repository, always.")
        .await;

    assert!(!reply.is_error, "{}", reply.content);
    let notes = scene.notes(None);
    assert_eq!(
        notes,
        vec![(
            "I prefer tabs over spaces.".to_string(),
            NoteKind::Fact,
            NoteScope::Global,
            Origin::User
        )]
    );
    let id = scene
        .service
        .list(&scene.user_ctx(None), ListInput::default())
        .unwrap()
        .items[0]
        .id;
    assert_eq!(
        reply.content,
        format!(
            "Saved as a note you wrote: “I prefer tabs over spaces.” ({}, global). It is used from your next message.",
            id8(id)
        )
    );
    assert_eq!(
        scene.body_of("I prefer tabs over spaces."),
        "I prefer tabs over spaces.\nIn every repository, always.",
        "the body is the whole text, as typed"
    );
    let mut changes = Vec::new();
    let mut intents = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::MemoryChanged {
                change,
                surface,
                origin,
                ..
            } => changes.push((change, surface, origin)),
            SystemEvent::IntentClassified { intent_type, .. } => intents.push(intent_type),
            _ => {}
        }
    }
    assert_eq!(
        changes,
        vec![(
            "created".to_string(),
            Some("user".to_string()),
            "user".to_string()
        )]
    );
    assert_eq!(intents, vec!["memory_command".to_string()]);

    let report = learning.flush(&scene).await;
    assert_eq!(report.calls, 0, "{report:?}");
    assert_eq!(scene.llm.calls(), 0, "no model was called");
}

/// The title is the first line cut at 80 characters on a character boundary;
/// the body is the whole text.
#[tokio::test]
async fn a_long_first_line_is_cut_to_80_characters_for_the_title() {
    let scene = Scene::new();
    let line = "é".repeat(100);
    let reply = scene.say(&format!("/remember {line}\nsecond line")).await;
    assert!(reply.starts_with("Saved as a note you wrote"), "{reply}");
    let notes = scene.notes(None);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, "é".repeat(80));
    assert_eq!(
        scene.body_of(&"é".repeat(80)),
        format!("{line}\nsecond line")
    );
    assert_eq!(scene.llm.calls(), 0);
}

/// `--kind` takes any of the six kinds, in any case; the scope defaults by
/// kind against the turn's own project (a preference is global, the other
/// kinds go to the project); `--global` and `--project` override it.
#[tokio::test]
async fn remember_takes_a_kind_and_a_scope() {
    let scene = Scene::new();
    let repo = scene.project("lantern");
    let in_repo = |content: &str| HandleRequest {
        workspace_path: Some(repo.to_string_lossy().into_owned()),
        ..turn(content)
    };
    for (content, scope) in [
        ("/remember Ship with make release", "project"),
        ("/remember --kind preference Dark mode everywhere", "global"),
        ("/remember --global --kind DECISION Use Postgres", "global"),
        (
            "/remember --kind=procedure --project Run make check first",
            "project",
        ),
        (
            "/remember --kind Reference   Docs live at docs.rs",
            "project",
        ),
        (
            "/remember --project --kind episode Moved the office",
            "project",
        ),
    ] {
        let reply = scene.say_as(in_repo(content)).await;
        assert!(
            reply.starts_with("Saved as a note you wrote")
                && reply.contains(&format!(", {scope})")),
            "{content}: {reply}"
        );
    }
    let all = scene.notes(Some(&repo));
    let expected = vec![
        (
            "Dark mode everywhere",
            NoteKind::Preference,
            NoteScope::Global,
        ),
        (
            "Docs live at docs.rs",
            NoteKind::Reference,
            NoteScope::Project,
        ),
        ("Moved the office", NoteKind::Episode, NoteScope::Project),
        (
            "Run make check first",
            NoteKind::Procedure,
            NoteScope::Project,
        ),
        ("Ship with make release", NoteKind::Fact, NoteScope::Project),
        ("Use Postgres", NoteKind::Decision, NoteScope::Global),
    ];
    assert_eq!(
        all.iter()
            .map(|(t, k, s, o)| {
                assert_eq!(*o, Origin::User, "{t}");
                (t.as_str(), *k, *s)
            })
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(scene.llm.calls(), 0);
}

/// What `/remember` cannot save it refuses in the refusal's own words, and
/// writes nothing: `--project` with no project root (`invalid_input`, §7b
/// P1), an unknown kind, both scopes, an unknown option, and no text. `--`
/// ends the options.
#[tokio::test]
async fn remember_refuses_what_it_cannot_save_and_writes_nothing() {
    let scene = Scene::new();
    let mut rx = scene.bus.subscribe();
    for (content, words) in [
        ("/remember --project Deploy notes", "needs a project"),
        ("/remember --kind bogus Some text", "fact, preference"),
        ("/remember --kind", "--kind"),
        ("/remember --global --project Both", "--global"),
        ("/remember --kind fact --kind decision Twice", "--kind"),
        ("/remember --verbose logs are useful", "--verbose"),
        ("/remember", "after /remember"),
        ("/remember --kind fact   ", "after /remember"),
    ] {
        let reply = scene.say(content).await;
        assert!(
            reply.starts_with("Nothing was saved: invalid input:") && reply.contains(words),
            "{content}: {reply}"
        );
    }
    assert!(scene.notes(None).is_empty(), "nothing was written");
    assert!(memory_changes(&mut rx).is_empty());

    let reply = scene.say("/remember -- --global is a flag name").await;
    assert!(reply.starts_with("Saved as a note you wrote"), "{reply}");
    assert_eq!(scene.notes(None)[0].0, "--global is a flag name");
    assert_eq!(scene.llm.calls(), 0);
}

/// §7b P8: the same text twice answers the note already there and writes
/// nothing new — one `memory_changed` for the one write.
#[tokio::test]
async fn remembering_the_same_text_twice_answers_the_existing_note() {
    let scene = Scene::new();
    let mut rx = scene.bus.subscribe();
    let first = scene.say("/remember I use a standing desk").await;
    let second = scene.say("/remember I use a standing desk").await;
    let id = scene
        .service
        .list(&scene.user_ctx(None), ListInput::default())
        .unwrap()
        .items[0]
        .id;
    assert!(first.contains(&id8(id)), "{first}");
    assert_eq!(
        second,
        format!(
            "Already saved: “I use a standing desk” ({}, global). Nothing new was written.",
            id8(id)
        )
    );
    assert_eq!(scene.notes(None).len(), 1);
    assert_eq!(memory_changes(&mut rx).len(), 1, "one write, one event");
}

// ── Where the commands run (§12.2) ────────────────────────────────────────

/// A connector's turn, an unattended app turn, a scheduled skill, a
/// follow-up and a `/v1/command process` turn get one fixed sentence; a turn
/// with no owner, and a daemon with no memory service, another. Nothing is
/// written, nothing is read, no model is called.
#[tokio::test]
async fn memory_commands_are_refused_off_an_attended_app_or_cli_turn() {
    let scene = Scene::new();
    let saved = scene.save("Editor", "I use helix.", NoteKind::Preference);
    let mut rx = scene.bus.subscribe();
    let commands = [
        "/remember I prefer tabs".to_string(),
        format!("/forget {}", id8(saved.id)),
        "/forget editor".to_string(),
        "/memories".to_string(),
        "/memories editor".to_string(),
    ];
    let not_here = [
        // Connectors, for their linked owner.
        ("telegram", owner(), false),
        ("discord", owner(), false),
        ("imessage", owner(), false),
        // A piped `openalpaca chat`: unattended.
        ("gui", owner(), true),
        // A scheduled skill (unattended) and a follow-up (either).
        ("internal", owner(), true),
        ("internal", owner(), false),
        // `/v1/command process`: the `System` principal on the API.
        ("api", Principal::System, false),
    ];
    for (source, principal, unattended) in not_here {
        for command in &commands {
            let reply = scene
                .say_as(HandleRequest {
                    source: source.to_string(),
                    principal: principal.clone(),
                    unattended,
                    lane_key: format!("{OWNER}:{source}"),
                    ..turn(command)
                })
                .await;
            assert_eq!(
                reply, NOT_HERE,
                "{source} (unattended: {unattended}) {command}"
            );
        }
    }
    let unavailable = [
        ("gui", Principal::System),
        (
            "cli",
            Principal::User {
                global_id: "u_nobody".to_string(),
            },
        ),
    ];
    for (source, principal) in unavailable {
        for command in &commands {
            let reply = scene
                .say_as(HandleRequest {
                    source: source.to_string(),
                    principal: principal.clone(),
                    ..turn(command)
                })
                .await;
            assert_eq!(reply, UNAVAILABLE, "{source} {principal:?} {command}");
        }
    }
    // An unlinked sender never reaches the handler: the gate refuses the
    // turn itself.
    for command in &commands {
        let refused = scene
            .orch
            .handle_message(HandleRequest {
                source: "telegram".to_string(),
                principal: Principal::External {
                    provider: "telegram".to_string(),
                    id: "12345".to_string(),
                },
                ..turn(command)
            })
            .await;
        assert!(refused.is_err(), "{command}: {refused:?}");
    }
    assert_eq!(
        scene.notes(None),
        vec![(
            "Editor".to_string(),
            NoteKind::Preference,
            NoteScope::Global,
            Origin::User
        )],
        "nothing was written or forgotten"
    );
    assert!(memory_changes(&mut rx).is_empty());
    assert_eq!(scene.llm.calls(), 0);
}

/// L9 review: a piped `openalpaca chat` is an unattended turn of the CLI's
/// own chat, so its refusal names what is missing — a terminal — instead of
/// telling a CLI user that the commands work "in the CLI". The same
/// sentence still answers a connector's turn.
#[tokio::test]
async fn a_piped_cli_turn_is_told_that_the_commands_need_a_terminal() {
    let scene = Scene::new();
    let expected = "Memory commands work only in the OpenAlpaca app's chat, or in \
                    `openalpaca chat` run at a terminal — not with its input or output piped.";
    for (source, unattended) in [("gui", true), ("telegram", false)] {
        let reply = scene
            .say_as(HandleRequest {
                source: source.to_string(),
                principal: owner(),
                unattended,
                lane_key: format!("{OWNER}:{source}"),
                ..turn("/memories")
            })
            .await;
        assert_eq!(reply, expected, "{source} (unattended: {unattended})");
    }
}

/// A daemon with no memory service answers the same sentence as a turn with
/// no owner.
#[tokio::test]
async fn memory_commands_without_a_memory_service_say_memory_is_not_available() {
    let with = Scene::new();
    let orch = Orchestrator::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        with.bus.clone(),
        SystemPersona::default(),
        Some(with.router.clone()),
        LoopConfig::default(),
        Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
            Arc::new(ToolRegistry::default()),
            with.bus.clone(),
        )))),
        Arc::new(ToolRegistry::default()),
        Some(with.db.clone()),
        Arc::new(SkillCatalog::new()),
        Arc::new(SkillRouter::new(0.65, 0.45)),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    );
    for command in ["/remember x", "/forget 1a2b3c4d", "/memories"] {
        assert_eq!(
            orch.handle_message(turn(command)).await.unwrap(),
            UNAVAILABLE,
            "{command}"
        );
    }
    assert_eq!(with.llm.calls(), 0);
}

/// §12.2: a memory-command turn's files are skipped with a reason — a
/// command saves the text typed, never a file.
#[tokio::test]
async fn a_memory_command_skips_the_turns_attachments_with_a_reason() {
    let scene = Scene::new();
    let request = turn("/remember I use tabs");
    let request_id = request.request_id;
    let reply = scene
        .orch
        .handle_message_with_attachments(
            request,
            vec![ResolvedAttachment {
                file_id: "doc-9".to_string(),
                filename: "notes.txt".to_string(),
                mime_type: "text/plain".to_string(),
                size_bytes: 30,
                extracted_text: Some("the codeword is PLATYPUS".to_string()),
                storage_path: "/dev/null".to_string(),
            }],
        )
        .await
        .unwrap();
    assert!(reply.starts_with("Saved as a note you wrote"), "{reply}");
    assert_eq!(
        scene.body_of("I use tabs"),
        "I use tabs",
        "the file is not saved"
    );
    let skipped = scene
        .orch
        .attachments_skipped_map
        .remove(&request_id)
        .map(|(_, v)| v)
        .expect("the turn records its attachments skipped");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].id, "doc-9");
    assert!(
        skipped[0].reason.contains("memory command"),
        "{}",
        skipped[0].reason
    );
    assert_eq!(scene.llm.calls(), 0);
}

// ── /forget ───────────────────────────────────────────────────────────────

/// **Evidence 5.** `/forget <id8>` forgets exactly that note: its tombstone
/// is in the snapshot, its file is gone, the other note stays; the answer
/// names it and says what a forget leaves behind. A full id in capitals
/// works too.
#[tokio::test]
async fn forget_by_id_prefix_forgets_exactly_that_note() {
    let scene = Scene::new();
    let editor = scene.save("Editor", "I use helix.", NoteKind::Preference);
    let laptop = scene.save("Laptop", "A 2024 MacBook.", NoteKind::Fact);
    let desk = scene.save("Desk", "A standing desk.", NoteKind::Fact);
    let mut rx = scene.bus.subscribe();

    let reply = scene.say(&format!("/forget {}", id8(editor.id))).await;

    assert!(
        reply.starts_with(&format!("Forgot “Editor” ({}). ", id8(editor.id))),
        "{reply}"
    );
    assert!(
        reply.contains("will not be re-extracted"),
        "the service's retained-copies sentence follows: {reply}"
    );
    let snapshot = Partition::open(&StoreScope::Home, OWNER)
        .unwrap()
        .snapshot()
        .unwrap();
    assert!(snapshot.tombstone(&editor.id).is_some(), "tombstone first");
    let notes = home_notes_dir(&scene);
    assert!(!has_file(&notes, editor.id), "the file is gone");
    assert!(has_file(&notes, laptop.id) && has_file(&notes, desk.id));
    assert_eq!(
        memory_changes(&mut rx),
        vec![(
            "forgotten".to_string(),
            Some("user".to_string()),
            "user".to_string()
        )]
    );

    let full = laptop.id.to_string().to_uppercase();
    let reply = scene.say(&format!("/forget {full}")).await;
    assert!(reply.starts_with("Forgot “Laptop”"), "{reply}");
    assert_eq!(
        scene
            .notes(None)
            .into_iter()
            .map(|n| n.0)
            .collect::<Vec<_>>(),
        vec!["Desk"]
    );
    assert_eq!(scene.llm.calls(), 0);
}

/// An ambiguous prefix lists the notes it matches, by their full ids, and
/// forgets nothing; a prefix nothing has says so.
#[tokio::test]
async fn forget_with_an_ambiguous_prefix_lists_and_forgets_nothing() {
    let scene = Scene::new();
    scene.save("Seed", "Makes the partition.", NoteKind::Fact);
    let dir = home_notes_dir(&scene);
    for n in 1..=2 {
        let id = format!("abcdef12-0000-4000-8000-00000000000{n}");
        std::fs::write(
            dir.join(format!("{id}--twin-{n}.md")),
            format!(
                "---\nschema: 1\nid: {id}\ntitle: Twin {n}\nkind: fact\nstatus: active\ncreated_at: 2026-09-23T10:0{n}:00Z\nupdated_at: 2026-09-23T10:0{n}:00Z\n---\n# Twin {n}\n\nA twin.\n"
            ),
        )
        .unwrap();
    }
    scene.service.sync(&scene.user_ctx(None)).unwrap();
    let mut rx = scene.bus.subscribe();

    let reply = scene.say("/forget abcdef12").await;

    assert!(
        reply.starts_with("`abcdef12` matches 2 notes, so nothing was forgotten."),
        "{reply}"
    );
    for n in 1..=2 {
        assert!(
            reply.contains(&format!(
                "abcdef12-0000-4000-8000-00000000000{n}  Twin {n}  [fact/global]"
            )),
            "{reply}"
        );
    }
    assert_eq!(scene.notes(None).len(), 3, "nothing was forgotten");
    assert!(memory_changes(&mut rx).is_empty());

    let reply = scene.say("/forget 00000000").await;
    assert_eq!(
        reply,
        "No note of yours has an id starting with `00000000`. Nothing was forgotten."
    );
    assert_eq!(scene.notes(None).len(), 3);
    assert_eq!(scene.llm.calls(), 0);
}

/// `/forget <text>` searches and **forgets nothing**: it lists up to five
/// matches, one line each, and says how to forget one.
#[tokio::test]
async fn forget_with_text_lists_matches_and_forgets_nothing() {
    let scene = Scene::new();
    let editor = scene.save("Editor", "I use helix for editing.", NoteKind::Preference);
    for n in 0..7 {
        scene.save(
            &format!("Editing habit {n}"),
            "Something about editing.",
            NoteKind::Fact,
        );
    }
    let mut rx = scene.bus.subscribe();

    let reply = scene.say("/forget editing").await;

    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 6, "five matches and the hint: {reply}");
    assert_eq!(lines[5], "Send /forget <id> to forget one.");
    assert!(!reply.contains("Something about"), "no body: {reply}");
    assert_eq!(
        scene.say("/forget helix").await,
        format!(
            "{}  Editor  [preference/global]\nSend /forget <id> to forget one.",
            id8(editor.id)
        )
    );
    assert_eq!(scene.notes(None).len(), 8, "nothing was forgotten");
    assert!(memory_changes(&mut rx).is_empty());

    assert_eq!(
        scene.say("/forget quantum chromodynamics").await,
        "Nothing matched “quantum chromodynamics”."
    );
    let bare = scene.say("/forget").await;
    assert!(bare.contains("/forget <id>"), "{bare}");
    assert_eq!(scene.notes(None).len(), 8);
    assert_eq!(scene.llm.calls(), 0);
}

/// L4 re-review: a listed title is what the person picks an id by, so it
/// carries no Unicode *format* character (category Cf) — a bidi override
/// (U+202E) makes `gpj.exe` read as "exe.jpg", a zero-width character
/// hides text — in `/memories`, in `/forget <text>`'s matches, or in
/// `/remember`'s echo.
#[tokio::test]
async fn listed_titles_carry_no_format_characters() {
    let scene = Scene::new();
    let invoice = scene.save(
        "Invoice \u{202e}gpj.exe\u{202c}\u{200b}",
        "The invoice attachment.",
        NoteKind::Fact,
    );
    let format = regex::Regex::new(r"\p{Cf}").unwrap();
    let line = format!("{}  Invoice gpj.exe  [fact/global]", id8(invoice.id));
    let listed = scene.say("/memories").await;
    assert!(!format.is_match(&listed), "{listed:?}");
    assert!(listed.lines().any(|l| l == line), "{listed}");
    let matches = scene.say("/forget invoice").await;
    assert!(!format.is_match(&matches), "{matches:?}");
    assert!(matches.lines().any(|l| l == line), "{matches}");
    let saved = scene
        .say("/remember Receipt \u{2066}\u{202e}fdp.exe\u{202c}\u{2069}")
        .await;
    assert!(!format.is_match(&saved), "{saved:?}");
    assert!(saved.contains("“Receipt fdp.exe”"), "{saved}");
}

// ── /memories ─────────────────────────────────────────────────────────────

/// `/memories` lists the ten most recently updated active notes — one line
/// each, at most 160 characters, no body — and how many suggestions wait in
/// the Memory view.
#[tokio::test]
async fn memories_lists_ten_bounded_lines_with_no_body() {
    let scene = Scene::new();
    let commute = scene.save(
        "Commute",
        "# Commute\n\n## Knowledge\nThe user takes the bus.\n",
        NoteKind::Fact,
    );
    for n in 0..10 {
        scene.save(
            &format!("Note {n}"),
            &format!("BODY-TEXT {n}"),
            NoteKind::Fact,
        );
    }
    // A suggestion: learning's change to a note the user saved.
    let extraction = scene
        .service
        .context(
            OWNER,
            WorkspaceHint::None,
            Surface::Extraction,
            Capabilities::EXTRACTION,
            Attribution::default(),
        )
        .unwrap();
    let suggestion = scene
        .service
        .store(
            &extraction,
            StoreInput {
                title: "Commute".into(),
                body: "# Commute\n\n## Knowledge\nThe user cycles.\n".into(),
                kind: Some(NoteKind::Fact),
                supersedes: Some((commute.id, commute.revision.clone())),
                replace_policy: ReplacePolicy::SuggestUnlessLearned,
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(suggestion.suggestion);
    // The newest update is listed first.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let newest = scene.save("Newest", "BODY-TEXT newest", NoteKind::Decision);

    let reply = scene.say("/memories").await;

    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(
        lines.len(),
        12,
        "ten notes, the rest, the suggestions: {reply}"
    );
    assert_eq!(
        lines[0],
        format!("{}  Newest  [decision/global]", id8(newest.id))
    );
    for line in &lines[..10] {
        assert!(line.ends_with("/global]"), "{line}");
    }
    assert!(!reply.contains("BODY-TEXT"), "no body is printed: {reply}");
    assert_eq!(
        lines[10],
        "More notes are kept: send /memories <words> to search them."
    );
    assert_eq!(lines[11], "1 suggestion is waiting in the Memory view.");
    assert_eq!(scene.llm.calls(), 0);
}

/// A line is at most 160 characters, whatever the title: the title gives
/// way, ending in `…`, and the id, the kind, the scope and the match reasons
/// stay whole.
#[tokio::test]
async fn a_long_title_gives_way_to_the_line_bound() {
    let scene = Scene::new();
    let long_title = "A very long title ".repeat(11);
    let long = scene.save(long_title.trim(), "BODY-TEXT long", NoteKind::Fact);
    for reply in [
        scene.say("/memories").await,
        scene.say("/memories very long title").await,
        scene.say("/forget very long title").await,
    ] {
        let line = reply.lines().next().unwrap();
        assert!(
            line.chars().count() <= 160,
            "{} chars: {line}",
            line.chars().count()
        );
        assert!(
            line.starts_with(&format!("{}  A very long title", id8(long.id))),
            "{line}"
        );
        assert!(line.contains("…  [fact/global]"), "{line}");
    }
    assert!(
        scene
            .say("/memories very long title")
            .await
            .ends_with("[fact/global]  (matched: term)"),
        "the match reasons stay whole"
    );
}

#[tokio::test]
async fn memories_with_no_notes_says_how_to_save_one() {
    let scene = Scene::new();
    assert_eq!(
        scene.say("/memories").await,
        "Nothing remembered yet. Send /remember <text> to save a note."
    );
}

/// `/memories <query>` searches (at most 8, active notes), one line per hit
/// with its match reason, and prints no body.
#[tokio::test]
async fn memories_with_a_query_lists_hits_with_their_match_reason() {
    let scene = Scene::new();
    let editor = scene.save("Editor", "BODY-TEXT: I use helix.", NoteKind::Preference);
    for n in 0..10 {
        scene.save(
            &format!("Editor habit {n}"),
            "BODY-TEXT habit",
            NoteKind::Fact,
        );
    }
    let reply = scene.say("/memories Editor").await;
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 8, "{reply}");
    assert_eq!(
        lines[0],
        format!(
            "{}  Editor  [preference/global]  (matched: title, term)",
            id8(editor.id)
        )
    );
    assert!(lines.iter().all(|l| l.contains("(matched: ")), "{reply}");
    assert!(!reply.contains("BODY-TEXT"), "{reply}");

    assert_eq!(
        scene.say("/memories quantum chromodynamics").await,
        "Nothing matched “quantum chromodynamics”."
    );
    let long = format!("/memories {}", "x".repeat(600));
    let refused = scene.say(&long).await;
    assert!(refused.contains("invalid input"), "{refused}");
    assert_eq!(scene.llm.calls(), 0);
}

// ── Learning (§12.2) ──────────────────────────────────────────────────────

/// **Evidence 7, second half.** Memory-command turns and their answers are
/// not in the next learning window: the one extraction call the flush makes
/// carries the ordinary turn and none of the commands.
#[tokio::test]
async fn memory_command_turns_are_not_in_the_next_learning_window() {
    let scene = Scene::with_llm(ScriptedLlm::answering());
    let learning = Learning::new(&scene);
    for command in [
        "/remember The user prefers vim for editing.",
        "/memories",
        "/forget vim",
        "/MEMORIES vim",
    ] {
        let reply = learning.say(command).await;
        assert!(!reply.is_error, "{command}: {}", reply.content);
    }
    assert_eq!(scene.llm.calls(), 0, "the commands called no model");
    learning.say("It is sunny in Lisbon today.").await;

    let report = learning.flush(&scene).await;

    assert_eq!(report.calls, 1, "{report:?}");
    let calls = scene.llm.extractor_calls();
    assert_eq!(calls.len(), 1);
    let prompt: String = calls[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(prompt.contains("It is sunny in Lisbon today."), "{prompt}");
    for left_out in [
        "/remember",
        "/memories",
        "/MEMORIES",
        "/forget",
        "prefers vim",
        "Saved as a note you wrote",
        "Nothing matched",
    ] {
        assert!(
            !prompt.contains(left_out),
            "{left_out} reached the extractor: {prompt}"
        );
    }
}
