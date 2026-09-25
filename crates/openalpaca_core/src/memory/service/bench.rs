//! Design 09 §6.4's service-level bench: what one owner's partition of 1 000,
//! 10 000 and 100 000 notes costs through `MemoryService`, with the parse
//! cache on and off. `#[ignore]`d; run in release:
//! `cargo test --release -p openalpaca_core --lib -- --ignored bench_service --nocapture`
//! (one size: `-- --ignored --exact memory::service::bench::<name>`).
//!
//! **Seeding** writes the note files and builds the control snapshot in one
//! pass, the way `doctor --repair` records a directory — never through
//! `create`, which reads the partition for every note and would make seeding
//! quadratic. The notes are 45 % `user`, 45 % `extracted_user` (some within
//! *recently learned*'s week), 10 % `external`, one in ten with a
//! `fact_key`, a fifth of the prose in Chinese, and one in a thousand (at
//! least five) a suggestion from the user's own words.
//!
//! **What each run prints**, once per cache mode (`on`: the service's
//! default 128 MiB; `off`: `NoteParseCache::new(0)`, which caches nothing):
//! the first whole-partition read (a cold cache), recall p50/p95 (its stat
//! scan included), `store` (a save from the app, and a learned write),
//! `update`, `forget`, `list`, `recently_learned`, `learning_view` for one
//! chunk and `pending_suggestions` (the one learning read on the reply
//! path), and the first and a quiet reconciliation tick. Once per run: the
//! seed, `open_default` with a missing index cache (the boot rebuild) and a
//! valid one, the sizes of the snapshot, `MEMORY.md` and the index cache,
//! and the machine. Design 09 §6.4's proposed targets, not gates: recall p95
//! and learned-`store` p95 at or under 100 ms at 100K notes.

use super::*;
use crate::test_util::HomeStoreGuard;
use openalpaca_storage::memory_files::{
    NOTE_PARSE_CACHE_BYTES, Note, NoteKind, NoteParseCache, NoteRecord, revision_of,
};
use openalpaca_storage::{Database, IdentityRepository};
use std::time::{Duration, Instant};

const OWNER: &str = "u_bench_owner";
const MIB: f64 = 1024.0 * 1024.0;

#[rustfmt::skip]
const WORDS: &[&str] = &[
    "user", "prefers", "vim", "helix", "editing", "coffee", "espresso", "lunch", "noodle", "bar",
    "friday", "parking", "bay", "office", "street", "meeting", "wednesday", "team", "headings",
    "sentence", "style", "guide", "greet", "surname", "bundler", "vite", "project", "build",
    "checks", "component", "test", "release", "daemon", "store", "scratch", "path", "memory",
    "library", "index", "cache", "rebuild", "tabs", "spaces", "tomatoes", "garden", "august",
    "october", "apples", "season", "gym", "monday", "birthday", "sister", "colour", "green",
    "timezone", "berlin", "tokyo", "moved", "flat", "white", "morning", "latte", "oat", "milk",
    "quick", "replies", "chat", "answer", "short", "deploy", "script", "docker", "postgres",
    "migration", "schema", "version", "budget", "context", "window", "prompt", "persona",
    "commute", "bicycle", "train", "tea", "dinner", "piano", "guitar", "novel", "hiking",
];

#[rustfmt::skip]
const CJK_WORDS: &[&str] = &[
    "用户", "喜欢", "简洁", "回答", "风格", "编译", "测试", "项目", "文件", "数据", "缓存", "索引",
    "记忆", "笔记", "标题", "别名", "标签", "版本", "历史", "咖啡", "午餐", "停车", "办公室", "会议",
    "团队", "问候", "音乐", "旅行", "早餐", "周末", "自行车", "地铁", "茶", "晚饭",
];

/// A small deterministic generator (xorshift64*), so a run is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// The note sizes a partition is seeded with (design 09 §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mix {
    /// About 1 KiB on average: 76 % at 500–1 000 B, 20 % in Chinese at
    /// 400–1 500 B, 4 % large (4–16 KiB).
    Average,
    /// 70 % at 200–400 B in a learned note's shape (design 09 §2.6); the
    /// rest as [`Mix::Average`].
    Learned,
}

