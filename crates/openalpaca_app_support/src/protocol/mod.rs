//! The daemon's wire protocol as a number (DU-29 (a), Q04), and what a
//! client concludes from it: the one source `make_discovery`
//! (`discovery.json`'s `build.protocol`), `GET /v1/health` (`protocol`) and
//! the clients read, the starting state's budget ([`STARTING_BUDGET`]), and
//! the decision whether a client uses the daemon a discovery file names
//! ([`decide_reuse`]; moved here from the app shell by Q05, for the CLI's
//! same check, CLI-PROTOCOL).
//!
//! A daemon reports [`reported`]; a client accepts a daemon whose protocol
//! is from [`OLDEST_SUPPORTED`] to [`PROTOCOL`], reuses one it speaks
//! exactly, reuses an older one it still supports with a note, and refuses
//! any other with a mismatch it names — never killing, never spawning beside
//! it. When the two numbers move is the PROTOCOL rule (CLAUDE.md,
//! *Storage*; API-CONTRACT): a removed or renamed route, field or event
//! variant bumps [`PROTOCOL`] in the same commit, and
//! [`OLDEST_SUPPORTED`] with it where the older protocol can no longer be
//! served; an addition bumps neither. The daemon's contract test
//! (`apps/openalpacad/src/contract/`) refuses a response, event or request
//! body sample, or one of a response only the CLI reads, that lost a field
//! unless [`PROTOCOL`] moved (request bodies and the CLI's reads since
//! CLI-PROTOCOL); it does not see a route's path or a query string's keys,
//! which the rule binds all the same.
//!
//! Every sentence a person reads about a verdict — the note beside a daemon
//! used, the refusal of a mismatch, the log line — is the client's own: a
//! verdict here carries what was decided and the facts the sentence names
//! ([`Compat`], [`Mismatch`]), never the words.

use crate::discovery::Discovery;
use std::time::Duration;

/// The protocol this build speaks.
pub const PROTOCOL: u32 = 1;

/// The oldest protocol this build still works with.
pub const OLDEST_SUPPORTED: u32 = 1;

/// The variable a **debug** build's daemon reads to report `PROTOCOL + 1`
/// (C-20): set to `1`, a hand-started debug daemon looks newer than every
/// app built from the same commit, which is how the native mismatch check
/// gets a daemon to refuse. Compiled only under `debug_assertions`, so a
/// release build neither reads it nor carries its name.
#[cfg(debug_assertions)]
pub const PROTOCOL_PLUS_ONE_ENV: &str = "OPENALPACA_DEBUG_PROTOCOL_PLUS_ONE";

/// The protocol this daemon reports — the one read site of the debug
/// override: [`PROTOCOL`], or in a debug build with
/// [`PROTOCOL_PLUS_ONE_ENV`] set to `1`, `PROTOCOL + 1`.
pub fn reported() -> u32 {
    #[cfg(debug_assertions)]
    {
        reported_given(std::env::var_os(PROTOCOL_PLUS_ONE_ENV).as_deref())
    }
    #[cfg(not(debug_assertions))]
    {
        PROTOCOL
    }
}

/// [`reported`]'s decision over the variable's value, apart from the
/// environment, so a test drives it without setting a variable every other
/// test reads.
#[cfg(debug_assertions)]
fn reported_given(value: Option<&std::ffi::OsStr>) -> u32 {
    if value.is_some_and(|v| v == "1") {
        PROTOCOL + 1
    } else {
        PROTOCOL
    }
}

// ============================================================================
// The starting state's budget
// ============================================================================

