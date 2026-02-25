//! Text extraction for various document formats.
//!
//! Each extractor returns `(Option<String>, Option<String>)` = (extracted_text, error_message).
//! - `(Some(text), None)` — success
//! - `(None, Some(err))` — hard failure
//! - `(Some(""), Some(warning))` — partial success with warning

use std::io::Read;

/// Extract text content from a file based on its MIME type.
pub fn extract_text(
    storage_path: &str,
    mime_type: &str,
    max_chars: usize,
) -> (Option<String>, Option<String>) {
    match mime_type {
        // Plain text (including CSV via text/csv)
        m if m.starts_with("text/") => extract_text_file(storage_path, max_chars),

        // PDF
        "application/pdf" => extract_pdf(storage_path, max_chars),

        // OOXML formats
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            extract_docx(storage_path, max_chars)
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            extract_spreadsheet(storage_path, max_chars)
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            extract_pptx(storage_path, max_chars)
        }

        // Legacy Office formats
        "application/msword" => extract_ole2_legacy(storage_path, "doc", max_chars),
        "application/vnd.ms-excel" => extract_spreadsheet(storage_path, max_chars),
        "application/vnd.ms-powerpoint" => extract_ole2_legacy(storage_path, "ppt", max_chars),

        // iWork formats
        "application/vnd.apple.pages" => extract_iwork(storage_path, "pages", max_chars),
        "application/vnd.apple.numbers" => extract_iwork(storage_path, "numbers", max_chars),
        "application/vnd.apple.keynote" => extract_iwork(storage_path, "keynote", max_chars),

        // Image/audio: no text extraction (handled natively by vision/audio models)
        m if m.starts_with("image/") || m.starts_with("audio/") => (Some(String::new()), None),

        // Unknown type: mark as ready with empty text
        _ => (Some(String::new()), None),
    }
}

fn extract_text_file(path: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => (Some(truncate_text(&text, max_chars)), None),
        Err(e) => (None, Some(format!("Failed to read text file: {e}"))),
    }
}

fn extract_pdf(path: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    match pdf_extract::extract_text(path) {
        Ok(text) => (Some(truncate_text(&text, max_chars)), None),
        Err(e) => (None, Some(format!("PDF extraction failed: {e}"))),
    }
}

fn extract_docx(path: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return (None, Some(format!("Failed to open DOCX: {e}"))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return (None, Some(format!("Failed to read DOCX as ZIP: {e}"))),
    };
    let mut xml_content = String::new();
    if let Ok(mut entry) = archive.by_name("word/document.xml") {
        if entry.read_to_string(&mut xml_content).is_err() {
            return (None, Some("Failed to read word/document.xml".to_string()));
        }
    } else {
        return (None, Some("DOCX missing word/document.xml".to_string()));
    }
    let text = strip_xml_tags(&xml_content);
    (Some(truncate_text(&text, max_chars)), None)
}

fn extract_spreadsheet(path: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    use calamine::{open_workbook_auto, Reader};
    let mut workbook = match open_workbook_auto(path) {
        Ok(wb) => wb,
        Err(e) => return (None, Some(format!("Spreadsheet open failed: {e}"))),
    };
    let mut text = String::new();
    let sheet_names = workbook.sheet_names().to_vec();
    for sheet_name in &sheet_names {
        if text.len() >= max_chars {
            break;
        }
        text.push_str(&format!("## Sheet: {}\n", sheet_name));
        if let Ok(range) = workbook.worksheet_range(sheet_name) {
            for row in range.rows() {
                let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
                text.push_str(&cells.join("\t"));
                text.push('\n');
                if text.len() >= max_chars {
                    break;
                }
            }
        }
    }
    (Some(truncate_text(&text, max_chars)), None)
}

fn extract_pptx(path: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return (None, Some(format!("Failed to open PPTX: {e}"))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return (None, Some(format!("Failed to read PPTX as ZIP: {e}"))),
    };

    let mut text = String::new();
    // PPTX stores slides as ppt/slides/slide1.xml, slide2.xml, etc.
    for i in 1..=999 {
        if text.len() >= max_chars {
            break;
        }
        let slide_name = format!("ppt/slides/slide{}.xml", i);
        if let Ok(mut entry) = archive.by_name(&slide_name) {
            let mut xml = String::new();
            if entry.read_to_string(&mut xml).is_ok() {
                text.push_str("--- Slide ---\n");
                text.push_str(&strip_xml_tags(&xml));
                text.push('\n');
            }
        } else {
            break; // No more slides
        }
    }
    if text.is_empty() {
        (None, Some("PPTX: no slide content found".to_string()))
    } else {
        (Some(truncate_text(&text, max_chars)), None)
    }
}

