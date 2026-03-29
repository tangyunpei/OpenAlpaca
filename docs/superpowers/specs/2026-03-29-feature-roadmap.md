# OpenAlpaca Feature Roadmap

**Date:** 2026-03-29
**Baseline comparison:** OpenClaw v2026.3.28 (TypeScript, 100+ extensions, 20+ channels)
**Our stack:** Rust, tokio, SQLite, Tauri GUI

---

## Current Strengths (OpenAlpaca leads)

| Capability | Details |
|---|---|
| Multi-agent orchestration | DAG execution, lead agent delegation, sub-agent spawning with context distillation |
| Context budget management | 5-tier graduated compaction with telemetry events |
| Security model depth | 3-layer defense (capabilities + sanitizer + sandbox), per-agent constraints |
| LLM routing sophistication | Multi-key pool with round-robin/LRU/primary-fallback, per-key rate limiting, circuit breaker |
| Cost tracking | Per-agent, per-task budget tracking with enforcement hooks |
| Type safety | Rust — memory safe, compile-time guarantees |
| Hot-reload | Config, persona, skills, agents, LLM config — all hot-reloadable |
| Event telemetry | SystemEvent bus with CompactionTriggered, ToolExecuted, SecurityViolation etc. |

---

## Gap Tiers

### Tier 1 — Critical Gaps

| # | Capability | OpenClaw | OpenAlpaca | Priority |
|---|---|---|---|---|
| 1 | **Browser automation** | CDP-based Chrome control (click, type, scroll, screenshot, JS eval) in Docker sandbox | None — `web_fetch` is read-only HTML only | **NEXT** |
| 2 | **Plugin SDK & dynamic loading** | First-class TypeScript plugin SDK, 100+ extensions, npm-installable, hot-loadable | Skills (SKILL.md files) but no runtime plugin loading | **NEXT** |
| 3 | Channel breadth | 20+ channels (WhatsApp, Signal, Slack, Teams, Matrix, LINE, Nostr...) | 3 connectors (Telegram, Discord, iMessage) | Later |
| 4 | Voice I/O | TTS (ElevenLabs + system), STT (Deepgram), wake word, continuous talk mode | Nothing | Later |
| 5 | Native mobile apps | macOS (SwiftUI menu bar), iOS (voice + canvas), Android (Kotlin) | Tauri desktop GUI only | Later |
| 6 | Onboarding / setup wizard | `openclaw onboard` — interactive guided setup | Manual TOML config editing | Later |

### Tier 2 — Significant Gaps

| # | Capability | OpenClaw | OpenAlpaca |
|---|---|---|---|
| 7 | Image generation | Provider-pluggable (DALL-E etc.) | None |
| 8 | Media pipeline | FFmpeg transcoding, audio/video format conversion | None — file attachments exist but no processing |
| 9 | Canvas / live UI | Agent-driven visual workspace (A2UI protocol) | None |
| 10 | Web search providers | Brave, DuckDuckGo built-in + pluggable | Requires external API key, echo stub fallback |
| 11 | Multi-device orchestration | macOS/iOS/Android nodes — camera, screen record, location | Single machine only |
| 12 | Cron / scheduled tasks | Built-in cron + webhooks + Gmail Pub/Sub | WakeManager exists but limited |
| 13 | Approval workflows | Commands sent to channel for approval, timeout-based | Tool confirmation broker exists but less polished |

### Tier 3 — Nice-to-Have Gaps

| # | Capability | OpenClaw | OpenAlpaca |
|---|---|---|---|
| 14 | TUI (terminal UI) | Lit-based interactive TUI | CLI chat command (simpler) |
| 15 | Docker sandbox | Code execution in isolated Docker containers | SandboxManager exists but no Docker isolation |
| 16 | Remote access | Tailscale Serve/Funnel for remote gateway | Local only |
| 17 | Config version control | `~/.openclaw/` optionally git-tracked | No config versioning |
| 18 | Architecture smell tests | Dedicated test suite for structural health | None |
| 19 | Update channels | stable/beta/dev with `openclaw update` | Manual recompile |

---

## Roadmap Phases

### Phase A: Browser Automation (Gap #1)
**Status:** Design pending
**Goal:** Give agents the ability to control a browser — navigate, click, type, screenshot, evaluate JS — so they can interact with web apps on behalf of the user.

**Key decisions to make:**
- CDP (Chrome DevTools Protocol) vs WebDriver vs Playwright integration
- In-process vs subprocess vs Docker-sandboxed browser
- Tool surface: which browser actions become agent tools
- Security: URL allowlists, credential isolation, sandbox policy

**Dependencies:** SandboxManager (exists), tool registry (exists)

### Phase B: Plugin SDK & Dynamic Loading (Gap #2)
**Status:** Design pending
**Goal:** Build a plugin system so users can extend OpenAlpaca without recompiling — add channels, tools, LLM providers, and skills as loadable plugins.

**Key decisions to make:**
- Plugin format: shared libraries (.dylib/.so) vs WASM vs subprocess (JSON-RPC/stdio)
- Discovery: directory scanning vs registry vs both
- Hot-loading: load at startup only vs true hot-reload
- SDK surface: what traits/interfaces plugins implement
- Security: sandboxing untrusted plugins, capability gating

**Dependencies:** Tool registry (exists), connector system (exists), LLM provider trait (exists)

### Phase C-Z: Future (not yet designed)
- Channel breadth (WhatsApp, Slack, Signal, Matrix...)
- Voice I/O (TTS + STT)
- Image generation
- Media pipeline
- Canvas / live agent UI
- Mobile companion apps
- Onboarding wizard
- Docker sandbox for code execution
- Remote access (Tailscale / SSH tunnel)
- Multi-device orchestration

---

## Design Sequence

Phases A and B are independent and can be designed/built in parallel. Phase B (Plugin SDK) would make Phase C+ (more channels, providers) much easier since new integrations become plugins rather than compiled crates.

**Recommended order:**
1. Design Phase A (Browser Automation) + Phase B (Plugin SDK) in parallel
2. Build Phase B first — it's foundational infrastructure
3. Build Phase A — browser tools become the first "complex plugin" and validate the SDK
4. Phase C+ channels as plugins using the new SDK
