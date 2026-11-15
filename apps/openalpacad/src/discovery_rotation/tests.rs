//! T21 (2): the rotation task driven on an injected wall clock and tokio's
//! paused time, writing the real `discovery.json` under a guarded scratch
//! home store; and the daemon's token check, through the real middleware.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    routing::get,
};
use chrono::{DateTime, TimeDelta, Utc};
use openalpaca_storage::discovery::{self, Discovery, ROTATE_AFTER, TOKEN_LIFETIME};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::*;
use crate::test_util::HomeStoreGuard;

/// A wall clock the test sets.
type Clock = Arc<Mutex<DateTime<Utc>>>;

fn clock_at(at: DateTime<Utc>) -> Clock {
    Arc::new(Mutex::new(at))
}

fn set(clock: &Clock, at: DateTime<Utc>) {
    *clock.lock().unwrap() = at;
}

/// A guarded scratch home store, and the discovery file a daemon started at
/// `started` wrote there.
fn started_daemon(started: DateTime<Utc>) -> (tempfile::TempDir, HomeStoreGuard, Discovery) {
    let home = tempfile::tempdir().expect("a scratch home store");
    let guard = HomeStoreGuard::set(&home.path().canonicalize().expect("canonical home"));
    let first = discovery::make_discovery("127.0.0.1", 51823, "instance-t21".into(), "0.1.0")
        .with_rotated_auth(started);
    discovery::write_discovery_atomic(&first).expect("the start's discovery.json");
    (home, guard, first)
}

fn on_disk() -> Discovery {
    discovery::read_discovery()
        .expect("discovery.json reads")
        .expect("discovery.json is there")
}

/// What the test's writer reports of one write.
struct Written {
    /// The file it wrote.
    file: Discovery,
    /// Whether the daemon already accepted the new token when the write
    /// began.
    accepted_before_write: bool,
}

/// The rotation task as `spawn` starts it, with the clock injected and a
/// writer that notes whether the new token is already accepted, writes the
/// real file and then reports what it wrote.
fn start(
    first: &Discovery,
    tokens: &Arc<DaemonTokens>,
    cancel: &CancellationToken,
    clock: &Clock,
) -> (
    tokio::task::JoinHandle<()>,
    mpsc::UnboundedReceiver<Written>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let clock = clock.clone();
    let accepting = tokens.clone();
    let task = tokio::spawn(run(
        first.clone(),
        tokens.clone(),
        cancel.clone(),
        CHECK_EVERY,
        move || *clock.lock().unwrap(),
        move |next: Discovery| {
            let tx = tx.clone();
            let accepted_before_write = accepting.accepts_at(&next.auth.token, next.auth.issued_at);
            async move {
                discovery::write_discovery_atomic(&next)?;
                tx.send(Written {
                    file: next,
                    accepted_before_write,
                })
                .expect("the test listens");
                Ok(())
            }
        },
    ));
    (task, rx)
}

/// The entry's first case: a daemon past one interval has rewritten
/// `discovery.json` with a new token and a later `expires_at`, everything
/// else in the file as the start wrote it; the token it replaced is still
/// honoured until its own `expires_at`; and what a client reads from a daemon
/// up for more than a day is not expired.
#[tokio::test(start_paused = true)]
async fn a_daemon_past_one_interval_has_rewritten_discovery_with_a_new_token() {
    let started = Utc::now() - TimeDelta::hours(25);
    let (_home, _guard, first) = started_daemon(started);
    let tokens = Arc::new(DaemonTokens::new(first.auth.token.clone()));
    let cancel = CancellationToken::new();
    let clock = clock_at(started + ROTATE_AFTER - TimeDelta::minutes(1));
    let (task, mut rx) = start(&first, &tokens, &cancel, &clock);

    // Not yet due: five looks, and the file is the start's.
    assert!(timeout(CHECK_EVERY * 5, rx.recv()).await.is_err());
    assert_eq!(on_disk().auth.token, first.auth.token);

    // Twelve hours after the issue: rewritten at the next look.
    set(&clock, started + ROTATE_AFTER);
    let second = timeout(CHECK_EVERY + Duration::from_secs(1), rx.recv())
        .await
        .expect("rotated within one look")
        .expect("a rotation");
    // The new token is accepted before the file names it, so a client that
    // reads the new file at once is never refused.
    assert!(
        second.accepted_before_write,
        "the new token was accepted before the write began"
    );
    let second = second.file;
    let file = on_disk();
    assert_eq!(file.auth.token, second.auth.token);
    assert_ne!(file.auth.token, first.auth.token);
    assert_eq!(file.auth.issued_at, started + ROTATE_AFTER);
    assert_eq!(
        file.auth.expires_at,
        started + ROTATE_AFTER + TOKEN_LIFETIME
    );
    assert!(file.auth.expires_at > first.auth.expires_at);
    assert_eq!(file.instance_id, first.instance_id);
    assert_eq!(file.pid, first.pid);
    assert_eq!(file.started_at, first.started_at);
    assert_eq!(file.listen.port, first.listen.port);
    assert_eq!(file.build.version, first.build.version);

    // The daemon accepts the token it last wrote, and the one before it
    // until that one's own expiry, not after.
    let now = started + ROTATE_AFTER;
    assert!(tokens.accepts_at(&second.auth.token, now));
    assert!(tokens.accepts_at(&first.auth.token, now));
    assert!(tokens.accepts_at(
        &first.auth.token,
        first.auth.expires_at - TimeDelta::seconds(1)
    ));
    assert!(!tokens.accepts_at(&first.auth.token, first.auth.expires_at));

    // A day in: the second rotation, and the start's token is gone.
    set(&clock, started + ROTATE_AFTER * 2);
    let third = timeout(CHECK_EVERY + Duration::from_secs(1), rx.recv())
        .await
        .expect("rotated within one look")
        .expect("a rotation");
    assert!(third.accepted_before_write);
    let third = third.file;
    assert_eq!(on_disk().auth.token, third.auth.token);
    let now = started + ROTATE_AFTER * 2;
    assert!(tokens.accepts_at(&third.auth.token, now));
    assert!(tokens.accepts_at(&second.auth.token, now));
    assert!(!tokens.accepts_at(&first.auth.token, now - TimeDelta::hours(1)));

    // Up for twenty-five hours, and a client's own check passes on the file.
    discovery::ensure_not_expired(&on_disk()).expect("the rotated file is not expired");
    assert!(discovery::ensure_not_expired(&first).is_err());

    cancel.cancel();
    timeout(CHECK_EVERY, task)
        .await
        .expect("the task ends on the cancel")
        .expect("the task did not panic");
}

