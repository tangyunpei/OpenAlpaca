use super::*;

const VALID_SKILL: &str = r#"---
name: "Code Review"
description: "Review code for bugs, style issues, and improvements"
invoke:
  slash: "/review"
routing:
  intent:
    - "review.*code"
    - "code review"
tools:
  allow:
    - "file_read"
---

## Instructions

When performing a code review, follow these steps:

1. Read the file(s) specified by the user
2. Analyze the code for bugs, style issues, security concerns
3. Provide feedback organized by severity

## Style

Be constructive and specific. Reference line numbers.
"#;

const MINIMAL_SKILL: &str = r#"---
name: "Minimal"
description: "A minimal skill"
---
"#;

#[test]
fn test_parse_skill_frontmatter_only() {
    let fm = parse_skill_frontmatter(VALID_SKILL).expect("valid skill should parse");
    assert_eq!(fm.name, "Code Review");
    assert_eq!(
        fm.description,
        "Review code for bugs, style issues, and improvements"
    );
    assert_eq!(fm.invoke.slash, Some("/review".to_string()));
    // effective_slash_command strips the "/" prefix
    assert_eq!(fm.effective_slash_command(), Some("review".to_string()));
    assert_eq!(fm.routing.intent, vec!["review.*code", "code review"]);
    assert_eq!(fm.tools.allow, vec!["file_read"]);
}

#[test]
fn test_parse_skill_full() {
    let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
    assert_eq!(doc.frontmatter.name, "Code Review");
    assert!(!doc.body.is_empty());
    assert!(doc.body.contains("When performing a code review"));
    assert!(doc.sections.contains_key("Instructions"));
    assert!(doc.sections.contains_key("Style"));
    assert!(doc.sections["Style"].contains("constructive"));
}

#[test]
fn test_parse_minimal_skill() {
    let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal skill should parse");
    assert_eq!(doc.frontmatter.name, "Minimal");
    assert_eq!(doc.frontmatter.description, "A minimal skill");
    assert_eq!(doc.frontmatter.invoke.slash, None);
    assert!(doc.frontmatter.routing.intent.is_empty());
    assert!(doc.frontmatter.tools.allow.is_empty());
    assert!(doc.body.is_empty());
    assert!(doc.sections.is_empty());
}

#[test]
fn test_parse_missing_name() {
    let input = "---\ndescription: \"test\"\n---\n";
    let err = parse_skill_frontmatter(input).expect_err("missing name should fail");
    assert_eq!(err, SkillParseError::MissingField("name"));
}

#[test]
fn test_parse_missing_description() {
    let input = "---\nname: \"test\"\n---\n";
    let err = parse_skill_frontmatter(input).expect_err("missing description should fail");
    assert_eq!(err, SkillParseError::MissingField("description"));
}

#[test]
fn test_parse_missing_frontmatter() {
    let input = "# No frontmatter\nJust a heading.";
    let err = parse_skill_frontmatter(input).expect_err("no frontmatter should fail");
    assert_eq!(err, SkillParseError::MissingFrontmatter);
}

#[test]
fn test_parse_unterminated_frontmatter() {
    let input = "---\nname: \"test\"\ndescription: \"test\"\n";
    let err = parse_skill_frontmatter(input).expect_err("unterminated should fail");
    assert_eq!(err, SkillParseError::UnterminatedFrontmatter);
}

#[test]
fn test_render_roundtrip() {
    let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
    let rendered = render_skill_markdown(&doc);
    let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
    // Name, description should round-trip
    assert_eq!(doc.frontmatter.name, reparsed.frontmatter.name);
    assert_eq!(
        doc.frontmatter.description,
        reparsed.frontmatter.description
    );
    // Body sections should match
    assert_eq!(
        doc.sections.keys().collect::<Vec<_>>().len(),
        reparsed.sections.keys().collect::<Vec<_>>().len()
    );
    for (key, value) in &doc.sections {
        let reparsed_value = reparsed.sections.get(key).expect("section should exist");
        assert_eq!(
            value.trim(),
            reparsed_value.trim(),
            "Section '{}' content should match",
            key
        );
    }
}

#[test]
fn test_render_minimal_roundtrip() {
    let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal should parse");
    let rendered = render_skill_markdown(&doc);
    let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
    assert_eq!(doc.frontmatter.name, reparsed.frontmatter.name);
    assert_eq!(
        doc.frontmatter.description,
        reparsed.frontmatter.description
    );
}

