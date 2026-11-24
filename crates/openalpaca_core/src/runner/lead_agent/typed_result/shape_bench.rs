//! The manual benchmark of SA-4's checker (design §5.3.4, §5.3.7; the
//! owner's manual layer): the counted worst cases at the full product of
//! the two caps, timed one check each. Run by hand only, under the memory
//! and time guard, in a release build and at opt-level 0:
//!
//! ```text
//! OPENALPACA_SHAPE_BENCH_CASE=<1..4> <test binary> --ignored --exact \
//!   runner::lead_agent::typed_result::shape::shape_bench::shape_worst_cases_benchmark \
//!   --nocapture --test-threads=1
//! ```
//!
//! With no `OPENALPACA_SHAPE_BENCH_CASE` every case runs, in order. The
//! timed binary carries the test-only step counter; its increments are
//! inside the measured time. The schemas carry no `type`: an `items`,
//! `enum` or `required` asserts on its own kind without one, and with it
//! the largest forms do not fit 16,384 bytes.

use super::*;
use serde_json::json;
use std::time::Instant;

/// One worst case: its schema, its value and its step bound (§5.3.4's
/// per-row sum for that schema).
struct Case {
    name: &'static str,
    schema: Value,
    value: Value,
    /// The schema's units, as §5.3.4 counts them.
    units: u64,
}

fn cases() -> Vec<Case> {
    vec![
        // (1) A large `enum` under `items`: 8,175 × 32,767 ≈ 2.68 × 10^8
        // integer comparisons, every element a violation.
        Case {
            name: "enum 8,175 under items against 32,767 elements",
            schema: json!({"items": {"enum": vec![0; 8_175]}}),
            value: json!(vec![1; 32_767]),
            units: 2 + 8_175,
        },
        // (2) A large `anyOf` under `items`: 1,363 `{"const":0}` branches
        // (1,364 do not fit), about 4.47 × 10^7 branch calls.
        Case {
            name: "anyOf 1,363 under items against 32,767 elements",
            schema: json!({"items": {"anyOf": vec![json!({"const": 0}); 1_363]}}),
            value: json!(vec![1; 32_767]),
            units: 2 + 2 * 1_363,
        },
        // (3) Duplicated `required` names under `items`: 4,090 names
        // against 21,845 empty objects, about 8.9 × 10^7 violations.
        Case {
            name: "required 4,090 under items against 21,845 objects",
            schema: json!({"items": {"required": vec!["a"; 4_090]}}),
            value: json!(vec![json!({}); 21_845]),
            units: 2 + 4_090,
        },
        // (4) Long strings in `enum`: 2,727 members of three bytes against
        // 10,922 elements, about 2.98 × 10^7 string comparisons.
        Case {
            name: "enum 2,727 strings under items against 10,922 strings",
            schema: json!({"items": {"enum": vec!["aaa"; 2_727]}}),
            value: json!(vec!["aab"; 10_922]),
            units: 2 + 2_727,
        },
    ]
}

#[test]
#[ignore = "the manual benchmark: run by hand under the guard, one case at a time"]
fn shape_worst_cases_benchmark() {
    let only: Option<usize> = std::env::var("OPENALPACA_SHAPE_BENCH_CASE")
        .ok()
        .and_then(|c| c.parse().ok());
    let profile = if cfg!(debug_assertions) {
        "debug assertions on (opt-level 0 test profile)"
    } else {
        "debug assertions off (release profile)"
    };
    println!("profile: {profile}");
    for (index, case) in cases().into_iter().enumerate() {
        let number = index + 1;
        if only.is_some_and(|n| n != number) {
            continue;
        }
        let schema_bytes = json_size_within(&case.schema, usize::MAX, None).unwrap();
        let value_bytes = json_size_within(&case.value, usize::MAX, None).unwrap();
        assert!(
            schema_bytes <= SCHEMA_MAX_BYTES,
            "case {number}: {schema_bytes}"
        );
        assert!(
            value_bytes <= RESULT_MAX_BYTES,
            "case {number}: {value_bytes}"
        );
        let shape = ResultShape::admit(&case.schema)
            .unwrap_or_else(|e| panic!("case {number} is admitted: {e}"));
        let n = super::tests::nodes(&case.value);
        let bound = super::tests::step_bound(&case.schema, n);

        probe::reset();
        let started = Instant::now();
        let verdict = shape.check(&case.value);
        let elapsed = started.elapsed();
        let steps = probe::steps();

        let (verdict_word, violations) = match &verdict {
            ShapeVerdict::Matches { .. } => ("matches", 0),
            ShapeVerdict::Violations { lines, more } => ("violations", lines.len() + more),
            ShapeVerdict::TooLarge => ("too large", 0),
        };
        println!(
            "case={number} ({}) schema_bytes={schema_bytes} value_bytes={value_bytes} \
             U={} N={n} steps={steps} bound={bound} verdict={verdict_word} \
             violations={violations} elapsed_ms={:.1}",
            case.name,
            case.units,
            elapsed.as_secs_f64() * 1_000.0
        );
        assert!(steps <= bound, "case {number}: {steps} steps over {bound}");
        assert_eq!(verdict_word, "violations", "case {number}");
    }
}