/// How long a client waits for a daemon that is up but not serving yet
/// before it reports it still starting (Q04's starting state, C-09 with
/// A-18): the daemon writes `discovery.json` before it opens its database,
/// reconciles its MCP servers, starts its plugins and connectors and serves,
/// and a client that finds the file's pid a live `openalpacad` whose
/// `GET /v1/health` is silent waits — never spawning a second daemon,
/// rotating its log or probing its lock — for at most this long, counted
/// from its first silent look at that pid.
///
/// **An interim figure** (the owner's answer "b" of 2026-11-16 to
/// `tasks/pending-decisions.md` §1 *Rev 95*): the MCP connect's shipped
/// bound, `connect_timeout_secs = 30` in both `mcp.toml`s the daemon can be
/// given, plus a 15 s margin. It does not cover the rest of an MCP server's
/// bring-up, which boot awaits before it serves: after the connect, one
/// `tools/list` under `request_timeout_secs` (30 s in both shipped files),
/// and on a failed list a teardown of up to `request_timeout_secs` + 1 s —
/// so one server that handshakes and then stalls holds the boot about 60 s,
/// up to about 91 s, with the shipped config. Nor does it cover a slow
/// `Database::open`, many plugins or connectors (started one after another,
/// each bounded only per call), or a store whose `mcp.toml` raises either
/// timeout. A boot slower than it is reported still starting, with the log's
/// tail, while it goes on booting. The measured figure — `Discovery written
/// to:` to `Daemon ready` in one ordinary start of the real store (plan-rev3
/// Appendix B #51) — is deferred, not dropped: step 19's first real start
/// gives it, and this figure is checked against it.
///
/// The one figure the app shell and the CLI share: the shell's
/// `reuse::ensure_with` reads it, and so do the CLI's same check and its
/// wait after `openalpaca daemon start` (CLI-PROTOCOL). Here, with the
/// protocol, since Q05 (it was `daemon_lifecycle`'s).
pub const STARTING_BUDGET: Duration = Duration::from_secs(45);

// ============================================================================
// The client's own build
// ============================================================================

/// The protocols and version a client judges a daemon by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientBuild {
    /// The protocol it speaks ([`PROTOCOL`]).
    pub protocol: u32,
    /// The oldest it still works with ([`OLDEST_SUPPORTED`]).
    pub oldest_supported: u32,
    /// Its version, the client's own `CARGO_PKG_VERSION`.
    pub version: &'static str,
}

impl ClientBuild {
    /// A client of this build at `version`: the constants, never the debug
    /// override, which is a daemon's to report ([`reported`]). The version
    /// is the caller's, read in its own crate (`env!("CARGO_PKG_VERSION")`
    /// here would name this crate's package).
    pub const fn this_build(version: &'static str) -> ClientBuild {
        ClientBuild {
            protocol: PROTOCOL,
            oldest_supported: OLDEST_SUPPORTED,
            version,
        }
    }
}

// ============================================================================
// What one look sees
// ============================================================================

/// What `discovery.json` held.
#[derive(Debug, Clone)]
pub enum DiscoveryRead {
    /// No file: no daemon has written one, or the last removed it.
    Missing,
    /// A file that names nothing: not readable, not JSON, or no numeric
    /// `pid`.
    Unreadable,
    /// JSON with a numeric `pid` that is not this build's `Discovery` — a
    /// daemon of another build wrote it — with what of its build it says.
    Foreign {
        pid: u32,
        version: Option<String>,
        protocol: Option<u32>,
    },
    /// This build's `Discovery`.
    Parsed(Discovery),
}

/// `discovery.json`'s bytes as a [`DiscoveryRead`].
pub fn read_discovery_bytes(bytes: &[u8]) -> DiscoveryRead {
    if let Ok(d) = serde_json::from_slice::<Discovery>(bytes) {
        return DiscoveryRead::Parsed(d);
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return DiscoveryRead::Unreadable;
    };
    let number = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let Some(pid) = number(value.get("pid")) else {
        return DiscoveryRead::Unreadable;
    };
    DiscoveryRead::Foreign {
        pid,
        version: value
            .pointer("/build/version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        protocol: number(value.pointer("/build/protocol")),
    }
}

/// What the address answered to `GET /v1/health`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// A daemon's health: `status` `ok`, its `instance_id`, and — from Q04
    /// on — the `protocol` it speaks; `version` as it says.
    Answered {
        instance_id: String,
        protocol: Option<u32>,
        version: Option<String>,
    },
    /// No daemon's health — refused, silent past the timeout, closed early,
    /// another status, another body — and why, for the log.
    Unanswered(String),
}

