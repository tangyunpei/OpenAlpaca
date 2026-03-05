//! Layer 2: Input sanitization.
//!
//! Validates and sanitizes user input and tool arguments to prevent
//! injection attacks and unsafe operations.

use super::capabilities::SecurityViolation;

/// Default maximum input length in bytes (32 KB).
const MAX_INPUT_LENGTH: usize = 32 * 1024;

/// Sanitizes user input and tool arguments.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Sanitize user input before processing.
    ///
    /// Checks for:
    /// - Excessive length (> max_input_length, default 32KB)
    /// - Null bytes
    ///
    /// Path traversal and command injection checks are handled by
    /// `sanitize_tool_args`, which is context-aware (only shell tools
    /// are checked for injection patterns).
    ///
    /// Pass `None` for `max_input_length` to use the compiled default (32768 bytes).
    pub fn sanitize_user_input(
        input: &str,
        max_input_length: Option<usize>,
    ) -> Result<String, SecurityViolation> {
        let max_len = max_input_length.unwrap_or(MAX_INPUT_LENGTH);
        // Check length
        if input.len() > max_len {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "Input exceeds maximum length ({} > {} bytes)",
                    input.len(),
                    max_len
                ),
            });
        }

        // Check null bytes
        if input.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Input contains null bytes".to_string(),
            });
        }

        Ok(input.to_string())
    }

    /// Tools whose arguments should be checked for shell injection patterns.
    /// Non-shell tools (file_write, workspace_write, web_search, etc.) can
    /// legitimately contain semicolons, newlines, backticks, and pipes.
    const SHELL_TOOLS: &'static [&'static str] = &["shell_execute"];

    /// Sanitize tool call arguments.
    ///
    /// Checks:
    /// - Tool name is in the allowed list (if non-empty)
    /// - All string values are checked for path traversal and null bytes
    /// - Shell-related tool arguments are additionally checked for hidden
    ///   command injection patterns (backticks, `$(`, newlines). Normal shell
    ///   operators (pipes, redirections, chaining) are allowed because the
    ///   LLM agent constructs the full command intentionally. Non-shell tools
    ///   are NOT subject to these checks so that tools like `file_write` and
    ///   `workspace_write` can accept multi-line content.
    ///
    /// `extra_shell_tools` lists additional tool names (e.g., command-backend
    /// tools loaded from TOML config) that should receive shell injection checks
    /// alongside the hardcoded `SHELL_TOOLS` list.
    pub fn sanitize_tool_args(
        tool_name: &str,
        arguments: &serde_json::Value,
        allowed_tools: &[String],
        extra_shell_tools: &[String],
    ) -> Result<(), SecurityViolation> {
        // Check tool is in allowed list
        if !allowed_tools.is_empty() && !allowed_tools.iter().any(|t| t == tool_name) {
            return Err(SecurityViolation::InputBlocked {
                reason: format!("Tool '{}' is not in the allowed tools list", tool_name),
            });
        }

        let is_shell_tool = Self::SHELL_TOOLS.contains(&tool_name)
            || extra_shell_tools.iter().any(|t| t == tool_name);

        // Recursively check string values in arguments
        Self::check_value_safety(arguments, is_shell_tool)?;

        Ok(())
    }

    fn check_value_safety(
        value: &serde_json::Value,
        check_injection: bool,
    ) -> Result<(), SecurityViolation> {
        match value {
            serde_json::Value::String(s) => Self::check_string_safety(s, check_injection),
            serde_json::Value::Array(arr) => {
                for v in arr {
                    Self::check_value_safety(v, check_injection)?;
                }
                Ok(())
            }
            serde_json::Value::Object(obj) => {
                for v in obj.values() {
                    Self::check_value_safety(v, check_injection)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_string_safety(s: &str, check_injection: bool) -> Result<(), SecurityViolation> {
        // Path traversal — always checked for all tools
        if s.contains("../") || s.contains("..\\") {
            return Err(SecurityViolation::InputBlocked {
                reason: "Path traversal pattern detected".to_string(),
            });
        }

        // Null bytes — always checked for all tools
        if s.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Argument contains null bytes".to_string(),
            });
        }

        // Command injection patterns — only checked for shell-related tools.
        //
        // We only block patterns that enable *hidden* command injection where
        // an attacker could sneak a second command into what the agent thinks
        // is a data string.  Normal shell features like pipes (`|`),
        // redirections (`>`, `<`), and chaining (`&&`, `||`) are intentionally
        // allowed because the LLM agent constructs the full command string and
        // these operators are fundamental to useful shell usage (e.g.
        // `grep foo file.txt | wc -l` or `cargo build > /dev/null 2>&1`).
        if check_injection {
            let injection_patterns = [
                ("`", "backtick command substitution"),
                ("$(", "command substitution ($()"),
                ("\n", "newline (command separator)"),
                ("\r", "carriage return"),
            ];

            for (pattern, desc) in &injection_patterns {
                if s.contains(pattern) {
                    return Err(SecurityViolation::InputBlocked {
                        reason: format!("Potential command injection: {}", desc),
                    });
                }
            }
        }

        Ok(())
    }

    /// Validate a file upload for security issues.
    ///
    /// Checks:
    /// 1. Filename path traversal (`../`, `..\\`, null bytes, absolute paths)
    /// 2. File size against `max_size`
    /// 3. Polyglot detection — declared MIME must match magic-byte-inferred type
    /// 4. Archive bomb heuristic for ZIP files (compressed/uncompressed ratio > 100:1)
    /// 5. Image dimension bounds (width/height must not exceed `max_image_dimension`)
    pub fn validate_upload(
        filename: &str,
        data: &[u8],
        declared_mime: &str,
        max_size: u64,
    ) -> Result<(), SecurityViolation> {
        Self::validate_upload_with_image_limit(filename, data, declared_mime, max_size, 8192)
    }

    /// Full upload validation with configurable image dimension limit.
    pub fn validate_upload_with_image_limit(
        filename: &str,
        data: &[u8],
        declared_mime: &str,
        max_size: u64,
        max_image_dimension: u32,
    ) -> Result<(), SecurityViolation> {
        // 1. Filename path traversal
        if filename.contains("../") || filename.contains("..\\") {
            return Err(SecurityViolation::InputBlocked {
                reason: "Filename contains path traversal".to_string(),
            });
        }
        if filename.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Filename contains null bytes".to_string(),
            });
        }
        if filename.starts_with('/') || filename.starts_with('\\') || filename.starts_with('~') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Filename must be relative (no leading /, \\, or ~)".to_string(),
            });
        }

        // 2. Size check
        if data.len() as u64 > max_size {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "File exceeds maximum size ({} > {} bytes)",
                    data.len(),
                    max_size
                ),
            });
        }

        // 3. Polyglot detection — inferred MIME must match declared MIME,
        //    with allowance for container-based formats (ZIP → OOXML/iWork, CFB → legacy Office)
        if let Some(inferred) = infer::get(data) {
            let inferred_mime = inferred.mime_type();
            if declared_mime != inferred_mime
                && !Self::is_container_compatible_mime(declared_mime, inferred_mime)
                && !Self::is_audio_mime_compatible(declared_mime, inferred_mime)
            {
                return Err(SecurityViolation::InputBlocked {
                    reason: format!(
                        "MIME type mismatch: declared '{}' but content detected as '{}'",
                        declared_mime, inferred_mime
                    ),
                });
            }
        }
        // If infer returns None (e.g. plain text, CSV) — allow. Text formats have
        // no magic bytes and are harmless.

        // 4. Archive bomb heuristic for ZIP-based files (plain ZIP + OOXML + iWork)
        let is_zip_based = declared_mime == "application/zip"
            || declared_mime == "application/x-zip-compressed"
            || Self::is_container_compatible_mime(declared_mime, "application/zip");
        if is_zip_based
            && let Some(ratio) = Self::estimate_zip_ratio(data)
            && ratio > 100.0
        {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "Potential archive bomb: compression ratio {ratio:.0}:1 exceeds 100:1 limit"
                ),
            });
        }

        // 5. Image dimension check
        if declared_mime.starts_with("image/")
            && let Some((w, h)) = read_image_dimensions(data)
            && (w > max_image_dimension || h > max_image_dimension)
        {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "Image dimensions {w}x{h} exceed maximum {max_image_dimension}x{max_image_dimension}"
                ),
            });
        }

        Ok(())
    }

    /// Check if two audio MIME types refer to the same format but use different aliases.
    ///
    /// Browsers, OS file pickers, and the `infer` crate may report different MIME
    /// strings for the same audio format. For example, M4A may be declared as
    /// `audio/mp4` (Chrome/Firefox) or `audio/x-m4a` (Safari) while `infer` detects
    /// it as `audio/m4a`. This function treats known alias groups as compatible.
    pub fn is_audio_mime_compatible(declared: &str, detected: &str) -> bool {
        let m4a = ["audio/m4a", "audio/mp4", "audio/x-m4a"];
        if m4a.contains(&declared) && m4a.contains(&detected) {
            return true;
        }
        let wav = ["audio/wav", "audio/x-wav", "audio/wave"];
        if wav.contains(&declared) && wav.contains(&detected) {
            return true;
        }
        let flac = ["audio/flac", "audio/x-flac"];
        if flac.contains(&declared) && flac.contains(&detected) {
            return true;
        }
        false
    }

    /// Check if a declared MIME type is compatible with a detected container type.
    ///
    /// ZIP-based OOXML and iWork formats detect as `application/zip`.
    /// OLE2-based legacy Office formats detect as `application/x-cfb`.
    /// This is a closed whitelist — only known container-based document types pass.
    pub fn is_container_compatible_mime(declared: &str, detected: &str) -> bool {
        // ZIP-based Office/iWork formats
        if detected == "application/zip" {
            return matches!(
                declared,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                    | "application/vnd.apple.pages"
                    | "application/vnd.apple.numbers"
                    | "application/vnd.apple.keynote"
            );
        }
        // OLE2/CFB-based legacy Office formats
        if detected == "application/x-cfb" || detected == "application/x-ole-storage" {
            return matches!(
                declared,
                "application/msword" | "application/vnd.ms-excel" | "application/vnd.ms-powerpoint"
            );
        }
        false
    }

    /// Estimate the compression ratio from ZIP local file headers.
    ///
    /// Walks all local file headers (PK\x03\x04) and sums compressed/uncompressed
    /// sizes to detect multi-entry archive bombs where each individual entry has
    /// a modest ratio but the aggregate is extreme.
    fn estimate_zip_ratio(data: &[u8]) -> Option<f64> {
        let mut offset = 0usize;
        let mut total_compressed: f64 = 0.0;
        let mut total_uncompressed: f64 = 0.0;
        let mut found_any = false;

        loop {
            if offset + 30 > data.len() {
                break;
            }
            if &data[offset..offset + 4] != b"PK\x03\x04" {
                break;
            }

            let compressed = u32::from_le_bytes([
                data[offset + 18],
                data[offset + 19],
                data[offset + 20],
                data[offset + 21],
            ]) as f64;
            let uncompressed = u32::from_le_bytes([
                data[offset + 22],
                data[offset + 23],
                data[offset + 24],
                data[offset + 25],
            ]) as f64;

            if compressed == 0.0 && uncompressed > 0.0 {
                return Some(f64::INFINITY);
            }

            total_compressed += compressed;
            total_uncompressed += uncompressed;
            found_any = true;

            let filename_len =
                u16::from_le_bytes([data[offset + 26], data[offset + 27]]) as usize;
            let extra_len = u16::from_le_bytes([data[offset + 28], data[offset + 29]]) as usize;
            offset += 30 + filename_len + extra_len + compressed as usize;
        }

        if !found_any {
            return None;
        }
        if total_compressed == 0.0 {
            return Some(if total_uncompressed > 0.0 {
                f64::INFINITY
            } else {
                1.0
            });
        }
        Some(total_uncompressed / total_compressed)
    }
}