#[test]
fn test_skill_to_prompt_block_nonempty() {
    let doc = parse_skill_markdown(VALID_SKILL).expect("should parse");
    let block = skill_to_prompt_block(&doc);
    assert!(block.starts_with("<skill_context name=\"Code Review\""));
    assert!(block.contains("When performing a code review"));
    assert!(block.ends_with("</skill_context>"));
}

#[test]
fn test_skill_to_prompt_block_empty_body() {
    let doc = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
    let block = skill_to_prompt_block(&doc);
    assert!(block.is_empty());
}

#[test]
fn test_skill_document_has_content() {
    let full = parse_skill_markdown(VALID_SKILL).expect("should parse");
    assert!(skill_document_has_content(&full));

    let minimal = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
    assert!(!skill_document_has_content(&minimal));
}

#[test]
fn test_unknown_frontmatter_fields_tolerated() {
    let input = r#"---
name: "test"
description: "test"
unknown_field: "hello"
another_unknown:
  - "item1"
---
"#;
    let fm = parse_skill_frontmatter(input).expect("unknown fields should be tolerated");
    assert_eq!(fm.name, "test");
}

#[test]
fn test_unquoted_frontmatter_values() {
    let input = r#"---
name: My Skill
description: A skill without quotes
invoke:
  slash: /my-skill
---
"#;
    let fm = parse_skill_frontmatter(input).expect("unquoted values should parse");
    assert_eq!(fm.name, "My Skill");
    assert_eq!(fm.description, "A skill without quotes");
    assert_eq!(fm.effective_slash_command(), Some("my-skill".to_string()));
}

#[test]
fn test_new_schema_fields() {
    let input = r#"---
name: "Advanced Skill"
description: "A skill using the new schema"
invoke:
  mode: auto
  slash: "/advanced"
routing:
  intent:
    - "advanced.*query"
  keywords:
    - "advanced"
  weights:
    base: 0.3
tools:
  allow:
    - "file_read"
    - "file_write"
  deny:
    - "shell_exec"
permissions:
  level: readwrite
output:
  format: markdown
---

## Instructions

Do something advanced.
"#;
    let fm = parse_skill_frontmatter(input).expect("new schema should parse");
    assert_eq!(fm.name, "Advanced Skill");
    assert_eq!(fm.invoke.mode, "auto");
    assert_eq!(fm.invoke.slash, Some("/advanced".to_string()));
    assert_eq!(fm.routing.intent, vec!["advanced.*query"]);
    assert_eq!(fm.routing.keywords, vec!["advanced"]);
    assert!((fm.routing.weights.base - 0.3).abs() < f64::EPSILON);
    // Other weights should be defaults
    assert!((fm.routing.weights.intent_weight - 0.45).abs() < f64::EPSILON);
    assert_eq!(fm.tools.allow, vec!["file_read", "file_write"]);
    assert_eq!(fm.tools.deny, vec!["shell_exec"]);
    assert_eq!(fm.permissions.level, "readwrite");
    assert_eq!(fm.output.format, Some("markdown".to_string()));
    assert_eq!(fm.effective_slash_command(), Some("advanced".to_string()));
}

#[test]
fn test_routing_score_alias_maps_to_weights() {
    let input = r#"---
name: "Alias Test"
description: "Test routing.score alias"
routing:
  intent:
    - "test"
  score:
    base: 0.5
    intent_weight: 0.3
---
"#;
    let fm = parse_skill_frontmatter(input).expect("routing.score alias should parse");
    assert!((fm.routing.weights.base - 0.5).abs() < f64::EPSILON);
    assert!((fm.routing.weights.intent_weight - 0.3).abs() < f64::EPSILON);
    // Unspecified fields should use defaults
    assert!((fm.routing.weights.keyword_weight - 0.35).abs() < f64::EPSILON);
    assert!((fm.routing.weights.recency_weight - 0.2).abs() < f64::EPSILON);
}