/// A `200` answer's body to `GET /v1/health` as a daemon's health, or why
/// it is not one: a JSON object whose `status` is `ok` and whose
/// `instance_id` is a string; a `protocol` present must be a number a `u32`
/// holds; `version` as it says. The one reader of the body the app shell
/// (over its hand-written GET) and the CLI (over its HTTP client) share
/// (CLI-PROTOCOL), so both clients judge one answer alike; the status and
/// the transport are each client's own to read.
pub fn read_health_body(body: &[u8]) -> Health {
    let unanswered = |why: &str| Health::Unanswered(why.to_owned());
    let Ok(serde_json::Value::Object(body)) = serde_json::from_slice(body) else {
        return unanswered("a body that is not a JSON object");
    };
    if body.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return unanswered("a body whose status is not ok");
    }
    let Some(instance_id) = body.get("instance_id").and_then(serde_json::Value::as_str) else {
        return unanswered("a body with no instance_id");
    };
    let protocol = match body.get("protocol") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(protocol) => Some(protocol),
            None => return unanswered("a protocol that is not a number"),
        },
    };
    Health::Answered {
        instance_id: instance_id.to_owned(),
        protocol,
        version: body
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

/// One look at the world.
#[derive(Debug, Clone)]
pub struct Look {
    pub discovery: DiscoveryRead,
    /// What the file's address answered; [`Health::Unanswered`] when there
    /// was no address to ask.
    pub health: Health,
    /// Whether the file's pid is a live `openalpacad`
    /// (`daemon_lifecycle::pid_is_daemon`). Read only when [`needs_pid`]
    /// says the decision turns on it, `false` otherwise.
    pub pid_live: bool,
}

/// The pid whose liveness [`decide_reuse`] turns on, given what the file
/// held and health answered: a foreign file's, or a parsed file's whose
/// address did not answer as its daemon.
pub fn needs_pid(discovery: &DiscoveryRead, health: &Health) -> Option<u32> {
    match discovery {
        DiscoveryRead::Foreign { pid, .. } => Some(*pid),
        DiscoveryRead::Parsed(d) => match health {
            Health::Answered { instance_id, .. } if *instance_id == d.instance_id => None,
            _ => Some(d.pid),
        },
        DiscoveryRead::Missing | DiscoveryRead::Unreadable => None,
    }
}

// ============================================================================
// The decision
// ============================================================================

/// Why a client notes a daemon it uses: the client words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compat {
    /// An older protocol the client still supports.
    OlderProtocol,
    /// The client's protocol at another version.
    OtherVersion,
}

/// A daemon to reuse: its file, the protocol and version it answered (its
/// file's where health did not say), and why the client notes it, if it
/// does.
#[derive(Debug, Clone)]
pub struct Reused {
    pub discovery: Discovery,
    pub protocol: u32,
    pub version: String,
    pub compat: Option<Compat>,
}

/// A daemon a client cannot use, with what its sentence names.
#[derive(Debug, Clone)]
pub struct Mismatch {
    /// The daemon's pid.
    pub pid: u32,
    pub daemon_version: Option<String>,
    pub daemon_protocol: Option<u32>,
    /// The client that judged it.
    pub client: ClientBuild,
    /// Whether this build could read the daemon's file: a file it cannot is
    /// another build's whose protocol may be unknown.
    pub readable: bool,
}

/// What [`decide_reuse`] concluded.
#[derive(Debug)]
pub enum Verdict {
    /// No daemon to reuse or protect: start one.
    Spawn,
    /// Use this one.
    Reuse(Reused),
    /// A live daemon this client cannot use: say so, start nothing, stop
    /// nothing.
    Mismatch(Mismatch),
    /// The file's pid is a live `openalpacad` that does not answer as the
    /// file's daemon: wait for it, start nothing.
    Starting { pid: u32, instance_id: String },
}

