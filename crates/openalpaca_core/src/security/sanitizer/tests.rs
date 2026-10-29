use super::*;

#[test]
fn test_clean_input_passes() {
    let result = InputSanitizer::sanitize_user_input("Hello, how are you?", None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello, how are you?");
}

#[test]
fn test_oversized_input_blocked() {
    let large = "x".repeat(MAX_INPUT_LENGTH + 1);
    let result = InputSanitizer::sanitize_user_input(&large, None);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("maximum length"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_null_byte_blocked() {
    let result = InputSanitizer::sanitize_user_input("hello\0world", None);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("null bytes"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_path_traversal_in_args() {
    let args = serde_json::json!({"path": "../../etc/passwd"});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("Path traversal"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_shell_semicolon_allowed() {
    // Semicolons are normal shell separators; not blocked
    let args = serde_json::json!({"cmd": "cd /tmp; ls"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_command_injection_backtick() {
    let args = serde_json::json!({"cmd": "echo `whoami`"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("backtick"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_shell_pipe_allowed() {
    // Pipes are a fundamental shell feature — allowed
    let args = serde_json::json!({"command": "grep TODO src/*.rs | wc -l"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_shell_redirect_allowed() {
    // Output redirection is a normal shell feature — allowed
    let args = serde_json::json!({"command": "cargo build > /dev/null 2>&1"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_shell_chaining_allowed() {
    // && and || are normal shell control flow — allowed
    let args = serde_json::json!({"command": "cargo test && cargo build"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_newlines() {
    // file_write content with newlines should NOT be blocked
    let args = serde_json::json!({"path": "test.txt", "content": "line1\nline2\nline3"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_semicolons() {
    // web_search query with semicolons should NOT be blocked
    let args = serde_json::json!({"query": "node.js; tutorial"});
    let result = InputSanitizer::sanitize_tool_args("web_search", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_backticks() {
    // file_write content with backticks (markdown) should NOT be blocked
    let args = serde_json::json!({"content": "```rust\nfn main() {}\n```"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_still_blocks_traversal() {
    // Path traversal is still blocked for all tools
    let args = serde_json::json!({"path": "../../etc/passwd", "content": "safe"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_non_shell_tool_still_blocks_null_bytes() {
    let args = serde_json::json!({"content": "hello\0world"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_unknown_tool_blocked() {
    let args = serde_json::json!({});
    let allowed = vec!["web_search".to_string(), "summarize".to_string()];
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &allowed, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("not in the allowed tools list"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_nested_traversal_in_array() {
    let args = serde_json::json!({"files": ["safe.txt", "../../../secret"]});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_command_backend_tool_injection_blocked_via_extra_shell_tools() {
    // A command-backend tool listed in extra_shell_tools should have
    // injection patterns blocked, just like shell_execute.
    let args = serde_json::json!({"query": "test `whoami`"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("backtick"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_command_backend_tool_blocks_command_substitution() {
    let args = serde_json::json!({"count": "5$(rm -rf /)"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("command substitution"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_command_backend_tool_allows_safe_args() {
    // Safe arguments should pass for command-backend tools
    let args = serde_json::json!({"count": "10"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_ok());
}

#[test]
fn test_non_extra_shell_tool_not_affected() {
    // A tool NOT in extra_shell_tools should still allow backticks
    let args = serde_json::json!({"content": "use `backticks` in markdown"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &extra);
    assert!(result.is_ok());
}

// --- Container-compatible MIME tests ---

#[test]
fn test_is_container_compatible_docx_zip() {
    assert!(InputSanitizer::is_container_compatible_mime(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application/zip",
    ));
}

#[test]
fn test_is_container_compatible_xlsx_zip() {
    assert!(InputSanitizer::is_container_compatible_mime(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "application/zip",
    ));
}

#[test]
fn test_is_container_compatible_pptx_zip() {
    assert!(InputSanitizer::is_container_compatible_mime(
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "application/zip",
    ));
}

#[test]
fn test_is_container_compatible_iwork_zip() {
    for mime in &[
        "application/vnd.apple.pages",
        "application/vnd.apple.numbers",
        "application/vnd.apple.keynote",
    ] {
        assert!(
            InputSanitizer::is_container_compatible_mime(mime, "application/zip"),
            "iWork {mime} should be ZIP-compatible"
        );
    }
}

#[test]
fn test_is_container_compatible_legacy_office_cfb() {
    for mime in &[
        "application/msword",
        "application/vnd.ms-excel",
        "application/vnd.ms-powerpoint",
    ] {
        assert!(
            InputSanitizer::is_container_compatible_mime(mime, "application/x-cfb"),
            "Legacy Office {mime} should be CFB-compatible"
        );
    }
}

#[test]
fn test_is_container_compatible_legacy_office_ole_storage() {
    for mime in &[
        "application/msword",
        "application/vnd.ms-excel",
        "application/vnd.ms-powerpoint",
    ] {
        assert!(
            InputSanitizer::is_container_compatible_mime(mime, "application/x-ole-storage"),
            "Legacy Office {mime} should be OLE-storage-compatible"
        );
    }
}

#[test]
fn test_is_container_compatible_rejects_unknown() {
    assert!(!InputSanitizer::is_container_compatible_mime(
        "application/octet-stream",
        "application/zip",
    ));
}

#[test]
fn test_is_container_compatible_rejects_unknown_container() {
    assert!(!InputSanitizer::is_container_compatible_mime(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application/pdf",
    ));
}

// --- Audio MIME alias tests ---

#[test]
fn test_audio_mime_m4a_aliases() {
    // All M4A alias combinations should be compatible
    let aliases = ["audio/m4a", "audio/mp4", "audio/x-m4a"];
    for a in &aliases {
        for b in &aliases {
            assert!(
                InputSanitizer::is_audio_mime_compatible(a, b),
                "{a} should be compatible with {b}"
            );
        }
    }
}

#[test]
fn test_audio_mime_wav_aliases() {
    let aliases = ["audio/wav", "audio/x-wav", "audio/wave"];
    for a in &aliases {
        for b in &aliases {
            assert!(
                InputSanitizer::is_audio_mime_compatible(a, b),
                "{a} should be compatible with {b}"
            );
        }
    }
}

#[test]
fn test_audio_mime_flac_aliases() {
    assert!(InputSanitizer::is_audio_mime_compatible(
        "audio/flac",
        "audio/x-flac"
    ));
    assert!(InputSanitizer::is_audio_mime_compatible(
        "audio/x-flac",
        "audio/flac"
    ));
}

#[test]
fn test_audio_mime_mp3_aliases() {
    let aliases = ["audio/mpeg", "audio/mp3"];
    for a in &aliases {
        for b in &aliases {
            assert!(
                InputSanitizer::is_audio_mime_compatible(a, b),
                "{a} should be compatible with {b}"
            );
        }
    }
    assert!(!InputSanitizer::is_audio_mime_compatible(
        "audio/mp3",
        "audio/mp4"
    ));
}

#[test]
fn test_audio_mime_rejects_unrelated() {
    // Different audio formats should NOT be compatible
    assert!(!InputSanitizer::is_audio_mime_compatible(
        "audio/mpeg",
        "audio/m4a"
    ));
    assert!(!InputSanitizer::is_audio_mime_compatible(
        "audio/wav",
        "audio/m4a"
    ));
    assert!(!InputSanitizer::is_audio_mime_compatible(
        "audio/flac",
        "audio/mp4"
    ));
    // Non-audio types should also fail
    assert!(!InputSanitizer::is_audio_mime_compatible(
        "application/pdf",
        "audio/m4a"
    ));
}

// ── Boundary edge-case tests ───────────────────────────────────────

#[test]
fn test_empty_input_passes() {
    let result = InputSanitizer::sanitize_user_input("", None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "");
}

#[test]
fn test_input_at_exact_max_length_passes() {
    let exact = "x".repeat(MAX_INPUT_LENGTH);
    let result = InputSanitizer::sanitize_user_input(&exact, None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), MAX_INPUT_LENGTH);
}

#[test]
fn test_input_one_byte_over_max_length_blocked() {
    let over = "x".repeat(MAX_INPUT_LENGTH + 1);
    let result = InputSanitizer::sanitize_user_input(&over, None);
    assert!(result.is_err());
}

#[test]
fn test_null_byte_at_various_positions_blocked() {
    // Leading null
    let result = InputSanitizer::sanitize_user_input("\0hello", None);
    assert!(result.is_err());
    // Trailing null
    let result = InputSanitizer::sanitize_user_input("hello\0", None);
    assert!(result.is_err());
    // Only null
    let result = InputSanitizer::sanitize_user_input("\0", None);
    assert!(result.is_err());
}

#[test]
fn test_double_encoded_traversal_passes_sanitizer() {
    // "%2F" is a literal string, not a decoded slash — sanitizer checks
    // literal "../" patterns, not URL-decoded forms. This is correct behavior
    // because the sanitizer operates on raw strings, not URLs.
    let input = "..%2F..%2Fetc/passwd";
    let result = InputSanitizer::sanitize_user_input(input, None);
    assert!(result.is_ok(), "Double-encoded traversal should pass user input sanitizer (not URL-decoded)");
}

#[test]
fn test_double_encoded_traversal_in_tool_args_passes_sanitizer() {
    // "%2F" is a literal string, not a decoded slash — sanitize_tool_args checks
    // literal "../" patterns, not URL-decoded forms. This should pass.
    let args = serde_json::json!({"path": "..%2F..%2Fetc/passwd"});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_ok(), "Double-encoded traversal should pass tool args sanitizer (not URL-decoded)");
}

#[test]
fn test_null_byte_in_tool_args_path_blocked() {
    // Null byte combined with a path — both checks should catch this
    let args = serde_json::json!({"path": "/etc/pass\u{0000}wd"});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_err());
}

// --- R29: TraversalRule (storage design §11) ---

#[test]
fn sanitize_tool_args_keeps_refusing_a_relative_path() {
    let args = serde_json::json!({"content": "import x from '../lib/x';"});
    for result in [
        InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]),
        InputSanitizer::sanitize_tool_args_with("file_write", &args, &[], &[], TraversalRule::Refuse),
    ] {
        match result {
            Err(SecurityViolation::InputBlocked { reason }) => {
                assert_eq!(reason, "Path traversal pattern detected");
            }
            other => panic!("Expected the traversal refusal, got: {:?}", other),
        }
    }
}

#[test]
fn allow_lets_a_relative_path_through() {
    let args = serde_json::json!({
        "path": "src/app.ts",
        "content": "import x from '../lib/x';\nexport {};"
    });
    assert!(
        InputSanitizer::sanitize_tool_args_with("file_write", &args, &[], &[], TraversalRule::Allow)
            .is_ok()
    );
    let windows = serde_json::json!({"content": "..\\lib\\x"});
    assert!(
        InputSanitizer::sanitize_tool_args_with("file_write", &windows, &[], &[], TraversalRule::Allow)
            .is_ok()
    );
}

#[test]
fn allow_still_refuses_null_bytes_and_a_tool_off_the_list() {
    let args = serde_json::json!({"content": "a\0b"});
    match InputSanitizer::sanitize_tool_args_with("file_write", &args, &[], &[], TraversalRule::Allow) {
        Err(SecurityViolation::InputBlocked { reason }) => {
            assert_eq!(reason, "Argument contains null bytes");
        }
        other => panic!("Expected the null-byte refusal, got: {:?}", other),
    }
    let allowed = vec!["file_read".to_string()];
    assert!(
        InputSanitizer::sanitize_tool_args_with(
            "file_write",
            &serde_json::json!({}),
            &allowed,
            &[],
            TraversalRule::Allow
        )
        .is_err()
    );
}

#[test]
fn allow_still_checks_a_shell_tool_for_injection() {
    assert!(
        InputSanitizer::sanitize_tool_args_with(
            "shell_execute",
            &serde_json::json!({"command": "cat ../README.md"}),
            &[],
            &[],
            TraversalRule::Allow
        )
        .is_ok(),
        "ST-14: the shell is a working directory, not a confinement"
    );
    let err = InputSanitizer::sanitize_tool_args_with(
        "shell_execute",
        &serde_json::json!({"command": "cat ../$(id)"}),
        &[],
        &[],
        TraversalRule::Allow,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Input blocked: Potential command injection: command substitution ($()"
    );
}

// --- Upload sniffing verdicts on infer 0.22 (deps plan P14, P17, P17a, P18;
//     DU-6) ---

/// `validate_upload_with_image_limit` as the upload route calls it, with the
/// route's default image bound; `true` when the upload passes.
fn upload_passes(filename: &str, data: &[u8], declared_mime: &str) -> bool {
    InputSanitizer::validate_upload_with_image_limit(filename, data, declared_mime, 1 << 20, 8192)
        .is_ok()
}

/// A text body whose first KiB names a PDF header, and a CSV whose first cell
/// is AutoCAD 2000's `AC1015` signature, pass as declared `text/*`: infer 0.22
/// reports `application/pdf` (a `%PDF` anywhere in the first KiB) and
/// `image/vnd.dwg`, which `is_text_mime_compatible` (P17, DU-6 (b)) does not
/// hold against a declared text file — the verdict infer 0.16, which read
/// `%PDF` at offset 0 only and had no DWG matcher, reached by classifying
/// neither.
#[test]
fn a_declared_text_upload_with_a_pdf_or_dwg_signature_inside_passes() {
    let notes = b"Notes on file formats.\nA PDF starts with %PDF-1.7 then a comment.\n";
    assert!(upload_passes("notes.txt", notes, "text/plain"));
    let html = b"<!DOCTYPE html><html><body><p>Header is %PDF-1.4</p></body></html>";
    assert!(upload_passes("page.html", html, "text/html"));
    let csv = b"AC1015,Widget,3\nAC1018,Gadget,5\n";
    assert!(upload_passes("parts.csv", csv, "text/csv"));
}

/// A body that starts with `%PDF` is a PDF under infer 0.16 and 0.22 alike, and
/// declared `text/plain` it is refused: the text rule is for a signature
/// inside the text only.
#[test]
fn a_declared_text_upload_that_is_a_pdf_is_refused() {
    let pdf = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let err = InputSanitizer::validate_upload_with_image_limit(
        "notes.txt",
        pdf,
        "text/plain",
        1 << 20,
        8192,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Input blocked: MIME type mismatch: declared 'text/plain' but content detected as \
         'application/pdf'"
    );
}

#[test]
fn text_mime_compatibility_covers_an_incidental_pdf_or_dwg_signature_only() {
    let note = b"A PDF starts with %PDF-1.7";
    let pdf = b"%PDF-1.7\n";
    let dwg = b"AC1015,Widget,3\n";
    let cases: [(&str, &str, &[u8], bool); 8] = [
        ("text/plain", "application/pdf", note, true),
        ("text/html", "application/pdf", note, true),
        ("text/csv", "image/vnd.dwg", dwg, true),
        ("text/plain", "application/pdf", pdf, false),
        ("application/pdf", "application/pdf", note, false),
        ("application/octet-stream", "image/vnd.dwg", dwg, false),
        ("text/plain", "image/png", note, false),
        ("text/plain", "application/zip", dwg, false),
    ];
    for (declared, detected, data, expected) in cases {
        assert_eq!(
            InputSanitizer::is_text_mime_compatible(declared, detected, data),
            expected,
            "{declared} detected as {detected}"
        );
    }
}

/// The first connector media case (DU-6 m): an `audio/mp3` clip passes
/// whether its first frame is MPEG-2 (`FF F3`) or MPEG-1 (`FF FB`): infer 0.22
/// reports both as `audio/mpeg`, and `audio/mp3` is its alias (P17a, DU-6
/// (m-b)). infer 0.16 knew MP3 by an ID3 tag or `FF FB` only — the MPEG-2
/// clip passed unclassified, the MPEG-1 one was refused until P17a.
#[test]
fn an_audio_mp3_upload_of_mpeg_frames_passes() {
    for frame in [[0xFF, 0xF3, 0x44, 0xC4], [0xFF, 0xFB, 0x90, 0x64]] {
        let mut mp3 = frame.to_vec();
        mp3.extend_from_slice(&[0u8; 60]);
        assert!(
            upload_passes("clip.mp3", &mp3, "audio/mp3"),
            "frame {frame:02X?}"
        );
    }
}

/// The second connector media case (DU-6 m): an `image/heic` whose ftyp
/// brand is `heix` is refused. infer 0.22 reports `image/heif`, which no alias
/// admits for a declared `image/heic` (DU-6 (m-c), which would also admit
/// every ordinary iPhone HEIC — refused by infer 0.16 and 0.22 alike — is not
/// taken). infer 0.16 knew HEIF by brand `heic` only, so the upload passed
/// unclassified until P18.
#[test]
fn a_heix_heic_upload_is_refused_as_heif() {
    let mut heic = b"\x00\x00\x00\x18ftypheix\x00\x00\x00\x00mif1heix".to_vec();
    heic.extend_from_slice(&[0u8; 32]);
    let err = InputSanitizer::validate_upload_with_image_limit(
        "photo.heic",
        &heic,
        "image/heic",
        1 << 20,
        8192,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Input blocked: MIME type mismatch: declared 'image/heic' but content detected as \
         'image/heif'"
    );
}