#[test]
fn test_script_config_deserialization() {
    let input = r#"---
name: "Script Skill"
description: "A skill with scripts"
scripts:
  - file: lint.sh
    name: lint_code
    description: "Run linter"
    parameters:
      type: object
      properties:
        path:
          type: string
      required:
        - path
    timeout_secs: 60
  - file: analyze.py
    name: analyze
    description: "Run analysis"
    interpreter: python3
---
"#;
    let fm = parse_skill_frontmatter(input).expect("scripts should parse");
    assert_eq!(fm.scripts.len(), 2);
    assert_eq!(fm.scripts[0].file, "lint.sh");
    assert_eq!(fm.scripts[0].name, "lint_code");
    assert_eq!(fm.scripts[0].timeout_secs, 60);
    assert!(fm.scripts[0].interpreter.is_none());
    assert_eq!(fm.scripts[1].file, "analyze.py");
    assert_eq!(fm.scripts[1].interpreter.as_deref(), Some("python3"));
    // Default timeout
    assert_eq!(fm.scripts[1].timeout_secs, 30);
}

#[test]
fn test_script_config_to_tool_definition() {
    let cfg = ScriptConfig {
        file: "lint.sh".to_string(),
        name: "lint_code".to_string(),
        description: "Run linter".to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        interpreter: None,
        timeout_secs: 30,
    };
    let def = cfg.to_tool_definition();
    assert_eq!(def.name, "skill_script:lint_code");
    assert_eq!(def.description, "Run linter");
    assert_eq!(def.parameters["type"], "object");
}

#[test]
fn test_skill_without_scripts_has_empty_vec() {
    let fm = parse_skill_frontmatter(MINIMAL_SKILL).expect("minimal skill should parse");
    assert!(fm.scripts.is_empty());
}

/// serde_yaml 0.9 read these YAML 1.1 spellings as plain text, and a script's
/// `parameters` schema reaches the model as written. serde-saphyr's defaults
/// would turn `n`/`yes`/`off` into booleans, merge `<<`, refuse the repeated
/// key and quote the file in the error; `yaml_options` keeps all four as they
/// were.
#[test]
fn yaml_1_1_spellings_read_the_way_serde_yaml_0_9_read_them() {
    let input = "---\nname: N\ndescription: n\n\
                 scripts:\n  - file: a.sh\n    name: a\n    description: d\n    \
                 parameters:\n      type: object\n      properties:\n        \
                 flag: {type: string, enum: [yes, no, on, off, y, n], default: yes}\n\
                 tools:\n  defaults:\n    k: 1\n    k: 2\n\
                 invoke:\n  <<: {mode: auto}\n---\n";
    let fm = parse_skill_frontmatter(input).expect("parses");
    assert_eq!(fm.name, "N");
    assert_eq!(fm.description, "n");
    assert_eq!(
        fm.scripts[0].parameters["properties"]["flag"],
        serde_json::json!({
            "type": "string",
            "enum": ["yes", "no", "on", "off", "y", "n"],
            "default": "yes",
        })
    );
    assert_eq!(fm.tools.defaults["k"], serde_json::json!(2));
    assert_eq!(
        fm.invoke.mode, "manual",
        "`<<` is an ordinary key, not a merge"
    );

    let err = parse_skill_frontmatter(
        "---\nname: N\ndescription: D\npermissions:\n  sandbox:\n    net: yes\n---\n",
    )
    .expect_err("`yes` is not a boolean");
    match err {
        SkillParseError::InvalidYaml(msg) => assert!(!msg.contains('\n'), "one line: {msg}"),
        other => panic!("expected InvalidYaml, got {other:?}"),
    }
}

/// A run of more than 32 comment lines between a key and its block value,
/// which serde_yaml 0.9 read and serde-saphyr's default comment handling
/// refuses ("too many consecutive comments"; `emit_comments: false` in
/// `yaml_options`), and non-finite floats in a `serde_json::Value` position,
/// which read as their canonical spelling (serde_yaml read them as null).
#[test]
fn comment_runs_and_non_finite_floats_saphyr_leg_pin() {
    let comments: String = (0..40)
        .map(|i| format!("  # commented-out line {i}\n"))
        .collect();
    let input = format!(
        "---\nname: N\ndescription: D\nrouting:\n{comments}  keywords: [a]\n\
         tools:\n  defaults:\n    a: .inf\n    b: -.inf\n    c: .NaN\n---\n"
    );
    let fm = parse_skill_frontmatter(&input).expect("a long comment run parses");
    assert_eq!(fm.routing.keywords, vec!["a".to_string()]);
    assert_eq!(fm.tools.defaults["a"], serde_json::json!(".inf"));
    assert_eq!(fm.tools.defaults["b"], serde_json::json!("-.inf"));
    assert_eq!(fm.tools.defaults["c"], serde_json::json!(".nan"));
}

