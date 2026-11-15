//! T21 (2): the daemon rotates `discovery.json`'s token and `expires_at` on a
//! schedule while it runs, and accepts the token it last wrote.
//!
//! Before this the file was written once, at the start, with a token that
//! expired a day later; the daemon compared only the token, while the
//! clients — the CLI's commands and the shell's `get_connection_info` —
//! refuse an expired file, so a daemon up for more than a day served the app
//! and refused every CLI command until it was restarted.
//!
//! The schedule ([`run`]): every [`CHECK_EVERY`] the task reads the wall
//! clock and, when the file's token is [`ROTATE_AFTER`] old (or the clock was
//! set back before its issue), issues a new one ([`Discovery::rotation_due`],
//! [`Discovery::with_rotated_auth`]). The look is short and on the wall clock
//! because `expires_at` is a wall-clock instant and tokio's timers are not:
//! they do not count the time the Mac sleeps, so a single twelve-hour timer
//! could leave the file expired for hours after a wake.
//!
//! The carry-over ([`DaemonTokens`]): the daemon accepts the token it last
//! wrote, and the one it replaced until that one's own `expires_at` — so a
//! client that read the file just before a rotation is not refused, and a
//! client that holds a token longer than its life (the app's window) reads
//! the file again on the refusal. Each rotation drops the token before the
//! one it replaces; on the schedule that one has expired by then, but after
//! the wall clock is set back a rotation comes early and can drop a token
//! before its `expires_at`, and a client refused then reads the file again. The new token is accepted before the file
//! names it; a write that fails puts the set back, so the file and the set
//! never disagree, and the next look tries again.
//!
//! ZCode's server keeps a map of capabilities to their expiry with an
//! injected `now` (ZCode:packages/server/src/hostCapability.ts:18-52); this
//! set is written fresh for one current token and one previous.

use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use openalpaca_storage::discovery::{self, Discovery};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// How often the task reads the wall clock: the longest a file stays expired
/// after a wake from a sleep past its expiry.
pub(crate) const CHECK_EVERY: Duration = Duration::from_secs(60);

/// A token check: the daemon's set, or (in a route's tests) one fixed token.
pub(crate) trait AcceptsToken {
    /// Whether a request carrying `token` is let in.
    fn accepts_token(&self, token: &str) -> bool;
}

impl AcceptsToken for str {
    fn accepts_token(&self, token: &str) -> bool {
        self == token
    }
}

/// The tokens the daemon accepts: the one `discovery.json` names now, and the
/// one it replaced until that one's own `expires_at` (or until the next
/// rotation, which comes before it only after the wall clock was set back).
#[derive(Debug)]
pub struct DaemonTokens {
    tokens: RwLock<Tokens>,
}

#[derive(Debug, Clone)]
pub(crate) struct Tokens {
    current: String,
    /// The token replaced last and the `expires_at` its file gave it.
    previous: Option<(String, DateTime<Utc>)>,
}

impl DaemonTokens {
    /// The set at the start: the token the boot wrote, no previous one.
    pub(crate) fn new(current: String) -> Self {
        Self {
            tokens: RwLock::new(Tokens {
                current,
                previous: None,
            }),
        }
    }

    /// Whether `token` is let in at `now`.
    pub(crate) fn accepts_at(&self, token: &str, now: DateTime<Utc>) -> bool {
        let tokens = self.tokens.read().unwrap_or_else(|p| p.into_inner());
        token == tokens.current
            || tokens
                .previous
                .as_ref()
                .is_some_and(|(previous, until)| token == previous && now < *until)
    }

    /// `next` becomes the current token; the current one is honoured until
    /// `previous_until`, and the one before it is dropped. Returns the set as
    /// it was, for [`Self::restore`].
    pub(crate) fn rotate(&self, next: String, previous_until: DateTime<Utc>) -> Tokens {
        let mut tokens = self.tokens.write().unwrap_or_else(|p| p.into_inner());
        let before = tokens.clone();
        let replaced = std::mem::replace(&mut tokens.current, next);
        tokens.previous = Some((replaced, previous_until));
        before
    }

    /// Puts the set back as [`Self::rotate`] found it.
    pub(crate) fn restore(&self, before: Tokens) {
        *self.tokens.write().unwrap_or_else(|p| p.into_inner()) = before;
    }
}

impl AcceptsToken for DaemonTokens {
    fn accepts_token(&self, token: &str) -> bool {
        self.accepts_at(token, Utc::now())
    }
}

/// Starts the rotation for the file the boot wrote, on the wall clock, until
/// `cancel`. `main` awaits the handle before it removes the file at shutdown,
/// so no rotation writes it again after.
pub(crate) fn spawn(
    written: Discovery,
    tokens: Arc<DaemonTokens>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(
        written,
        tokens,
        cancel,
        CHECK_EVERY,
        Utc::now,
        |next| async move {
            tokio::task::spawn_blocking(move || discovery::write_discovery_atomic(&next).map(drop))
                .await
                .context("the write of discovery.json could not be run")?
        },
    ))
}

/// The rotation loop, with its clock and its writer given, so a test drives it
/// on an injected clock and tokio's paused time. Looks every `check_every`,
/// rotates when [`Discovery::rotation_due`] says so, and returns when `cancel`
/// is cancelled — before a write it has not begun.
pub(crate) async fn run<C, W, F>(
    mut written: Discovery,
    tokens: Arc<DaemonTokens>,
    cancel: CancellationToken,
    check_every: Duration,
    now: C,
    write: W,
) where
    C: Fn() -> DateTime<Utc>,
    W: Fn(Discovery) -> F,
    F: Future<Output = anyhow::Result<()>>,
{
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(check_every) => {}
        }
        let at = now();
        if !written.rotation_due(at) {
            continue;
        }
        if cancel.is_cancelled() {
            return;
        }
        let next = written.with_rotated_auth(at);
        // Accepted before the file names it, so a client that reads the new
        // file is never refused.
        let before = tokens.rotate(next.auth.token.clone(), written.auth.expires_at);
        match write(next.clone()).await {
            Ok(()) => {
                info!(
                    "Discovery token rotated: the new one expires at {}; the one it replaced \
                     is honoured until {}",
                    next.auth.expires_at.to_rfc3339(),
                    written.auth.expires_at.to_rfc3339()
                );
                written = next;
            }
            Err(error) => {
                tokens.restore(before);
                warn!(
                    "Could not write discovery.json with a new token ({error:#}); the token it \
                     names stays, and the rotation is tried again in {} s",
                    check_every.as_secs()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