/// Read image dimensions from raw bytes by parsing format-specific headers.
/// Supports PNG, JPEG, GIF, and WebP. Returns `(width, height)` or `None`.
fn read_image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG: bytes 16..24 contain width (4 bytes BE) and height (4 bytes BE) in IHDR
    if data.len() >= 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }

    // GIF: bytes 6..10 contain width (2 bytes LE) and height (2 bytes LE)
    if data.len() >= 10 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }

    // WebP: "RIFF" + 4 bytes size + "WEBP" header, then VP8 subchunk with dimensions
    if data.len() >= 30 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        // VP8 (lossy): starts at offset 12 with "VP8 " marker
        if &data[12..16] == b"VP8 " && data.len() >= 30 {
            // Frame dimensions at offset 26 (2 bytes LE width) and 28 (2 bytes LE height)
            let w = u16::from_le_bytes([data[26], data[27]]) as u32;
            let h = u16::from_le_bytes([data[28], data[29]]) as u32;
            return Some((w, h));
        }
        // VP8L (lossless): starts at offset 12 with "VP8L"
        if &data[12..16] == b"VP8L" && data.len() >= 25 {
            // Dimensions packed in 4 bytes at offset 21: width-1 in bits 0..13, height-1 in bits 14..27
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            return Some((w, h));
        }
    }

    // JPEG: scan for SOF0/SOF2 markers (0xFF 0xC0 / 0xFF 0xC2) which contain dimensions
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut pos = 2;
        while pos + 4 < data.len() {
            if data[pos] != 0xFF {
                pos += 1;
                continue;
            }
            let marker = data[pos + 1];
            // SOF markers: 0xC0-0xCF except 0xC4 (DHT), 0xC8 (JPG), 0xCC (DAC)
            if (0xC0..=0xCF).contains(&marker)
                && marker != 0xC4
                && marker != 0xC8
                && marker != 0xCC
                && pos + 9 < data.len()
            {
                let h = u16::from_be_bytes([data[pos + 5], data[pos + 6]]) as u32;
                let w = u16::from_be_bytes([data[pos + 7], data[pos + 8]]) as u32;
                return Some((w, h));
            }
            // Skip this marker segment
            if pos + 3 < data.len() {
                let seg_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
                pos += 2 + seg_len;
            } else {
                break;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests;