/// A rotation whose write fails changes nothing a client holds: the file
/// still names the old token, the daemon still accepts it and not the one it
/// could not write, and the next look tries again.
#[tokio::test(start_paused = true)]
async fn a_failed_write_keeps_the_token_the_file_names_and_is_retried() {
    let started = Utc::now() - TimeDelta::hours(13);
    let (_home, _guard, first) = started_daemon(started);
    let tokens = Arc::new(DaemonTokens::new(first.auth.token.clone()));
    let cancel = CancellationToken::new();
    let clock = clock_at(started + ROTATE_AFTER);
    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel::<Result<Discovery, Discovery>>();
    let task = {
        let clock = clock.clone();
        let calls = calls.clone();
        tokio::spawn(run(
            first.clone(),
            tokens.clone(),
            cancel.clone(),
            CHECK_EVERY,
            move || *clock.lock().unwrap(),
            move |next: Discovery| {
                let tx = tx.clone();
                let call = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if call == 0 {
                        tx.send(Err(next)).expect("the test listens");
                        anyhow::bail!("the disk is full");
                    }
                    discovery::write_discovery_atomic(&next)?;
                    tx.send(Ok(next)).expect("the test listens");
                    Ok(())
                }
            },
        ))
    };

    let failed = timeout(CHECK_EVERY + Duration::from_secs(1), rx.recv())
        .await
        .expect("a first attempt")
        .expect("an attempt")
        .expect_err("the first write fails");
    let now = started + ROTATE_AFTER;
    assert_eq!(on_disk().auth.token, first.auth.token);
    assert!(tokens.accepts_at(&first.auth.token, now));
    assert!(!tokens.accepts_at(&failed.auth.token, now));

    let written = timeout(CHECK_EVERY + Duration::from_secs(1), rx.recv())
        .await
        .expect("retried at the next look")
        .expect("an attempt")
        .expect("the second write succeeds");
    assert_ne!(written.auth.token, failed.auth.token);
    assert_eq!(on_disk().auth.token, written.auth.token);
    assert!(tokens.accepts_at(&written.auth.token, now));
    assert!(tokens.accepts_at(&first.auth.token, now));
    assert!(!tokens.accepts_at(&failed.auth.token, now));

    cancel.cancel();
    timeout(CHECK_EVERY, task)
        .await
        .expect("ends")
        .expect("no panic");
}

/// The shutdown's cancel ends the task, and no write follows it: the file the
/// shutdown removes, or the watchdog before a forced exit, stays removed.
#[tokio::test(start_paused = true)]
async fn the_rotation_stops_on_the_cancel_and_writes_nothing_after_it() {
    let started = Utc::now() - TimeDelta::hours(1);
    let (_home, _guard, first) = started_daemon(started);
    let tokens = Arc::new(DaemonTokens::new(first.auth.token.clone()));
    let cancel = CancellationToken::new();
    let clock = clock_at(started);
    let (task, mut rx) = start(&first, &tokens, &cancel, &clock);

    assert!(timeout(CHECK_EVERY * 2, rx.recv()).await.is_err());
    cancel.cancel();
    timeout(CHECK_EVERY, task)
        .await
        .expect("the task ends on the cancel")
        .expect("the task did not panic");

    discovery::remove_discovery().expect("the shutdown removes the file");
    set(&clock, started + ROTATE_AFTER * 4);
    tokio::time::sleep(CHECK_EVERY * 3).await;
    assert!(
        rx.recv().await.is_none(),
        "nothing was written after the cancel"
    );
    assert!(discovery::read_discovery().expect("reads").is_none());
}