/// Frontmatter edge cases, each written fresh and read the way serde_yaml 0.9
/// read it (audit Z3-05; the same values under the serde-saphyr that replaced
/// it): a byte-order mark, CRLF line ends, the four block-scalar styles, a
/// quoted value holding `: `, a nested map, an unknown key and an empty body.
#[test]
fn frontmatter_edge_cases_read_the_way_serde_yaml_0_9_read_them() {
    let fm = parse_skill_frontmatter("\u{feff}---\nname: B\ndescription: d\n---\n")
        .expect("a leading byte-order mark is dropped");
    assert_eq!((fm.name.as_str(), fm.description.as_str()), ("B", "d"));

    let doc = parse_skill_markdown(
        "---\r\nname: C\r\ndescription: d\r\ninvoke:\r\n  slash: /c\r\n---\r\n\
         ## Instructions\r\nDo it.\r\n",
    )
    .expect("CRLF line ends");
    assert_eq!(doc.frontmatter.name, "C");
    assert_eq!(doc.frontmatter.invoke.slash.as_deref(), Some("/c"));
    assert_eq!(doc.sections["Instructions"], "Do it.");

    let fm = parse_skill_frontmatter(
        "---\nname: S\ndescription: >\n  folded\n  text\n\
         routing:\n  intent:\n    - >-\n      strip\n      folded\n    - |\n      clip\n      literal\n\
         \x20   - |+\n      keep\n\n  keywords: [k]\n---\n",
    )
    .expect("block scalars");
    assert_eq!(fm.description, "folded text\n");
    assert_eq!(
        fm.routing.intent,
        vec!["strip folded", "clip\nliteral\n", "keep\n\n"]
    );
    assert_eq!(fm.routing.keywords, vec!["k"]);

    let fm =
        parse_skill_frontmatter("---\nname: \"Q: double\"\ndescription: 'single: quoted'\n---\n")
            .expect("quoted values holding `: `");
    assert_eq!(fm.name, "Q: double");
    assert_eq!(fm.description, "single: quoted");

    let fm = parse_skill_frontmatter(
        "---\nname: M\ndescription: d\npermissions:\n  level: readonly\n  sandbox:\n    net: true\n\
         tools:\n  defaults:\n    outer:\n      inner: [1, two]\n---\n",
    )
    .expect("a nested map");
    assert_eq!(fm.permissions.level, "readonly");
    assert!(fm.permissions.sandbox.net);
    assert_eq!(
        fm.tools.defaults["outer"],
        serde_json::json!({"inner": [1, "two"]})
    );

    let fm = parse_skill_frontmatter(
        "---\nname: U\ndescription: d\nrequire_capabilities: [file_read]\n---\n",
    )
    .expect("an unknown key is read past");
    assert_eq!(fm.unknown_keys.names(), ["require_capabilities"]);
    assert!(fm.requires_capabilities.is_empty());

    let doc = parse_skill_markdown("---\nname: E\ndescription: d\n---\n").expect("an empty body");
    assert_eq!(doc.body, "");
    assert!(doc.sections.is_empty());
}

/// The closing `---` is a whole line: a `----` or `---x` line does not end
/// the frontmatter (audit Z3-05), trailing spaces and a CR do not stop one
/// that is, and a file that ends on it, with no newline, is closed.
#[test]
fn the_closing_delimiter_is_a_whole_line() {
    assert_eq!(
        parse_skill_frontmatter("---\nname: N\ndescription: d\n---x\n").unwrap_err(),
        SkillParseError::UnterminatedFrontmatter
    );
    assert!(matches!(
        parse_skill_frontmatter("---\nname: N\ndescription: d\n----\nmore: x\n---\n"),
        Err(SkillParseError::InvalidYaml(_))
    ));
    let doc = parse_skill_markdown("---\nname: N\ndescription: d\n--- \r\n## A\nbody\n")
        .expect("trailing space and CR after the delimiter");
    assert_eq!(doc.sections["A"], "body");
    let fm = parse_skill_frontmatter("---\nname: N\ndescription: d\n---").expect("no newline");
    assert_eq!(fm.name, "N");
}