fn extract_ole2_legacy(path: &str, kind: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    match std::fs::read(path) {
        Ok(data) => {
            let text = extract_printable_text(&data, max_chars);
            if text.len() < 50 {
                (
                    Some(String::new()),
                    Some(format!(
                        "Legacy .{kind} format: limited text extraction (consider converting to modern format)"
                    )),
                )
            } else {
                (Some(text), None)
            }
        }
        Err(e) => (None, Some(format!("Failed to read .{kind} file: {e}"))),
    }
}

fn extract_iwork(path: &str, kind: &str, max_chars: usize) -> (Option<String>, Option<String>) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return (None, Some(format!("Failed to open iWork file: {e}"))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return (None, Some(format!("Failed to read iWork file as ZIP: {e}"))),
    };

    // Try legacy XML index first
    if let Ok(mut entry) = archive.by_name("index.xml") {
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_ok() && !xml.is_empty() {
            let text = strip_xml_tags(&xml);
            return (Some(truncate_text(&text, max_chars)), None);
        }
    }

    // Modern iWork: IWA protobuf files -- extract text runs from all .iwa entries
    let mut text = String::new();
    for i in 0..archive.len() {
        if text.len() >= max_chars {
            break;
        }
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().to_string();
            if name.ends_with(".iwa") {
                // IWA files are snappy-compressed protobuf.
                // Best-effort: read raw bytes and extract printable text runs.
                let mut buf = Vec::new();
                let mut entry = entry;
                if entry.read_to_end(&mut buf).is_ok() {
                    let remaining = max_chars.saturating_sub(text.len());
                    text.push_str(&extract_printable_text(&buf, remaining));
                    text.push('\n');
                }
            }
        }
    }

    if text.trim().is_empty() {
        (
            Some(String::new()),
            Some(format!(
                "iWork .{kind} format: content is protobuf-encoded, limited text extraction available"
            )),
        )
    } else {
        (Some(truncate_text(&text, max_chars)), None)
    }
}

/// Strip XML tags from a string, preserving text content.
fn strip_xml_tags(xml: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut prev_was_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                prev_was_tag = true;
            }
            '>' => {
                in_tag = false;
                if prev_was_tag {
                    result.push(' ');
                    prev_was_tag = false;
                }
            }
            _ if !in_tag => {
                result.push(ch);
                prev_was_tag = false;
            }
            _ => {}
        }
    }
    // Collapse whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate text to max_chars at a character boundary.
fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        text[..text.floor_char_boundary(max_chars)].to_string()
    }
}

