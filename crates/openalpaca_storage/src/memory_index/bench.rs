//! The slice-2 bench (plan: "10K notes, warm p95 at or under 50 ms including
//! file verification") and design 09 §6.4's 100K runs, one body
//! ([`run_bench`]) for all of them. `#[ignore]`d; run with
//! `cargo test --release -p openalpaca_storage -- --ignored bench_ --nocapture`
//! (one of them: `-- --ignored --exact memory_index::bench::<name>`). The
//! process's peak memory is measured from outside, so the compile is not
//! counted: `/usr/bin/time -l target/release/deps/openalpaca_storage-<hash>
//! --ignored bench_search_100k --nocapture`.
//!
//! The average mix ([`Mix::Average`]): synthetic notes averaging about 1 KiB — English prose, a few
//! percent of large notes up to 16 KiB, a fifth in Chinese — are written as
//! files under a temp directory, indexed, then searched with 200 queries of
//! the shapes the fixtures use (natural questions, bare terms, identifiers,
//! CJK of one, two and four characters), limit 8. The English vocabulary is
//! a head of common words that every note draws on (so common-term queries
//! score thousands of chunks, the expensive case) and a Zipf-skewed tail of
//! generated words; queries draw from both. Every hit is **verified**:
//! its file is re-read and re-hashed and the hash must equal the indexed
//! revision (design §5 step 7 is the caller's; the bench pays for it here so
//! the number means what the plan asks). Prints build time, cache size, and
//! p50/p95/max of the search-plus-verification latency; asserts only that
//! every query found something and every hit verified. The learned mix
//! ([`Mix::Learned`], design 09 §6.4) draws 70 % of its notes at 200–400 B —
//! the shape of a learned note's body (§2.6) — and the rest as the average
//! mix does. Every run also prints the cache's `dbstat` breakdown by table
//! and index, the empty-query rate and the machine.

use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const QUERIES: usize = 200;
const WARMUP: usize = 20;

#[rustfmt::skip]
const WORDS: &[&str] = &[
    "user", "prefers", "vim", "editing", "coffee", "espresso", "lunch", "noodle", "bar",
    "friday", "parking", "bay", "office", "elm", "street", "meeting", "wednesday", "thursday",
    "team", "headings", "sentence", "case", "style", "guide", "greet", "surname", "first",
    "name", "bundler", "webpack", "vite", "project", "build", "checks", "component", "test",
    "release", "daemon", "store", "scratch", "absolute", "path", "memory", "library", "index",
    "cache", "rebuild", "tabs", "spaces", "indentation", "alignment", "tomatoes", "garden",
    "august", "october", "apples", "season", "gym", "monday", "birthday", "sister", "colour",
    "green", "timezone", "berlin", "tokyo", "moved", "flat", "white", "morning", "latte", "oat",
    "milk", "quick", "replies", "chat", "answer", "short", "deploy", "blue", "script", "docker",
    "compose", "postgres", "migration", "schema", "version", "baseline", "refused", "legacy",
    "database", "opener", "tokens", "budget", "context", "window", "prompt", "persona",
    "profile", "identity", "bootstrap", "extraction", "candidate", "confirm", "archive",
    "supersede", "forget", "tombstone", "snapshot", "partition", "owner", "global", "workspace",
    "marker", "symlink", "junction", "confine", "atomic", "rename", "fsync", "history",
    "revision", "hash", "bytes", "chunk", "excerpt", "heading", "line", "range", "reason",
    "trigram", "phrase", "alias", "title", "kind", "status", "validity", "authority", "recency",
    "pinned", "limit", "partial", "ready", "rebuilding", "scan", "mtime", "inode", "external",
    "imported", "extracted", "candidate", "conflict", "loser", "winner", "recall", "search",
    "read", "list", "doctor", "repair", "adopt", "sync", "event", "changed", "surface", "tool",
    "model", "loop", "lead", "subagent", "worker", "skill", "plugin", "extension", "capability",
    "sandbox", "policy", "allow", "deny", "gate", "withheld", "silent", "degrade", "cost",
    "cap", "turn", "run", "task", "lane", "followup", "steer", "queue", "cancel", "pause",
    "resume", "attach", "upload", "document", "image", "audio", "transcript", "vision",
    "native", "fenced", "block", "markdown", "table", "quote", "emphasis", "code", "span",
    "list", "ordered", "marker", "resume", "stream", "delta", "reasoning", "done", "frame",
    "socket", "close", "shutdown", "restart", "lock", "pid", "port", "token", "discovery",
    "bundle", "sidecar", "log", "tail", "rotate", "append",
];