/// The decision over one look (DU-29 (a)'s five steps, C-09's sixth case,
/// A-20's seventh row):
///
/// 1. no file, or one that names nothing → spawn;
/// 2. a file this build cannot read, over a live daemon → mismatch, over no
///    live daemon → spawn;
/// 3. the address answers as the file's daemon: its protocol — health's,
///    else the file's `build.protocol` (a daemon from before Q04 answers
///    none) — equal to the client's → reuse, with a note when its version is
///    another (seventh row); older but still supported → reuse with a note;
///    newer, or older than the oldest supported → mismatch;
/// 4. it does not (silent, a 404, another instance) and the pid is a live
///    `openalpacad` → starting (sixth case);
/// 5. it does not, and the pid is not → spawn.
///
/// The token's age is not judged: a daemon whose health answers is used
/// whether its file was rotated or has lapsed (T21; liveness the authority).
pub fn decide_reuse(look: &Look, client: &ClientBuild) -> Verdict {
    match &look.discovery {
        DiscoveryRead::Missing | DiscoveryRead::Unreadable => Verdict::Spawn,
        DiscoveryRead::Foreign {
            pid,
            version,
            protocol,
        } => {
            if look.pid_live {
                Verdict::Mismatch(Mismatch {
                    pid: *pid,
                    daemon_version: version.clone(),
                    daemon_protocol: *protocol,
                    client: *client,
                    readable: false,
                })
            } else {
                Verdict::Spawn
            }
        }
        DiscoveryRead::Parsed(d) => {
            if let Health::Answered {
                instance_id,
                protocol,
                version,
            } = &look.health
                && *instance_id == d.instance_id
            {
                let protocol = protocol.unwrap_or(d.build.protocol);
                let version = version.clone().unwrap_or_else(|| d.build.version.clone());
                if protocol > client.protocol || protocol < client.oldest_supported {
                    return Verdict::Mismatch(Mismatch {
                        pid: d.pid,
                        daemon_version: Some(version),
                        daemon_protocol: Some(protocol),
                        client: *client,
                        readable: true,
                    });
                }
                let compat = compat(client, protocol, &version);
                return Verdict::Reuse(Reused {
                    discovery: d.clone(),
                    protocol,
                    version,
                    compat,
                });
            }
            if look.pid_live {
                Verdict::Starting {
                    pid: d.pid,
                    instance_id: d.instance_id.clone(),
                }
            } else {
                Verdict::Spawn
            }
        }
    }
}

/// Why `client` notes a daemon of `protocol` at `version` it uses, `None`
/// for one of its own protocol and version — or one it does not use at all.
pub fn compat(client: &ClientBuild, protocol: u32, version: &str) -> Option<Compat> {
    if protocol > client.protocol || protocol < client.oldest_supported {
        return None;
    }
    if protocol < client.protocol {
        return Some(Compat::OlderProtocol);
    }
    (version != client.version).then_some(Compat::OtherVersion)
}

/// The mismatch a discovery file states by itself: `d`'s `build.protocol`
/// outside what `client` works with, `None` when it is inside. What the app
/// shell's `get_connection_info` refuses (the reconnect ladder's rung, which
/// asks no health): the caller decides whether `d`'s pid is a live daemon
/// first, since a mismatch is never said of a process that is gone.
pub fn file_mismatch(d: &Discovery, client: &ClientBuild) -> Option<Mismatch> {
    let protocol = d.build.protocol;
    (protocol > client.protocol || protocol < client.oldest_supported).then(|| Mismatch {
        pid: d.pid,
        daemon_version: Some(d.build.version.clone()),
        daemon_protocol: Some(protocol),
        client: *client,
        readable: true,
    })
}

#[cfg(test)]
mod tests;