fn sentence(rng: &mut Rng, cjk: bool) -> String {
    if cjk {
        let n = 4 + rng.below(8);
        let mut s: String = (0..n).map(|_| rng.pick(CJK_WORDS)).collect();
        s.push('。');
        s
    } else {
        let n = 6 + rng.below(10);
        let mut s = (0..n)
            .map(|_| rng.pick(WORDS))
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(first) = s.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        s.push('.');
        s
    }
}

/// A title and a body of about `target` bytes: a knowledge section, and
/// for a learned note its one-line evidence.
fn text(rng: &mut Rng, cjk: bool, target: usize, learned: bool) -> (String, String) {
    let title = if cjk {
        (0..2).map(|_| rng.pick(CJK_WORDS)).collect::<String>()
    } else {
        format!("{} {}", rng.pick(WORDS), rng.pick(WORDS))
    };
    let mut body = format!("# {title}\n\n## Knowledge\n");
    let tail = if learned {
        format!(
            "\n\n## Evidence and limits\nFrom what you said on 2026-09-{:02}.\n",
            1 + rng.below(28)
        )
    } else {
        "\n".to_string()
    };
    while body.len() + tail.len() < target {
        body.push_str(&sentence(rng, cjk));
        body.push(' ');
    }
    body.push_str(&tail);
    (title, body)
}

/// What seeding made, for the measurements that need real ids.
struct Seeded {
    notes: usize,
    body_bytes: usize,
    suggestions: usize,
    /// User rows in the database the learned notes cite.
    message_ids: Vec<i64>,
    elapsed: Duration,
}

/// `n` notes in the owner's home partition, written straight to disk with
/// the snapshot built in one pass (design 09 §6.4: through repair's path,
/// never `create`).
fn seed(db: &Database, n: usize, mix: Mix, rng: &mut Rng) -> Seeded {
    let started = Instant::now();
    let message_ids: Vec<i64> = (0..50)
        .map(|i| {
            db.with_connection(|c| {
                c.execute(
                    "INSERT INTO conversation_messages (lane_key, role, content, request_id) \
                     VALUES ('u_bench_owner:gui', 'user', ?1, ?2)",
                    [format!("I said thing number {i}."), format!("req-{i}")],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap()
        })
        .collect();
    let p = Partition::open(&StoreScope::Home, OWNER).unwrap();
    p.initialize().unwrap();
    let mut snapshot = p.snapshot().unwrap();
    let now = Utc::now();
    // One suggestion in a thousand notes, and at least five.
    let every = (n / 5).clamp(1, 1000);
    let mut user_notes: Vec<NoteId> = Vec::new();
    let mut body_bytes = 0;
    let mut suggestions = 0;
    for i in 0..n {
        let id = NoteId::parse(&format!(
            "{i:08x}-0000-4000-8000-{:012x}",
            rng.next() & 0xffff_ffff_ffff
        ))
        .unwrap();
        let learned = mix == Mix::Learned && rng.below(100) < 70;
        let roll = rng.below(100);
        let (cjk, target) = if learned {
            (rng.below(5) == 0, 200 + rng.below(100))
        } else if roll < 20 {
            (true, 400 + rng.below(1100))
        } else if roll < 24 {
            (false, 4096 + rng.below(12 * 1024))
        } else {
            (false, 500 + rng.below(500))
        };
        let (title, body) = text(rng, cjk, target, learned);
        let origin_roll = rng.below(100);
        let suggestion = i % every == every - 1 && !user_notes.is_empty();
        let origin = if suggestion || (45..90).contains(&origin_roll) {
            Origin::ExtractedUser
        } else if origin_roll < 45 {
            Origin::User
        } else {
            Origin::External
        };
        let age = chrono::Duration::hours(rng.below(24 * 30) as i64);
        let at = second(now - age);
        let sources = if origin == Origin::ExtractedUser {
            vec![NoteSource {
                kind: SourceType::Message,
                id: message_ids[rng.below(message_ids.len())].to_string(),
                observed_at: Some(at),
            }]
        } else {
            Vec::new()
        };
        let supersedes = if suggestion {
            suggestions += 1;
            vec![user_notes[rng.below(user_notes.len())]]
        } else {
            Vec::new()
        };
        let note = Note {
            id,
            title: format!("{title} {i}"),
            kind: if learned {
                NoteKind::Preference
            } else {
                NoteKind::Fact
            },
            status: if suggestion {
                NoteStatus::Candidate
            } else {
                NoteStatus::Active
            },
            fact_key: (rng.below(10) == 0).then(|| format!("bench.k{i}")),
            aliases: if rng.below(5) == 0 {
                vec![rng.pick(WORDS).to_string()]
            } else {
                Vec::new()
            },
            tags: Vec::new(),
            created_at: at,
            updated_at: at,
            observed_at: (origin == Origin::ExtractedUser).then_some(at),
            valid_from: None,
            valid_to: None,
            review_after: None,
            pinned: false,
            sources,
            supersedes,
            unknown: Vec::new(),
            body,
        };
        if origin == Origin::User {
            user_notes.push(id);
        }
        let bytes = note.serialize();
        body_bytes += note.body.len();
        std::fs::write(p.notes_dir().join(note.file_name()), &bytes).unwrap();
        snapshot.notes.insert(
            id,
            NoteRecord {
                revision: revision_of(bytes.as_bytes()),
                origin,
                written_at: at,
            },
        );
    }
    snapshot.save(p.snapshot_path(), now).unwrap();
    Seeded {
        notes: n,
        body_bytes,
        suggestions,
        message_ids,
        elapsed: started.elapsed(),
    }
}

/// `at` to the second, as every note timestamp is.
fn second(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(at.timestamp(), 0).expect("a representable instant")
}

fn file_mib(path: &Path) -> f64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) as f64 / MIB
}

/// Samples, reported as `n, p50, p95, max` in milliseconds.
struct Timings(Vec<Duration>);

impl Timings {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn time<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = f();
        self.0.push(started.elapsed());
        out
    }

    fn line(&self, what: &str) -> String {
        let mut sorted = self.0.clone();
        sorted.sort();
        let at = |p: f64| {
            let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
            sorted[i.min(sorted.len() - 1)].as_secs_f64() * 1e3
        };
        format!(
            "  {what:<34} n={:<3} p50 {:>9.2} ms  p95 {:>9.2} ms  max {:>9.2} ms",
            sorted.len(),
            at(0.50),
            at(0.95),
            at(1.0)
        )
    }
}