#[rustfmt::skip]
const CJK_WORDS: &[&str] = &[
    "用户", "喜欢", "简洁", "回答", "风格", "编译", "测试", "项目", "文件", "数据", "缓存", "索引", "重建", "记忆", "笔记",
    "标题", "别名", "标签", "状态", "版本", "修订", "历史", "快照", "分区", "所有者", "全局", "工作区", "标记", "链接", "原子",
    "重命名", "同步", "外部", "导入", "提取", "候选", "确认", "归档", "替换", "遗忘", "墓碑", "冲突", "胜者", "召回", "搜索",
    "阅读", "列表", "医生", "修复", "采用", "事件", "变更", "工具", "模型", "循环", "领导", "子代理", "技能", "插件", "扩展",
    "能力", "沙盒", "策略", "允许", "拒绝", "成本", "上限", "轮次", "任务", "队列", "取消", "暂停", "恢复", "附件", "上传",
    "文档", "图片", "音频", "转录", "视觉", "代码", "表格", "引用", "强调", "流式", "推理", "完成", "帧", "套接字", "关闭",
    "关机", "重启", "锁", "端口", "令牌", "发现", "捆绑", "日志", "苹果", "香蕉", "咖啡", "午餐", "停车", "办公室", "会议",
    "星期三", "团队", "问候",
];

#[rustfmt::skip]
const IDENTIFIERS: &[&str] = &[
    "OPENALPACA_HOME_STORE", "openalpaca_storage::memory_files", "memory-index-v1.sqlite",
    "state/cache", "p50/p95", "cargo_test_release", "BASELINE_VERSION", "daemon.toml",
    "llm.toml", "x-workspace-path",
];

/// How many generated tail words the English vocabulary carries beyond the
/// common head.
const TAIL_WORDS: usize = 3000;

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

    /// A unit float in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The English vocabulary: the common head plus a generated tail whose
/// words are drawn with a Zipf-like skew (rank ∝ u²).
struct Vocabulary {
    tail: Vec<String>,
}

impl Vocabulary {
    fn new(rng: &mut Rng) -> Self {
        const SYLLABLES: &[&str] = &[
            "ba", "ce", "di", "fo", "gu", "ha", "je", "ki", "lo", "mu", "na", "pe", "ri", "so",
            "tu", "va", "we", "xi", "yo", "zu", "ka", "ne", "ti", "ro", "sa", "mi", "lu", "de",
        ];
        let mut tail = Vec::with_capacity(TAIL_WORDS);
        while tail.len() < TAIL_WORDS {
            let n = 2 + rng.below(3);
            let word: String = (0..n).map(|_| rng.pick(SYLLABLES)).collect();
            if !tail.contains(&word) {
                tail.push(word);
            }
        }
        Self { tail }
    }

    /// Half the draws come from the head, half from the tail's skewed ranks.
    fn word<'a>(&'a self, rng: &mut Rng) -> &'a str {
        if rng.below(2) == 0 {
            rng.pick(WORDS)
        } else {
            let u = rng.unit();
            let rank = ((u * u) * self.tail.len() as f64) as usize;
            &self.tail[rank.min(self.tail.len() - 1)]
        }
    }
}