/// A cancel already given when a rotation falls due wins: no write.
#[tokio::test(start_paused = true)]
async fn a_rotation_due_after_the_cancel_is_not_written() {
    let started = Utc::now() - TimeDelta::hours(13);
    let (_home, _guard, first) = started_daemon(started);
    let tokens = Arc::new(DaemonTokens::new(first.auth.token.clone()));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let clock = clock_at(started + ROTATE_AFTER);
    let (task, mut rx) = start(&first, &tokens, &cancel, &clock);

    timeout(CHECK_EVERY, task)
        .await
        .expect("ends")
        .expect("no panic");
    assert!(rx.recv().await.is_none());
    assert_eq!(on_disk().auth.token, first.auth.token);
}

/// A cancel given between the look that finds a rotation due and its write
/// wins: the loop checks the cancel again before it writes, so nothing is
/// written and the file keeps the start's token.
#[tokio::test(start_paused = true)]
async fn a_cancel_between_the_look_and_the_write_stops_the_rotation() {
    let started = Utc::now() - TimeDelta::hours(13);
    let (_home, _guard, first) = started_daemon(started);
    let tokens = Arc::new(DaemonTokens::new(first.auth.token.clone()));
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<Discovery>();
    let task = {
        let cancel_at_look = cancel.clone();
        tokio::spawn(run(
            first.clone(),
            tokens.clone(),
            cancel.clone(),
            CHECK_EVERY,
            // The shutdown's cancel arrives while the loop reads the clock:
            // after its sleep, before its write.
            move || {
                cancel_at_look.cancel();
                started + ROTATE_AFTER
            },
            move |next: Discovery| {
                let tx = tx.clone();
                async move {
                    discovery::write_discovery_atomic(&next)?;
                    tx.send(next).expect("the test listens");
                    Ok(())
                }
            },
        ))
    };

    timeout(CHECK_EVERY * 2, task)
        .await
        .expect("the task ends on the cancel")
        .expect("the task did not panic");
    assert!(cancel.is_cancelled(), "the clock was read");
    assert!(rx.recv().await.is_none(), "nothing was written");
    assert_eq!(on_disk().auth.token, first.auth.token);
}

/// The status `auth_middleware` answers a request carrying `header`.
async fn status_for(tokens: &Arc<DaemonTokens>, header: Option<&str>) -> StatusCode {
    let app = Router::new()
        .route("/v1/status", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            tokens.clone(),
            crate::middleware::auth_middleware,
        ));
    let mut request = Request::builder().uri("/v1/status");
    if let Some(header) = header {
        request = request.header("Authorization", header);
    }
    app.oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("an answer")
        .status()
}

/// The entry's third case, through the real middleware: a request whose
/// token is neither the current one nor an honoured previous one is refused;
/// the content routes' inline check (`?token=` or the header) agrees.
#[tokio::test]
async fn a_token_neither_current_nor_an_honoured_previous_one_is_refused() {
    let now = Utc::now();
    let honoured = Arc::new(DaemonTokens::new("token-a".into()));
    honoured.rotate("token-b".into(), now + TimeDelta::hours(1));
    assert_eq!(
        status_for(&honoured, Some("Bearer token-b")).await,
        StatusCode::OK
    );
    assert_eq!(
        status_for(&honoured, Some("Bearer token-a")).await,
        StatusCode::OK
    );
    assert_eq!(
        status_for(&honoured, Some("Bearer token-c")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_for(&honoured, None).await, StatusCode::UNAUTHORIZED);

    let lapsed = Arc::new(DaemonTokens::new("token-a".into()));
    lapsed.rotate("token-b".into(), now - TimeDelta::seconds(1));
    assert_eq!(
        status_for(&lapsed, Some("Bearer token-b")).await,
        StatusCode::OK
    );
    assert_eq!(
        status_for(&lapsed, Some("Bearer token-a")).await,
        StatusCode::UNAUTHORIZED
    );

    let none = HeaderMap::new();
    assert!(crate::routes::content_token_ok(
        &none,
        Some("token-a"),
        &*honoured
    ));
    assert!(!crate::routes::content_token_ok(
        &none,
        Some("token-a"),
        &*lapsed
    ));
    assert!(!crate::routes::content_token_ok(
        &none,
        Some("token-c"),
        &*honoured
    ));
    let mut bearer = HeaderMap::new();
    bearer.insert("authorization", HeaderValue::from_static("Bearer token-b"));
    assert!(crate::routes::content_token_ok(&bearer, None, &*lapsed));
}