/// Extract printable text runs from binary data.
/// Looks for runs of >= 4 printable ASCII characters.
fn extract_printable_text(data: &[u8], max_chars: usize) -> String {
    let mut result = String::new();
    let mut run = String::new();
    for &b in data {
        if (0x20..0x7F).contains(&b) {
            run.push(b as char);
        } else {
            if run.len() >= 4 {
                result.push_str(&run);
                result.push(' ');
            }
            run.clear();
        }
        if result.len() >= max_chars {
            break;
        }
    }
    if run.len() >= 4 {
        result.push_str(&run);
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip_fixture(path: &std::path::Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn test_strip_xml_tags() {
        let xml = "<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>";
        let text = strip_xml_tags(xml);
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn test_strip_xml_tags_preserves_spaces() {
        let xml = "<p>First</p><p>Second</p>";
        let text = strip_xml_tags(xml);
        assert!(text.contains("First"));
        assert!(text.contains("Second"));
    }

    #[test]
    fn test_truncate_text() {
        let long = "a".repeat(1000);
        let truncated = truncate_text(&long, 500);
        assert_eq!(truncated.len(), 500);
    }

    #[test]
    fn test_truncate_text_short() {
        let short = "hello";
        let truncated = truncate_text(short, 500);
        assert_eq!(truncated, "hello");
    }

    #[test]
    fn test_extract_printable_text() {
        let data = b"\x00\x00Hello World\x00\x00More text here\x00";
        let text = extract_printable_text(data, 1000);
        assert!(text.contains("Hello World"));
        assert!(text.contains("More text here"));
    }

    #[test]
    fn test_extract_printable_text_skips_short_runs() {
        let data = b"\x00ab\x00Hello\x00cd\x00";
        let text = extract_printable_text(data, 1000);
        assert!(text.contains("Hello"));
        assert!(!text.contains("ab"));
        assert!(!text.contains("cd"));
    }

    #[test]
    fn test_extract_text_plain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "Hello from text file").unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "text/plain", 10000);
        assert!(error.is_none());
        assert_eq!(text.unwrap(), "Hello from text file");
    }

    #[test]
    fn test_extract_text_csv() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "name,age\nAlice,30\nBob,25").unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "text/csv", 10000);
        assert!(error.is_none());
        assert!(text.unwrap().contains("Alice"));
    }

    #[test]
    fn test_extract_text_docx_minimal_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.docx");
        write_zip_fixture(&path, &[("word/document.xml", "<w:t>Hello DOCX</w:t>")]);
        let (text, error) = extract_text(
            path.to_str().unwrap(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            10000,
        );
        assert!(error.is_none(), "unexpected DOCX error: {error:?}");
        assert!(text.unwrap_or_default().contains("Hello DOCX"));
    }

    #[test]
    fn test_extract_text_pptx_minimal_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.pptx");
        write_zip_fixture(
            &path,
            &[("ppt/slides/slide1.xml", "<a:t>Hello PPTX</a:t>")],
        );
        let (text, error) = extract_text(
            path.to_str().unwrap(),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            10000,
        );
        assert!(error.is_none(), "unexpected PPTX error: {error:?}");
        assert!(text.unwrap_or_default().contains("Hello PPTX"));
    }

    #[test]
    fn test_extract_text_iwork_index_xml_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.pages");
        write_zip_fixture(&path, &[("index.xml", "<sf:p>Hello iWork</sf:p>")]);
        let (text, error) = extract_text(path.to_str().unwrap(), "application/vnd.apple.pages", 10000);
        assert!(error.is_none(), "unexpected iWork error: {error:?}");
        assert!(text.unwrap_or_default().contains("Hello iWork"));
    }

    #[test]
    fn test_extract_text_legacy_doc_best_effort_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.doc");
        std::fs::write(&path, b"\xD0\xCF\x11\xE0binary").unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "application/msword", 10000);
        assert_eq!(text.unwrap_or_default(), "");
        assert!(
            error.unwrap_or_default().contains("Legacy .doc format"),
            "expected legacy DOC warning"
        );
    }

    #[test]
    fn test_extract_text_legacy_ppt_best_effort_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.ppt");
        std::fs::write(&path, b"\xD0\xCF\x11\xE0binary").unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "application/vnd.ms-powerpoint", 10000);
        assert_eq!(text.unwrap_or_default(), "");
        assert!(
            error.unwrap_or_default().contains("Legacy .ppt format"),
            "expected legacy PPT warning"
        );
    }

    #[test]
    fn test_extract_text_xlsx_invalid_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.xlsx");
        std::fs::write(&path, "not-a-workbook").unwrap();
        let (text, error) = extract_text(
            path.to_str().unwrap(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            10000,
        );
        assert!(text.is_none());
        assert!(
            error.unwrap_or_default().contains("Spreadsheet open failed"),
            "expected spreadsheet open failure"
        );
    }

    #[test]
    fn test_extract_text_xls_invalid_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.xls");
        std::fs::write(&path, "not-a-workbook").unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "application/vnd.ms-excel", 10000);
        assert!(text.is_none());
        assert!(
            error.unwrap_or_default().contains("Spreadsheet open failed"),
            "expected spreadsheet open failure"
        );
    }

    #[test]
    fn test_extract_text_image_returns_empty() {
        let (text, error) = extract_text("/dev/null", "image/png", 10000);
        assert!(error.is_none());
        assert_eq!(text.unwrap(), "");
    }

    #[test]
    fn test_extract_text_unknown_mime() {
        let (text, error) = extract_text("/dev/null", "application/x-unknown", 10000);
        assert!(error.is_none());
        assert_eq!(text.unwrap(), "");
    }
}