fn english_sentence(rng: &mut Rng, vocab: &Vocabulary) -> String {
    let n = 6 + rng.below(10);
    let mut words: Vec<&str> = (0..n).map(|_| vocab.word(rng)).collect();
    if rng.below(4) == 0 {
        words.push(rng.pick(IDENTIFIERS));
    }
    let mut s = words.join(" ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s.push('.');
    s
}

fn english_body(rng: &mut Rng, vocab: &Vocabulary, title: &str, target: usize) -> String {
    let mut body = format!("# {title}\n\n## Knowledge\n");
    let mut paragraph = 0;
    while body.len() < target {
        let sentences = 2 + rng.below(4);
        let para: Vec<String> = (0..sentences)
            .map(|_| english_sentence(rng, vocab))
            .collect();
        body.push_str(&para.join(" "));
        body.push_str("\n\n");
        paragraph += 1;
        if paragraph % 3 == 0 {
            body.push_str(&format!("## {} {}\n", rng.pick(WORDS), rng.pick(WORDS)));
        }
    }
    body
}

fn cjk_body(rng: &mut Rng, title: &str, target: usize) -> String {
    let mut body = format!("# {title}\n\n");
    while body.len() < target {
        let n = 4 + rng.below(8);
        let sentence: String = (0..n)
            .map(|_| rng.pick(CJK_WORDS))
            .collect::<Vec<_>>()
            .join("");
        body.push_str(&sentence);
        body.push_str("。\n");
        if rng.below(5) == 0 {
            body.push('\n');
        }
    }
    body
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The note sizes a corpus is drawn from (design 09 §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mix {
    /// The slice-2 corpus: about 1 KiB on average, a fifth in Chinese, a
    /// few percent large.
    Average,
    /// 70 % of the notes at 200–400 B in a learned note's shape — a
    /// `## Knowledge` section and a one-line `## Evidence and limits` — a
    /// fifth of those in Chinese; the other 30 % as [`Mix::Average`].
    Learned,
}

/// A learned note's body (design 09 §2.6): the knowledge in a sentence or
/// two, then where it came from, about `target` bytes.
fn learned_body(
    rng: &mut Rng,
    vocab: &Vocabulary,
    title: &str,
    target: usize,
    cjk: bool,
) -> String {
    let head = format!("# {title}\n\n## Knowledge\n");
    let evidence = format!(
        "\n\n## Evidence and limits\nFrom what you said on 2026-09-{:02}.\n",
        1 + rng.below(28)
    );
    let mut knowledge = String::new();
    while head.len() + knowledge.len() + evidence.len() < target {
        if cjk {
            let n = 4 + rng.below(8);
            let sentence: String = (0..n).map(|_| rng.pick(CJK_WORDS)).collect();
            knowledge.push_str(&sentence);
            knowledge.push('。');
        } else {
            if !knowledge.is_empty() {
                knowledge.push(' ');
            }
            knowledge.push_str(&english_sentence(rng, vocab));
        }
    }
    format!("{head}{knowledge}{evidence}")
}

/// Builds a corpus of `n` notes of `mix` on disk and the records that
/// describe it. The average mix of 10 000 notes is the slice-2 corpus, draw
/// for draw.
fn build_corpus(
    root: &Path,
    rng: &mut Rng,
    vocab: &Vocabulary,
    n: usize,
    mix: Mix,
) -> Vec<IndexRecord> {
    std::fs::create_dir_all(root.join("notes")).unwrap();
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let id = format!(
            "{:08x}-0000-4000-8000-{:012x}",
            i,
            rng.next() & 0xffff_ffff_ffff
        );
        let learned = mix == Mix::Learned && rng.below(100) < 70;
        let roll = if learned { 0 } else { rng.below(100) };
        let (title, body, kind) = if learned {
            let cjk = rng.below(5) == 0;
            let title = if cjk {
                (0..2).map(|_| rng.pick(CJK_WORDS)).collect()
            } else {
                format!("{} {}", vocab.word(rng), vocab.word(rng))
            };
            let target = 200 + rng.below(100);
            let body = learned_body(rng, vocab, &title, target, cjk);
            (title, body, "preference")
        } else if roll < 20 {
            let title: String = (0..2).map(|_| rng.pick(CJK_WORDS)).collect();
            let target = 400 + rng.below(1100);
            (title.clone(), cjk_body(rng, &title, target), "preference")
        } else if roll < 24 {
            let title = format!(
                "{} {} {}",
                vocab.word(rng),
                vocab.word(rng),
                vocab.word(rng)
            );
            let target = 4096 + rng.below(12 * 1024);
            (
                title.clone(),
                english_body(rng, vocab, &title, target),
                "reference",
            )
        } else {
            let title = format!("{} {}", vocab.word(rng), vocab.word(rng));
            let target = 500 + rng.below(500);
            (
                title.clone(),
                english_body(rng, vocab, &title, target),
                "fact",
            )
        };
        let header = format!(
            "---\nschema: 1\nid: {id}\ntitle: {title}\nkind: {kind}\nstatus: active\n---\n"
        );
        let file = format!("{header}{body}");
        let relative_path = format!("notes/{id}--{}.md", i);
        std::fs::write(root.join(&relative_path), &file).unwrap();
        let meta = std::fs::metadata(root.join(&relative_path)).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let status_roll = rng.below(20);
        let (status, effective) = match status_roll {
            0 => ("active", EffectiveStatus::Superseded),
            1 => ("archived", EffectiveStatus::Archived),
            2 => ("candidate", EffectiveStatus::Candidate),
            _ => ("active", EffectiveStatus::Active),
        };
        let mut aliases = Vec::new();
        if rng.below(5) == 0 {
            aliases.push(rng.pick(WORDS).to_string());
        }
        if rng.below(10) == 0 {
            aliases.push(rng.pick(CJK_WORDS).to_string());
        }
        records.push(IndexRecord {
            id: id.clone(),
            revision: hex(Sha256::digest(file.as_bytes())),
            partition_key: if rng.below(4) == 0 {
                "proj-bench-0000000000000000".to_string()
            } else {
                "home-bench".to_string()
            },
            scope: "global".to_string(),
            relative_path,
            title,
            aliases,
            tags: if rng.below(3) == 0 {
                vec![rng.pick(WORDS).to_string()]
            } else {
                Vec::new()
            },
            kind: kind.to_string(),
            status: status.to_string(),
            effective_status: effective,
            origin: rng
                .pick(&["user", "extracted_user", "extracted_other", "external"])
                .to_string(),
            fact_key: (rng.below(10) == 0).then(|| format!("bench.{}", rng.pick(WORDS))),
            valid_from: None,
            valid_to: None,
            updated_at: format!(
                "2026-{:02}-{:02}T10:00:00Z",
                1 + rng.below(9),
                1 + rng.below(28)
            ),
            // Set from `updated_at` below, so the generator draws exactly
            // what it drew before notes carried dates.
            created_at: String::new(),
            observed_at: None,
            pinned: rng.below(200) == 0,
            source_forgotten: rng.below(100) == 0,
            conflict: false,
            same_key_loser: rng.below(100) == 0,
            superseded_by: Vec::new(),
            supersedes: Vec::new(),
            sources: Vec::new(),
            body,
            body_line_offset: 7,
            file: FileStat {
                mtime_ns: mtime.as_nanos() as i64,
                size: meta.len(),
                inode: 0,
            },
        });
        if let Some(last) = records.last_mut() {
            last.created_at = last.updated_at.clone();
        }
    }
    records
}

fn queries(rng: &mut Rng, vocab: &Vocabulary) -> Vec<String> {
    (0..QUERIES)
        .map(|i| match i % 8 {
            0 => format!("what {} do I {}?", vocab.word(rng), vocab.word(rng)),
            1 => format!("{} {}", vocab.word(rng), vocab.word(rng)),
            2 => vocab.word(rng).to_string(),
            3 => rng.pick(IDENTIFIERS).to_string(),
            4 => rng.pick(CJK_WORDS).to_string(),
            5 => format!("{}{}", rng.pick(CJK_WORDS), rng.pick(CJK_WORDS)),
            6 => rng.pick(CJK_WORDS).chars().next().unwrap().to_string(),
            _ => format!(
                "please remind me which {} the {} needs and whether {} matters",
                vocab.word(rng),
                vocab.word(rng),
                vocab.word(rng)
            ),
        })
        .collect()
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// What the machine is, for a bench report (design 09 §6.4).
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

/// The cache's pages by table and index (`dbstat`, compiled into the
/// bundled SQLite), largest first, in MiB.
fn dbstat(index: &MemoryIndex) -> Vec<(String, f64)> {
    let conn = index.lock();
    let rows = conn
        .prepare("SELECT name, SUM(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as f64 / MIB))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        });
    match rows {
        Ok(rows) => rows,
        Err(e) => vec![(format!("dbstat unavailable: {e}"), 0.0)],
    }
}

const MIB: f64 = 1024.0 * 1024.0;

/// One bench run: `n` notes of `mix` written, indexed and searched with
/// 200 queries, every hit verified against its file (design 09 §6.4).
fn run_bench(name: &str, n: usize, mix: Mix) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let vocab = Vocabulary::new(&mut rng);
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("partition");
    let records = build_corpus(&root, &mut rng, &vocab, n, mix);
    let total_bytes: usize = records.iter().map(|r| r.body.len()).sum();
    let small = records.iter().filter(|r| r.body.len() <= 400).count();

    let cache = dir.path().join("state").join("cache").join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&cache).unwrap();
    let t0 = Instant::now();
    let report = index.rebuild(&records).unwrap();
    let build = t0.elapsed();
    let cache_bytes = std::fs::metadata(&cache).map(|m| m.len()).unwrap_or(0)
        + std::fs::metadata(format!("{}-wal", cache.display()))
            .map(|m| m.len())
            .unwrap_or(0);
    let pages = dbstat(&index);

    let partitions = vec![
        "home-bench".to_string(),
        "proj-bench-0000000000000000".to_string(),
    ];
    let qs = queries(&mut rng, &vocab);
    for query in qs.iter().take(WARMUP) {
        let _ = index.search(&SearchQuery::new(query.clone(), partitions.clone()));
    }

    let mut latencies = Vec::with_capacity(QUERIES);
    let mut search_only = Vec::with_capacity(QUERIES);
    let mut hits_total = 0usize;
    let mut verified = 0usize;
    let mut empty = 0usize;
    let mut partial = 0usize;
    for query in &qs {
        let t = Instant::now();
        let results = index
            .search(&SearchQuery::new(query.clone(), partitions.clone()))
            .unwrap();
        let searched = t.elapsed();
        for hit in &results.hits {
            let bytes = std::fs::read(root.join(&hit.relative_path)).unwrap();
            let revision = hex(Sha256::digest(&bytes));
            assert_eq!(
                revision, hit.revision,
                "{query:?}: {} did not verify",
                hit.id
            );
            verified += 1;
        }
        latencies.push(t.elapsed());
        search_only.push(searched);
        hits_total += results.count;
        if results.count == 0 {
            empty += 1;
        }
        if results.partial {
            partial += 1;
        }
    }
    latencies.sort();
    search_only.sort();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let max = *latencies.last().unwrap();
    let s50 = percentile(&search_only, 0.50);
    let s95 = percentile(&search_only, 0.95);

    println!("{name}");
    println!("  machine: {}", machine());
    println!(
        "  corpus: {} notes ({mix:?} mix, {} at 400 B or less), {} body bytes ({} B avg), {} chunks",
        report.notes,
        small,
        total_bytes,
        total_bytes / report.notes.max(1),
        report.chunks
    );
    println!(
        "  build: {:.3} s; cache file {:.1} MiB ({:.1}x the body bytes)",
        build.as_secs_f64(),
        cache_bytes as f64 / MIB,
        cache_bytes as f64 / total_bytes.max(1) as f64
    );
    println!("  dbstat (MiB by table and index):");
    for (name, mib) in &pages {
        println!("    {mib:>9.2}  {name}");
    }
    println!(
        "  {} queries, limit 8: {} hits ({:.2} avg), {} empty ({:.1} %), {} partial, {} verified",
        QUERIES,
        hits_total,
        hits_total as f64 / QUERIES as f64,
        empty,
        empty as f64 * 100.0 / QUERIES as f64,
        partial,
        verified
    );
    println!(
        "  search+verify: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        p50.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
    println!(
        "  search only:   p50 {:.2} ms, p95 {:.2} ms",
        s50.as_secs_f64() * 1e3,
        s95.as_secs_f64() * 1e3
    );
    assert_eq!(verified, hits_total, "every hit must verify");
    assert!(
        empty < QUERIES / 4,
        "{empty} of {QUERIES} queries found nothing; the corpus or the queries are off"
    );
}

#[test]
#[ignore = "bench: cargo test --release -p openalpaca_storage -- --ignored bench_ --nocapture"]
fn bench_search_10k_notes_with_verification() {
    run_bench(
        "bench_search_10k_notes_with_verification",
        10_000,
        Mix::Average,
    );
}

/// Design 09 §6.4: the average mix at 100 000 notes.
#[test]
#[ignore = "bench: cargo test --release -p openalpaca_storage -- --ignored bench_search_100k --nocapture"]
fn bench_search_100k_notes_with_verification() {
    run_bench(
        "bench_search_100k_notes_with_verification",
        100_000,
        Mix::Average,
    );
}

/// Design 09 §6.4: the learned mix at 100 000 notes.
#[test]
#[ignore = "bench: cargo test --release -p openalpaca_storage -- --ignored bench_search_100k --nocapture"]
fn bench_search_100k_learned_notes_with_verification() {
    run_bench(
        "bench_search_100k_learned_notes_with_verification",
        100_000,
        Mix::Learned,
    );
}

/// The corpus generator itself (not a bench, so it runs in CI): the average
/// mix draws exactly what the slice-2 generator drew, so the 10K numbers
/// stay comparable; the learned mix keeps 70 % of its notes small.
#[test]
fn the_bench_corpus_mixes_have_their_shapes() {
    let dir = TempDir::new().unwrap();
    let sizes = |mix: Mix| {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let vocab = Vocabulary::new(&mut rng);
        let root = dir.path().join(format!("{mix:?}"));
        let records = build_corpus(&root, &mut rng, &vocab, 400, mix);
        assert_eq!(records.len(), 400);
        for record in &records {
            let file = std::fs::read(root.join(&record.relative_path)).unwrap();
            assert_eq!(hex(Sha256::digest(&file)), record.revision);
        }
        let small = records.iter().filter(|r| r.body.len() <= 400).count();
        (small, records[0].id.clone(), records[0].body.len())
    };
    let (average_small, first_id, first_len) = sizes(Mix::Average);
    assert!(
        average_small < 40,
        "{average_small} small notes in the average mix"
    );
    // The slice-2 generator's first note, draw for draw.
    assert_eq!(first_id, "00000000-0000-4000-8000-1f8d40d96f36");
    assert_eq!(first_len, 1164);
    let (learned_small, ..) = sizes(Mix::Learned);
    assert!(
        (260..=300).contains(&learned_small),
        "{learned_small} of 400 notes at 400 B or less in the learned mix"
    );
}