fn machine() -> String {
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    format!(
        "{host}, {}/{}, {threads} threads available",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

const QUERIES: &[&str] = &[
    "what editor do I use",
    "coffee order in the morning",
    "where do I park at the office",
    "which bundler does the project build with",
    "my sister's birthday",
    "用户喜欢简洁的回答",
    "timezone berlin tokyo",
    "release checks for the daemon",
    "gym on monday",
    "咖啡 午餐",
];

/// One run: seed `n` notes of `mix`, then measure each operation with the
/// parse cache on and off.
fn run_service_bench(name: &str, n: usize, mix: Mix) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = HomeStoreGuard::set(&home.canonicalize().unwrap());
    let db = Database::open(&root.path().join("db.sqlite")).unwrap();
    IdentityRepository::new(&db)
        .create_global_user(OWNER, None)
        .unwrap();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let seeded = seed(&db, n, mix, &mut rng);
    let iterations = match n {
        n if n >= 100_000 => 8,
        n if n >= 10_000 => 20,
        _ => 40,
    };

    println!("{name}");
    println!("  machine: {}", machine());
    println!(
        "  seeded {} notes ({mix:?} mix, {} body bytes, {} B avg, {} suggestions) in {:.2} s",
        seeded.notes,
        seeded.body_bytes,
        seeded.body_bytes / seeded.notes.max(1),
        seeded.suggestions,
        seeded.elapsed.as_secs_f64()
    );

    // The boot path (design 09 §6.4): `open_default` over a missing index
    // cache rebuilds it from every partition; over a valid one it opens.
    let index_path = MemoryIndex::default_path().unwrap();
    let started = Instant::now();
    drop(MemoryService::open_default(Some(db.clone())));
    let cold = started.elapsed();
    let started = Instant::now();
    drop(MemoryService::open_default(Some(db.clone())));
    let warm = started.elapsed();
    println!(
        "  open_default: missing index cache {:.1} ms (rebuild), valid {:.1} ms; index cache {:.1} MiB",
        cold.as_secs_f64() * 1e3,
        warm.as_secs_f64() * 1e3,
        file_mib(&index_path) + file_mib(&PathBuf::from(format!("{}-wal", index_path.display())))
    );

    let mut counter = 0usize;
    for (mode, cache) in [
        ("on", NoteParseCache::new(NOTE_PARSE_CACHE_BYTES)),
        ("off", NoteParseCache::new(0)),
    ] {
        let service = MemoryService::open_default(Some(db.clone())).with_parse_cache(cache);
        let ctx = |surface, caps| {
            service
                .context(
                    OWNER,
                    WorkspaceHint::None,
                    surface,
                    caps,
                    Attribution::default(),
                )
                .unwrap()
        };
        let user = ctx(Surface::UserSurface, Capabilities::ALL);
        let tool = ctx(Surface::ModelTool, Capabilities::ALL);
        let extraction = ctx(Surface::Extraction, Capabilities::EXTRACTION);
        println!("  parse cache {mode}:");

        let mut t = Timings::new();
        t.time(|| service.list(&user, ListInput::default()).unwrap());
        println!("{}", t.line("first list (a cold read)"));
        let mut t = Timings::new();
        t.time(|| service.reconcile_owner(OWNER).unwrap());
        println!("{}", t.line("first reconciliation tick"));
        let mut t = Timings::new();
        t.time(|| service.reconcile_owner(OWNER).unwrap());
        println!("{}", t.line("quiet reconciliation tick"));

        let mut t = Timings::new();
        let mut empty = 0;
        for query in QUERIES.iter().cycle().take(50) {
            let found = t.time(|| {
                service
                    .recall(&tool, query, RecallBudget::default())
                    .unwrap()
            });
            if found.notes.is_empty() {
                empty += 1;
            }
        }
        println!("{} ({empty} empty)", t.line("recall"));

        let mut t = Timings::new();
        for _ in 0..iterations {
            t.time(|| service.list(&user, ListInput::default()).unwrap());
        }
        println!("{}", t.line("list"));
        let mut t = Timings::new();
        for _ in 0..iterations {
            t.time(|| service.recently_learned(&user, None, None).unwrap());
        }
        println!("{}", t.line("recently_learned"));
        let mut t = Timings::new();
        for query in QUERIES.iter().cycle().take(iterations) {
            t.time(|| {
                service
                    .learning_view(&extraction, &[query.to_string()], &seeded.message_ids[..4])
                    .unwrap()
            });
        }
        println!("{}", t.line("learning_view (one chunk)"));
        let mut t = Timings::new();
        let mut listed = 0;
        for _ in 0..iterations {
            listed = t
                .time(|| {
                    service
                        .pending_suggestions(&tool, 3, &HashSet::new())
                        .unwrap()
                })
                .len();
        }
        println!("{} ({listed} listed)", t.line("pending_suggestions"));

        let mut t = Timings::new();
        let mut saved = Vec::new();
        for _ in 0..iterations {
            counter += 1;
            let out = t.time(|| {
                service
                    .store(
                        &user,
                        StoreInput {
                            title: format!("Bench save {counter}"),
                            body: format!("# Bench save {counter}\n\nThe user saved this.\n"),
                            kind: Some(NoteKind::Fact),
                            ..Default::default()
                        },
                    )
                    .unwrap()
            });
            saved.push(out);
        }
        println!("{}", t.line("store (a save from the app)"));
        let mut t = Timings::new();
        let mut learned = Vec::new();
        for i in 0..iterations {
            counter += 1;
            let out = t.time(|| {
                service
                    .store(
                        &extraction,
                        StoreInput {
                            title: format!("Bench learned {counter}"),
                            body: format!(
                                "# Bench learned {counter}\n\n## Knowledge\nThe user takes the train on day {counter}.\n\n## Evidence and limits\nFrom what you said on 2026-09-24.\n"
                            ),
                            kind: Some(NoteKind::Preference),
                            sources: vec![NoteSource {
                                kind: SourceType::Message,
                                id: seeded.message_ids[i % seeded.message_ids.len()].to_string(),
                                observed_at: None,
                            }],
                            observed_at: Some(second(Utc::now())),
                            provenance: Some(ExtractionProvenance::UserWords),
                            ..Default::default()
                        },
                    )
                    .unwrap()
            });
            learned.push(out);
        }
        println!("{}", t.line("store (learned)"));
        let mut t = Timings::new();
        for out in &saved {
            t.time(|| {
                service
                    .update(
                        &user,
                        UpdateInput {
                            id: out.id,
                            expected_revision: out.revision.clone(),
                            action: UpdateAction::Edit(Patch {
                                body: Some(format!("# Edited\n\nEdited {}.\n", out.id)),
                                ..Default::default()
                            }),
                            reason: None,
                        },
                    )
                    .unwrap()
            });
        }
        println!("{}", t.line("update (an edit)"));
        let mut t = Timings::new();
        for out in &learned {
            t.time(|| {
                service
                    .forget(
                        &user,
                        ForgetInput {
                            targets: vec![(out.id, out.revision.clone())],
                            reason: None,
                        },
                    )
                    .unwrap()
            });
        }
        println!("{}", t.line("forget (one note)"));
        println!(
            "  parse cache holds {:.1} MiB",
            service.parse_cache().cached_bytes() as f64 / MIB
        );
    }

    let partition = Partition::open(&StoreScope::Home, OWNER).unwrap();
    println!(
        "  sizes: snapshot {:.2} MiB, MEMORY.md {:.2} MiB, index cache {:.1} MiB",
        file_mib(partition.snapshot_path()),
        file_mib(&partition.root().join(files::NAVIGATION_FILE)),
        file_mib(&index_path) + file_mib(&PathBuf::from(format!("{}-wal", index_path.display())))
    );
}

#[test]
#[ignore = "bench: cargo test --release -p openalpaca_core --lib -- --ignored bench_service --nocapture"]
fn bench_service_1k_notes() {
    run_service_bench("bench_service_1k_notes", 1_000, Mix::Average);
}

#[test]
#[ignore = "bench: cargo test --release -p openalpaca_core --lib -- --ignored bench_service --nocapture"]
fn bench_service_10k_notes() {
    run_service_bench("bench_service_10k_notes", 10_000, Mix::Average);
}

#[test]
#[ignore = "bench: cargo test --release -p openalpaca_core --lib -- --ignored bench_service_100k --nocapture"]
fn bench_service_100k_notes() {
    run_service_bench("bench_service_100k_notes", 100_000, Mix::Average);
}

#[test]
#[ignore = "bench: cargo test --release -p openalpaca_core --lib -- --ignored bench_service_100k --nocapture"]
fn bench_service_100k_learned_notes() {
    run_service_bench("bench_service_100k_learned_notes", 100_000, Mix::Learned);
}

/// The seeding itself (not a bench, so it runs in CI): a small partition
/// seeded in one pass opens ready, every note verifies against the
/// snapshot, and the suggestions are the service's own.
#[test]
fn the_service_bench_seeds_a_partition_the_service_reads() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = HomeStoreGuard::set(&home.canonicalize().unwrap());
    let db = Database::open(&root.path().join("db.sqlite")).unwrap();
    IdentityRepository::new(&db)
        .create_global_user(OWNER, None)
        .unwrap();
    let seeded = seed(&db, 60, Mix::Learned, &mut Rng(7));
    assert_eq!(seeded.notes, 60);
    let loaded = Partition::open(&StoreScope::Home, OWNER)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(loaded.health, PartitionHealth::Ready);
    assert_eq!(loaded.views.len(), 60);
    assert!(loaded.malformed.is_empty());
    assert!(loaded.views.iter().all(|v| v.derived.recorded));
    let service = MemoryService::new(Some(db.clone()), MemoryIndex::open_in_memory().unwrap());
    service.rebuild_index().unwrap();
    let tool = service
        .context(
            OWNER,
            WorkspaceHint::None,
            Surface::ModelTool,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    assert!(seeded.suggestions >= 1);
    assert!(
        !service
            .pending_suggestions(&tool, 3, &HashSet::new())
            .unwrap()
            .is_empty()
    );
}
