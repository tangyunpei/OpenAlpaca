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
    use calamine::{Reader, open_workbook_auto};
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

fn extract_ole2_legacy(
    path: &str,
    kind: &str,
    max_chars: usize,
) -> (Option<String>, Option<String>) {
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
        write_zip_fixture(&path, &[("ppt/slides/slide1.xml", "<a:t>Hello PPTX</a:t>")]);
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
        let (text, error) =
            extract_text(path.to_str().unwrap(), "application/vnd.apple.pages", 10000);
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
        let (text, error) = extract_text(
            path.to_str().unwrap(),
            "application/vnd.ms-powerpoint",
            10000,
        );
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
            error
                .unwrap_or_default()
                .contains("Spreadsheet open failed"),
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
            error
                .unwrap_or_default()
                .contains("Spreadsheet open failed"),
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

    // ------------------------------------------------------------------------
    // Characterisation of extraction on generated fixtures (deps plan P14;
    // owner decisions DU-7, DU-8, DU-9). Each test pins what pdf-extract 0.12
    // (P15), calamine 0.26.1 and zip 2.4.2 produce on this tree; the commit
    // that moves calamine and zip (P16) changes the expectations named below,
    // and nothing else may.
    // ------------------------------------------------------------------------

    const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    const DOCX_MIME: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

    /// Like `write_zip_fixture`, with deflated entries — what a real OOXML
    /// writer produces, and the one method zip keeps after P16 (DU-7).
    fn write_deflated_zip_fixture(path: &std::path::Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    /// A three-sheet workbook over one shared-strings table. Sheet `Cases`
    /// holds the cell-text cases, one per row beside a plain `Label`; sheets
    /// `StrayV` and `EmptyV` each hold one malformed cell calamine 0.26
    /// rejects the whole sheet for.
    fn write_cases_xlsx(path: &std::path::Path) {
        const CT: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
            r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
            r#"<Default Extension="xml" ContentType="application/xml"/>"#,
            r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
            r#"<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
            r#"<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
            r#"<Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
            r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
            "</Types>",
        );
        const RELS: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>"#,
            "</Relationships>",
        );
        const WORKBOOK: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
            r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
            r#"<workbookPr/><sheets>"#,
            r#"<sheet name="Cases" sheetId="1" r:id="rId1"/>"#,
            r#"<sheet name="StrayV" sheetId="2" r:id="rId2"/>"#,
            r#"<sheet name="EmptyV" sheetId="3" r:id="rId3"/>"#,
            "</sheets></workbook>",
        );
        const WORKBOOK_RELS: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>"#,
            r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>"#,
            r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/>"#,
            r#"<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#,
            "</Relationships>",
        );
        // Index → case. 1: untagged whitespace; 2: `xml:space="preserve"`;
        // 3: a rich run after a plain `<t>`; 4: runs only; 5: CRLF inside;
        // 6: Excel's `_x00HH_` escapes; 7: a literal TAB; 8: a leading LF.
        const SHARED_STRINGS: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="9" uniqueCount="9">"#,
            "<si><t>Label</t></si>",
            "<si><t>  leading and trailing  </t></si>",
            r#"<si><t xml:space="preserve">  preserved spaces  </t></si>"#,
            "<si><t>Plain </t><r><rPr><b/></rPr><t>Bold run</t></r></si>",
            r#"<si><r><t>Run one</t></r><r><t xml:space="preserve"> run two</t></r></si>"#,
            "<si><t>line1\r\nline2</t></si>",
            "<si><t>a_x000D_b_x0009_c_x0000_d_x001B_e</t></si>",
            "<si><t>Tab\there</t></si>",
            "<si><t>\nnewline-led</t></si>",
            "</sst>",
        );
        const SHEET_OPEN: &str = concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        );
        const SHEET_CLOSE: &str = "</sheetData></worksheet>";
        const CASES: &str = concat!(
            r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>"#,
            r#"<row r="2"><c r="A2" t="s"><v>0</v></c><c r="B2" t="s"><v>2</v></c></row>"#,
            r#"<row r="3"><c r="A3" t="s"><v>0</v></c><c r="B3" t="s"><v>3</v></c></row>"#,
            r#"<row r="4"><c r="A4" t="s"><v>0</v></c><c r="B4" t="s"><v>4</v></c></row>"#,
            r#"<row r="5"><c r="A5" t="s"><v>0</v></c><c r="B5" t="s"><v>5</v></c></row>"#,
            r#"<row r="6"><c r="A6" t="s"><v>0</v></c><c r="B6" t="s"><v>6</v></c></row>"#,
            r#"<row r="7"><c r="A7" t="s"><v>0</v></c><c r="B7" t="s"><v>7</v></c></row>"#,
            r#"<row r="8"><c r="A8" t="s"><v>0</v></c><c r="B8" t="s"><v>8</v></c></row>"#,
            // 9: an `inlineStr` with untagged padding, and an inline CDATA.
            r#"<row r="9"><c r="A9" t="inlineStr"><is><t>  inline padded  </t></is></c>"#,
            r#"<c r="B9" t="inlineStr"><is><t><![CDATA[cdata <text> & more]]></t></is></c></row>"#,
        );
        const STRAY_V: &str = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>inline with v</t></is><v>stray</v></c></row>"#;
        const EMPTY_V: &str =
            r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v/></c></row>"#;
        let cases = format!("{SHEET_OPEN}{CASES}{SHEET_CLOSE}");
        let stray_v = format!("{SHEET_OPEN}{STRAY_V}{SHEET_CLOSE}");
        let empty_v = format!("{SHEET_OPEN}{EMPTY_V}{SHEET_CLOSE}");
        write_deflated_zip_fixture(
            path,
            &[
                ("[Content_Types].xml", CT),
                ("_rels/.rels", RELS),
                ("xl/workbook.xml", WORKBOOK),
                ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
                ("xl/sharedStrings.xml", SHARED_STRINGS),
                ("xl/worksheets/sheet1.xml", &cases),
                ("xl/worksheets/sheet2.xml", &stray_v),
                ("xl/worksheets/sheet3.xml", &empty_v),
            ],
        );
    }

    /// Today's cell text, as calamine 0.26.1 reads it and `extract_spreadsheet`
    /// joins it (cells by TAB, rows by LF — `cells.join("\t")`): whitespace is
    /// kept whether or not `xml:space` says so; a plain `<t>` before a rich run
    /// ends the cell, so the run is lost; CRLF, a TAB and a LF inside a cell
    /// come through unchanged and collide with the separators; Excel's
    /// `_x00HH_` escapes stay literal; CDATA inside `<t>` reads as nothing; and
    /// a stray `<v>` beside an `inlineStr` or an empty `<v/>` on a shared-string
    /// cell makes the whole sheet unreadable, which `extract_spreadsheet` skips
    /// without a word. P16 (calamine 0.36, zip 8.6) changes the whitespace, the
    /// rich run, the escapes, the CDATA and the two rejected sheets, and under
    /// DU-9 (b′) maps TAB, LF and CR inside a cell to a space — rows 5, 7 and 8
    /// here — so those expectations move with it, in that commit alone.
    #[test]
    fn xlsx_cell_text_is_pinned_as_calamine_0_26_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cases.xlsx");
        write_cases_xlsx(&path);
        let (text, error) = extract_text(path.to_str().unwrap(), XLSX_MIME, 10000);
        assert!(error.is_none(), "unexpected XLSX error: {error:?}");
        let expected = concat!(
            "## Sheet: Cases\n",
            "Label\t  leading and trailing  \n",
            "Label\t  preserved spaces  \n",
            "Label\tPlain \n",
            "Label\tRun one run two\n",
            "Label\tline1\r\nline2\n",
            "Label\ta_x000D_b_x0009_c_x0000_d_x001B_e\n",
            "Label\tTab\there\n",
            "Label\t\nnewline-led\n",
            "  inline padded  \t\n",
            "## Sheet: StrayV\n",
            "## Sheet: EmptyV\n",
        );
        assert_eq!(text.as_deref(), Some(expected));
    }

    /// `d02_bzip2_entry.docx` (898 bytes): a two-entry DOCX whose entries are
    /// bzip2-compressed, a method OOXML does not allow. Made once with Python's
    /// `zipfile` (`ZIP_BZIP2`) by the dependency research's `make_fixtures.py`
    /// and embedded here because zip loses its `bzip2` feature at P16 and could
    /// not write it afterwards. Today zip 2.4.2's default features read it.
    /// P16 (zip 8.6, `deflate` only) turns this into an unreadable
    /// `word/document.xml` — that commit moves the expectation.
    const D02_BZIP2_ENTRY_DOCX: &[u8] = &[
        0x50, 0x4B, 0x03, 0x04, 0x2E, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x07, 0x76, 0x39, 0x5D, 0xE5,
        0x7F, 0x93, 0x24, 0x7E, 0x01, 0x00, 0x00, 0xB8, 0x03, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00,
        0x5B, 0x43, 0x6F, 0x6E, 0x74, 0x65, 0x6E, 0x74, 0x5F, 0x54, 0x79, 0x70, 0x65, 0x73, 0x5D,
        0x2E, 0x78, 0x6D, 0x6C, 0x42, 0x5A, 0x68, 0x39, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x84,
        0x62, 0x0D, 0xE6, 0x00, 0x00, 0x45, 0x5F, 0x80, 0x00, 0x10, 0x50, 0x0B, 0xF1, 0x57, 0x8F,
        0x01, 0xCE, 0x00, 0x3F, 0xEF, 0xDF, 0xE0, 0x30, 0x01, 0x98, 0x03, 0x43, 0x19, 0x0C, 0x86,
        0x83, 0x41, 0xA3, 0x40, 0x1A, 0x00, 0x1A, 0x18, 0x34, 0x29, 0x3C, 0xA6, 0x09, 0x93, 0x26,
        0x11, 0x9A, 0x08, 0x62, 0x60, 0x23, 0x41, 0x14, 0x85, 0x36, 0x94, 0xD9, 0x53, 0xF6, 0x8A,
        0x9E, 0x99, 0x94, 0x46, 0x83, 0x13, 0x4D, 0xA3, 0x50, 0xDA, 0x96, 0x9A, 0xF4, 0x3F, 0x09,
        0x23, 0xD3, 0xA4, 0x24, 0x46, 0x39, 0xC9, 0x78, 0x0D, 0x08, 0x06, 0x1A, 0xF3, 0x8E, 0xE9,
        0x8C, 0xB4, 0x8F, 0x21, 0x99, 0x85, 0xE5, 0x12, 0x19, 0xFA, 0xC7, 0x6D, 0x79, 0x57, 0x25,
        0xD7, 0x5F, 0x4D, 0xB0, 0xDE, 0x45, 0x86, 0x1A, 0xA4, 0xB6, 0xC7, 0x85, 0x2C, 0x5F, 0x3D,
        0xBD, 0xF9, 0x21, 0xAE, 0x69, 0x86, 0x52, 0x5C, 0xA6, 0x4B, 0xBF, 0x7D, 0x31, 0x90, 0xC6,
        0x9D, 0xCB, 0x04, 0x1E, 0x36, 0x49, 0xA4, 0xD3, 0x53, 0xCA, 0x8B, 0x4A, 0xD6, 0xC6, 0x09,
        0xB1, 0xED, 0xD9, 0xF9, 0x4E, 0xCA, 0xAF, 0x33, 0xCD, 0x2E, 0xF5, 0x5E, 0xF9, 0x2F, 0x33,
        0x7F, 0xB0, 0x3C, 0x39, 0x9C, 0xB4, 0xF7, 0x49, 0x0E, 0x84, 0x5C, 0x93, 0x9A, 0x6E, 0x4D,
        0x13, 0x63, 0x04, 0xE0, 0xB7, 0xF4, 0x5B, 0x26, 0x80, 0xFA, 0x93, 0x25, 0xB7, 0x16, 0xF4,
        0x85, 0xBD, 0x7F, 0xA3, 0x07, 0x5E, 0x24, 0xAC, 0x2E, 0x69, 0xC5, 0x28, 0xB8, 0x65, 0xA1,
        0x07, 0x68, 0xB9, 0xC6, 0xB7, 0x2D, 0x52, 0xA9, 0x5E, 0x12, 0x32, 0x5A, 0xB1, 0xA6, 0x1B,
        0xCD, 0xC9, 0x9A, 0x87, 0x54, 0xCB, 0x32, 0x6B, 0x9A, 0xC6, 0x34, 0xF8, 0x3C, 0xE5, 0x7A,
        0xD1, 0x28, 0x97, 0x8B, 0x8A, 0x40, 0x1A, 0xE2, 0x25, 0x7B, 0x46, 0xB3, 0xA3, 0xF3, 0x96,
        0xE5, 0x60, 0x08, 0x5D, 0x12, 0xDE, 0x29, 0x4B, 0x84, 0xBA, 0x29, 0xD1, 0x7E, 0x2C, 0x07,
        0x54, 0xE4, 0xB6, 0xDF, 0xF5, 0xE2, 0xB8, 0xAC, 0x3C, 0x57, 0x35, 0xD5, 0x32, 0x9A, 0x4C,
        0xC3, 0x6A, 0xF2, 0x4C, 0x09, 0x03, 0xC0, 0x6E, 0x48, 0x8C, 0xC3, 0x1C, 0x45, 0x82, 0x51,
        0xF2, 0x69, 0x05, 0x09, 0x03, 0xC7, 0x50, 0x44, 0x13, 0x10, 0x68, 0x29, 0xF3, 0x0A, 0x20,
        0xC3, 0x5B, 0xE1, 0xB0, 0xA3, 0x32, 0x06, 0x5C, 0x92, 0x5C, 0xFB, 0x89, 0x01, 0xF2, 0x03,
        0x72, 0x63, 0xD4, 0x9A, 0xCC, 0xF0, 0xA9, 0xC6, 0x87, 0x7A, 0x75, 0x4B, 0x64, 0xB6, 0x21,
        0x7F, 0x52, 0x4E, 0x5F, 0x01, 0x48, 0x2C, 0xB7, 0x75, 0x37, 0x66, 0x7E, 0xC0, 0x88, 0xFF,
        0x8B, 0xB9, 0x22, 0x9C, 0x28, 0x48, 0x42, 0x31, 0x06, 0xF3, 0x00, 0x50, 0x4B, 0x03, 0x04,
        0x2E, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x07, 0x76, 0x39, 0x5D, 0xB7, 0xA7, 0xC7, 0x8B, 0x0E,
        0x01, 0x00, 0x00, 0x41, 0x01, 0x00, 0x00, 0x11, 0x00, 0x00, 0x00, 0x77, 0x6F, 0x72, 0x64,
        0x2F, 0x64, 0x6F, 0x63, 0x75, 0x6D, 0x65, 0x6E, 0x74, 0x2E, 0x78, 0x6D, 0x6C, 0x42, 0x5A,
        0x68, 0x39, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x9D, 0x34, 0x1F, 0xF1, 0x00, 0x00, 0x29,
        0xDF, 0x9D, 0x00, 0x10, 0x50, 0x07, 0xF1, 0x57, 0x8D, 0x40, 0x8E, 0x40, 0x3F, 0xE7, 0xDF,
        0xE0, 0x42, 0x20, 0x00, 0x02, 0x08, 0x00, 0x06, 0x00, 0x30, 0x00, 0xCD, 0x88, 0x89, 0x03,
        0x40, 0x00, 0x01, 0x90, 0x00, 0x00, 0x00, 0x1A, 0x9A, 0x05, 0x3D, 0x30, 0x50, 0xC4, 0xD0,
        0xD3, 0x46, 0x86, 0x80, 0x64, 0x32, 0x00, 0x4A, 0x9E, 0xA9, 0x3F, 0x54, 0x68, 0x68, 0xF5,
        0x00, 0x1A, 0x0C, 0x81, 0xA0, 0x0D, 0x01, 0xB5, 0x19, 0x96, 0xE1, 0x8C, 0x2C, 0xC8, 0x0E,
        0x59, 0xA7, 0x6F, 0xAB, 0x23, 0x80, 0x3B, 0xA4, 0xAF, 0xDF, 0xDA, 0x21, 0x23, 0x32, 0x46,
        0x40, 0x10, 0x60, 0x42, 0xBB, 0x00, 0xCD, 0x2E, 0x7B, 0x80, 0x84, 0xA9, 0x83, 0xAE, 0xEA,
        0x73, 0xAD, 0x93, 0x44, 0x4C, 0x80, 0x95, 0xF1, 0xCA, 0x4F, 0x27, 0xBA, 0x49, 0xC3, 0x0B,
        0x32, 0x18, 0x19, 0x92, 0xC7, 0x43, 0xF9, 0x3E, 0x85, 0xCE, 0x3E, 0x4E, 0xB5, 0xFE, 0x3A,
        0x45, 0x0A, 0x48, 0xD8, 0x21, 0x6F, 0x5A, 0x74, 0x50, 0x80, 0x70, 0x4B, 0x12, 0x1C, 0x5F,
        0x05, 0x5D, 0x77, 0x17, 0xC0, 0x45, 0x90, 0xD2, 0xF7, 0x57, 0x91, 0x10, 0x07, 0x62, 0xA7,
        0x44, 0xD2, 0x07, 0x01, 0x29, 0x35, 0x5B, 0xD2, 0xEB, 0xC6, 0x1B, 0x48, 0xD8, 0x50, 0xB2,
        0x61, 0x20, 0x48, 0x93, 0x13, 0x1E, 0x23, 0x78, 0x59, 0xD5, 0xC2, 0xC2, 0xC2, 0xAA, 0x80,
        0x9A, 0xB8, 0xE3, 0xCD, 0x15, 0x83, 0x22, 0x55, 0x50, 0x40, 0xA5, 0x32, 0x5B, 0x9E, 0x84,
        0xBE, 0x76, 0xA2, 0x93, 0xAC, 0x8C, 0x44, 0x15, 0x89, 0x9A, 0xA7, 0x12, 0x31, 0xB6, 0x02,
        0x32, 0x13, 0x90, 0x27, 0x22, 0xD4, 0x5C, 0x1A, 0x45, 0x7C, 0xC1, 0x4C, 0x2A, 0xAA, 0x19,
        0xEC, 0xC3, 0xF8, 0xBB, 0x92, 0x29, 0xC2, 0x84, 0x84, 0xE9, 0xA0, 0xFF, 0x88, 0x50, 0x4B,
        0x01, 0x02, 0x2E, 0x03, 0x2E, 0x00, 0x00, 0x00, 0x0C, 0x00, 0x07, 0x76, 0x39, 0x5D, 0xE5,
        0x7F, 0x93, 0x24, 0x7E, 0x01, 0x00, 0x00, 0xB8, 0x03, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x5B,
        0x43, 0x6F, 0x6E, 0x74, 0x65, 0x6E, 0x74, 0x5F, 0x54, 0x79, 0x70, 0x65, 0x73, 0x5D, 0x2E,
        0x78, 0x6D, 0x6C, 0x50, 0x4B, 0x01, 0x02, 0x2E, 0x03, 0x2E, 0x00, 0x00, 0x00, 0x0C, 0x00,
        0x07, 0x76, 0x39, 0x5D, 0xB7, 0xA7, 0xC7, 0x8B, 0x0E, 0x01, 0x00, 0x00, 0x41, 0x01, 0x00,
        0x00, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01,
        0xAF, 0x01, 0x00, 0x00, 0x77, 0x6F, 0x72, 0x64, 0x2F, 0x64, 0x6F, 0x63, 0x75, 0x6D, 0x65,
        0x6E, 0x74, 0x2E, 0x78, 0x6D, 0x6C, 0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x02,
        0x00, 0x02, 0x00, 0x80, 0x00, 0x00, 0x00, 0xEC, 0x02, 0x00, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn a_docx_with_bzip2_entries_is_read_by_zip_2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bzip2.docx");
        std::fs::write(&path, D02_BZIP2_ENTRY_DOCX).unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), DOCX_MIME, 10000);
        assert!(error.is_none(), "unexpected DOCX error: {error:?}");
        assert_eq!(
            text.as_deref(),
            Some("Hello DOCX, 你好, café. Second paragraph.")
        );
    }

    /// `e01_p01_encrypted_v4.pdf` (1,380 bytes): `make_fixtures.py`'s two-page
    /// `p01_simple.pdf` encrypted with the standard security handler, V4 /
    /// AES-128 (`/StdCF`, `AESV2`), an empty user password and
    /// `0123456789abcdef` as its file identifier. Made once with lopdf 0.42's
    /// `Document::encrypt` by the dependency research's comparison tool
    /// (`cmp/src/main.rs`, `run_encrypt(…, "v4")`) and embedded here because
    /// lopdf is no dependency of the daemon's own, and the 0.34 the tree held
    /// when this was written had no encryption API.
    const E01_AES128_PDF: &[u8] = &[
        0x25, 0x50, 0x44, 0x46, 0x2D, 0x31, 0x2E, 0x34, 0x0A, 0x25, 0xE2, 0xE3, 0xCF, 0xD3, 0x0A,
        0x31, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F, 0x54, 0x79, 0x70, 0x65,
        0x2F, 0x43, 0x61, 0x74, 0x61, 0x6C, 0x6F, 0x67, 0x2F, 0x50, 0x61, 0x67, 0x65, 0x73, 0x20,
        0x32, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64, 0x6F, 0x62, 0x6A, 0x0A,
        0x32, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F, 0x54, 0x79, 0x70, 0x65,
        0x2F, 0x50, 0x61, 0x67, 0x65, 0x73, 0x2F, 0x4B, 0x69, 0x64, 0x73, 0x5B, 0x33, 0x20, 0x30,
        0x20, 0x52, 0x20, 0x34, 0x20, 0x30, 0x20, 0x52, 0x5D, 0x2F, 0x43, 0x6F, 0x75, 0x6E, 0x74,
        0x20, 0x32, 0x2F, 0x4D, 0x65, 0x64, 0x69, 0x61, 0x42, 0x6F, 0x78, 0x5B, 0x30, 0x20, 0x30,
        0x20, 0x36, 0x31, 0x32, 0x20, 0x37, 0x39, 0x32, 0x5D, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64,
        0x6F, 0x62, 0x6A, 0x0A, 0x33, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F,
        0x54, 0x79, 0x70, 0x65, 0x2F, 0x50, 0x61, 0x67, 0x65, 0x2F, 0x50, 0x61, 0x72, 0x65, 0x6E,
        0x74, 0x20, 0x32, 0x20, 0x30, 0x20, 0x52, 0x2F, 0x52, 0x65, 0x73, 0x6F, 0x75, 0x72, 0x63,
        0x65, 0x73, 0x3C, 0x3C, 0x2F, 0x46, 0x6F, 0x6E, 0x74, 0x3C, 0x3C, 0x2F, 0x46, 0x31, 0x20,
        0x35, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x3E, 0x3E, 0x2F, 0x43, 0x6F, 0x6E, 0x74, 0x65,
        0x6E, 0x74, 0x73, 0x20, 0x36, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64,
        0x6F, 0x62, 0x6A, 0x0A, 0x34, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F,
        0x54, 0x79, 0x70, 0x65, 0x2F, 0x50, 0x61, 0x67, 0x65, 0x2F, 0x50, 0x61, 0x72, 0x65, 0x6E,
        0x74, 0x20, 0x32, 0x20, 0x30, 0x20, 0x52, 0x2F, 0x52, 0x65, 0x73, 0x6F, 0x75, 0x72, 0x63,
        0x65, 0x73, 0x3C, 0x3C, 0x2F, 0x46, 0x6F, 0x6E, 0x74, 0x3C, 0x3C, 0x2F, 0x46, 0x31, 0x20,
        0x35, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x3E, 0x3E, 0x2F, 0x43, 0x6F, 0x6E, 0x74, 0x65,
        0x6E, 0x74, 0x73, 0x20, 0x37, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64,
        0x6F, 0x62, 0x6A, 0x0A, 0x35, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F,
        0x54, 0x79, 0x70, 0x65, 0x2F, 0x46, 0x6F, 0x6E, 0x74, 0x2F, 0x53, 0x75, 0x62, 0x74, 0x79,
        0x70, 0x65, 0x2F, 0x54, 0x79, 0x70, 0x65, 0x31, 0x2F, 0x42, 0x61, 0x73, 0x65, 0x46, 0x6F,
        0x6E, 0x74, 0x2F, 0x48, 0x65, 0x6C, 0x76, 0x65, 0x74, 0x69, 0x63, 0x61, 0x2F, 0x45, 0x6E,
        0x63, 0x6F, 0x64, 0x69, 0x6E, 0x67, 0x2F, 0x57, 0x69, 0x6E, 0x41, 0x6E, 0x73, 0x69, 0x45,
        0x6E, 0x63, 0x6F, 0x64, 0x69, 0x6E, 0x67, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64, 0x6F, 0x62,
        0x6A, 0x0A, 0x36, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C, 0x3C, 0x2F, 0x4C, 0x65,
        0x6E, 0x67, 0x74, 0x68, 0x20, 0x31, 0x37, 0x36, 0x2F, 0x46, 0x69, 0x6C, 0x74, 0x65, 0x72,
        0x2F, 0x46, 0x6C, 0x61, 0x74, 0x65, 0x44, 0x65, 0x63, 0x6F, 0x64, 0x65, 0x3E, 0x3E, 0x73,
        0x74, 0x72, 0x65, 0x61, 0x6D, 0x0A, 0x19, 0xB1, 0x16, 0x00, 0xD9, 0xB6, 0x56, 0xEA, 0xF6,
        0xE2, 0x3D, 0xE5, 0x07, 0x39, 0x27, 0x38, 0xD2, 0x04, 0x95, 0xF3, 0x70, 0x11, 0x93, 0xB8,
        0xF7, 0x83, 0xC5, 0x5B, 0x2B, 0xB9, 0xF0, 0xEE, 0x74, 0x5A, 0xCF, 0xEC, 0x05, 0xAA, 0x10,
        0x4B, 0x96, 0x6E, 0x12, 0x54, 0x67, 0x11, 0xC8, 0x30, 0xC2, 0xF4, 0xE2, 0x56, 0x07, 0x4C,
        0x5B, 0x79, 0x9B, 0x7F, 0xA8, 0x27, 0x2B, 0xC6, 0xA6, 0x14, 0x03, 0xCA, 0x1F, 0x69, 0x63,
        0xA0, 0x4C, 0xF1, 0x65, 0x36, 0x2A, 0x3D, 0x05, 0x1E, 0xCD, 0x0A, 0xCA, 0xA8, 0xC0, 0x53,
        0x98, 0xAD, 0xC1, 0x29, 0x3E, 0x0D, 0xBC, 0xB1, 0x47, 0x02, 0xD7, 0xEC, 0x6B, 0x6C, 0xC1,
        0xA1, 0xCE, 0x55, 0xFB, 0xE2, 0xDE, 0x14, 0x32, 0x95, 0xDC, 0x54, 0x39, 0x31, 0xD0, 0xD4,
        0x41, 0xB5, 0x91, 0x73, 0x26, 0x84, 0x56, 0x95, 0x10, 0xD4, 0xBB, 0x02, 0x13, 0x15, 0x16,
        0x8C, 0x51, 0x42, 0x09, 0x87, 0x0D, 0x3A, 0x34, 0x93, 0x7D, 0x9C, 0xDF, 0xCA, 0x4A, 0x0C,
        0x3E, 0x56, 0xBF, 0xF0, 0xFF, 0x9C, 0x46, 0x72, 0x57, 0xE2, 0xAE, 0x53, 0x9C, 0x21, 0x07,
        0xA5, 0x03, 0x49, 0x8F, 0x44, 0x7E, 0x0F, 0x27, 0x37, 0x48, 0xEB, 0xB5, 0x10, 0x8D, 0x57,
        0xD3, 0x09, 0x0A, 0x65, 0x6E, 0x64, 0x73, 0x74, 0x72, 0x65, 0x61, 0x6D, 0x20, 0x0A, 0x65,
        0x6E, 0x64, 0x6F, 0x62, 0x6A, 0x0A, 0x37, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C,
        0x3C, 0x2F, 0x4C, 0x65, 0x6E, 0x67, 0x74, 0x68, 0x20, 0x31, 0x31, 0x32, 0x2F, 0x46, 0x69,
        0x6C, 0x74, 0x65, 0x72, 0x2F, 0x46, 0x6C, 0x61, 0x74, 0x65, 0x44, 0x65, 0x63, 0x6F, 0x64,
        0x65, 0x3E, 0x3E, 0x73, 0x74, 0x72, 0x65, 0x61, 0x6D, 0x0A, 0x30, 0x74, 0x7E, 0xFF, 0xB0,
        0xE0, 0xDE, 0x92, 0x14, 0x01, 0x35, 0xF4, 0x05, 0xE2, 0xC8, 0x73, 0x06, 0xC5, 0x6F, 0x0D,
        0x61, 0x03, 0xB6, 0x50, 0x5F, 0x85, 0xAC, 0x9D, 0x50, 0x40, 0x14, 0x8F, 0x54, 0xF3, 0xDC,
        0x29, 0x69, 0xA7, 0xAD, 0xE6, 0x89, 0x0E, 0x17, 0xE5, 0xED, 0x41, 0x47, 0x08, 0xC6, 0xA5,
        0xD1, 0x7F, 0x8A, 0x6E, 0x64, 0xE8, 0x20, 0x1A, 0x37, 0x49, 0x74, 0x43, 0x8A, 0x10, 0x0D,
        0x18, 0x08, 0x8C, 0xD0, 0x90, 0x17, 0x23, 0xA4, 0x84, 0x4E, 0xAB, 0xE2, 0x03, 0xED, 0x37,
        0xBC, 0xDE, 0xBD, 0x5F, 0xC1, 0x11, 0x16, 0x45, 0x2F, 0x98, 0x0B, 0x3B, 0x9C, 0xB2, 0xA2,
        0x4A, 0x42, 0x5D, 0xCA, 0x3A, 0xA9, 0x56, 0xA4, 0xDD, 0x9D, 0x1F, 0x84, 0x14, 0xCE, 0x12,
        0xCB, 0x74, 0x0A, 0x65, 0x6E, 0x64, 0x73, 0x74, 0x72, 0x65, 0x61, 0x6D, 0x20, 0x0A, 0x65,
        0x6E, 0x64, 0x6F, 0x62, 0x6A, 0x0A, 0x38, 0x20, 0x30, 0x20, 0x6F, 0x62, 0x6A, 0x0A, 0x3C,
        0x3C, 0x2F, 0x46, 0x69, 0x6C, 0x74, 0x65, 0x72, 0x2F, 0x53, 0x74, 0x61, 0x6E, 0x64, 0x61,
        0x72, 0x64, 0x2F, 0x56, 0x20, 0x34, 0x2F, 0x52, 0x20, 0x34, 0x2F, 0x4C, 0x65, 0x6E, 0x67,
        0x74, 0x68, 0x20, 0x31, 0x32, 0x38, 0x2F, 0x45, 0x6E, 0x63, 0x72, 0x79, 0x70, 0x74, 0x4D,
        0x65, 0x74, 0x61, 0x64, 0x61, 0x74, 0x61, 0x20, 0x74, 0x72, 0x75, 0x65, 0x2F, 0x4F, 0x28,
        0x56, 0x6F, 0xA8, 0x73, 0xEE, 0x33, 0xC7, 0x97, 0xCD, 0x3B, 0x90, 0x4F, 0xDA, 0xDF, 0x81,
        0x4A, 0xFA, 0x34, 0xDF, 0x9A, 0x38, 0xF6, 0xED, 0x41, 0xB9, 0x84, 0xE2, 0xC6, 0xDA, 0x2A,
        0xA6, 0xF5, 0x29, 0x2F, 0x55, 0x28, 0x0E, 0x8D, 0x23, 0x1A, 0x4C, 0x0E, 0x10, 0xD1, 0x18,
        0xC4, 0x74, 0x3D, 0xC4, 0x47, 0x0F, 0x23, 0xA3, 0x45, 0xEB, 0x6E, 0x1C, 0xA3, 0xCD, 0xF2,
        0x58, 0x7E, 0xD2, 0x25, 0x2C, 0xC3, 0xEC, 0xE0, 0x29, 0x2F, 0x50, 0x20, 0x2D, 0x33, 0x38,
        0x38, 0x34, 0x2F, 0x43, 0x46, 0x3C, 0x3C, 0x2F, 0x53, 0x74, 0x64, 0x43, 0x46, 0x3C, 0x3C,
        0x2F, 0x54, 0x79, 0x70, 0x65, 0x2F, 0x43, 0x72, 0x79, 0x70, 0x74, 0x46, 0x69, 0x6C, 0x74,
        0x65, 0x72, 0x2F, 0x43, 0x46, 0x4D, 0x2F, 0x41, 0x45, 0x53, 0x56, 0x32, 0x3E, 0x3E, 0x3E,
        0x3E, 0x2F, 0x53, 0x74, 0x6D, 0x46, 0x2F, 0x53, 0x74, 0x64, 0x43, 0x46, 0x2F, 0x53, 0x74,
        0x72, 0x46, 0x2F, 0x53, 0x74, 0x64, 0x43, 0x46, 0x3E, 0x3E, 0x0A, 0x65, 0x6E, 0x64, 0x6F,
        0x62, 0x6A, 0x0A, 0x78, 0x72, 0x65, 0x66, 0x0A, 0x30, 0x20, 0x39, 0x0A, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x36, 0x35, 0x35, 0x33, 0x35, 0x20, 0x66,
        0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x31, 0x35, 0x20, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
        0x36, 0x30, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x31, 0x33, 0x39, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6E,
        0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x32, 0x32, 0x39, 0x20, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33,
        0x31, 0x39, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x34, 0x30, 0x37, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6E,
        0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x36, 0x35, 0x31, 0x20, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x38,
        0x33, 0x31, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6E, 0x20, 0x0A, 0x74, 0x72, 0x61,
        0x69, 0x6C, 0x65, 0x72, 0x0A, 0x3C, 0x3C, 0x2F, 0x53, 0x69, 0x7A, 0x65, 0x20, 0x39, 0x2F,
        0x52, 0x6F, 0x6F, 0x74, 0x20, 0x31, 0x20, 0x30, 0x20, 0x52, 0x2F, 0x49, 0x44, 0x5B, 0x3C,
        0x33, 0x30, 0x33, 0x31, 0x33, 0x32, 0x33, 0x33, 0x33, 0x34, 0x33, 0x35, 0x33, 0x36, 0x33,
        0x37, 0x33, 0x38, 0x33, 0x39, 0x36, 0x31, 0x36, 0x32, 0x36, 0x33, 0x36, 0x34, 0x36, 0x35,
        0x36, 0x36, 0x3E, 0x3C, 0x33, 0x30, 0x33, 0x31, 0x33, 0x32, 0x33, 0x33, 0x33, 0x34, 0x33,
        0x35, 0x33, 0x36, 0x33, 0x37, 0x33, 0x38, 0x33, 0x39, 0x36, 0x31, 0x36, 0x32, 0x36, 0x33,
        0x36, 0x34, 0x36, 0x35, 0x36, 0x36, 0x3E, 0x5D, 0x2F, 0x45, 0x6E, 0x63, 0x72, 0x79, 0x70,
        0x74, 0x20, 0x38, 0x20, 0x30, 0x20, 0x52, 0x3E, 0x3E, 0x0A, 0x73, 0x74, 0x61, 0x72, 0x74,
        0x78, 0x72, 0x65, 0x66, 0x0A, 0x31, 0x30, 0x35, 0x33, 0x0A, 0x25, 0x25, 0x45, 0x4F, 0x46,
    ];

    /// pdf-extract 0.12 (lopdf 0.42) reads an AES-128 PDF with an empty user
    /// password. 0.8.2 (lopdf 0.34) could not: `extract_text` answered
    /// `(None, Some(_))` with "the document uses an encryption scheme that is
    /// not supported", which the worker in `background.rs` stored as an `Error`
    /// row tagged `[attempt:N]` and retried up to `extraction_retry_count`
    /// times — so such a row still within its retries is read on its next one.
    #[test]
    fn an_aes_128_pdf_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.pdf");
        std::fs::write(&path, E01_AES128_PDF).unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "application/pdf", 10000);
        assert_eq!(error, None);
        assert_eq!(
            text.as_deref(),
            Some(concat!(
                "\n\nQuarterly report: revenue rose 12% to EUR 4.2m.\n",
                "Kerned wordshere\n",
                "Café naïve résumé ± 5\n",
                "\nPage two, first line.\n",
                "Second line on page two.",
            ))
        );
    }

    /// A minimal PDF, as `make_fixtures.py`'s `build_pdf` writes one: the
    /// objects numbered from 1, a classic xref table, object 1 the catalog.
    fn build_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A stream object; `entries` are added to its dictionary after `/Length`
    /// (`make_fixtures.py`'s `stream(data, extra)`).
    fn pdf_stream(entries: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = format!("<< /Length {} ", data.len()).into_bytes();
        out.extend_from_slice(entries);
        out.extend_from_slice(b">>\nstream\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    /// RUSTSEC-2026-0187: an unreferenced object holding 10,000 nested arrays.
    /// pdf-extract 0.8.2's lopdf 0.34 parsed it with no depth bound, overflowed
    /// the stack and aborted the process — the daemon itself, since the worker
    /// extracts in `spawn_blocking` — so this test could not run before P15.
    /// lopdf 0.42 stops at `MAX_NESTING_DEPTH` (100) and the page is read.
    #[test]
    fn a_pdf_with_ten_thousand_nested_arrays_is_read() {
        let depth = 10_000;
        let mut nested = vec![b'['; depth];
        nested.extend(std::iter::repeat_n(b']', depth));
        let pdf = build_pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            pdf_stream(
                b"",
                b"BT /F1 12 Tf 72 700 Td (Nested arrays live in object 6) Tj ET",
            ),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_vec(),
            nested,
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested.pdf");
        std::fs::write(&path, pdf).unwrap();
        let (text, error) = extract_text(path.to_str().unwrap(), "application/pdf", 10000);
        assert!(error.is_none(), "unexpected PDF error: {error:?}");
        assert!(
            text.as_deref()
                .is_some_and(|t| t.contains("Nested arrays live in object 6")),
            "the page text was not read: {text:?}"
        );
    }

    /// A Form XObject whose own `/Resources` bind `/F1` to another font than
    /// the page's `/F1` — legal, a resource name is scoped to its dictionary
    /// (`make_fixtures.py`'s p02, the page's text first, and p03, `/X1 Do`
    /// first). The page's `/F1` is Helvetica, so `(ABC page text)` reads as
    /// written; the form's is a Times-Roman whose `/Differences [65 /Z /Y /X]`
    /// make its `(ABC form text)` read `ZYX form text`. pdf-extract 0.8.2 read
    /// both right. 0.12 keeps one font cache per page keyed by resource name
    /// alone and `Do` recurses into the same processor, so whichever `/F1` is
    /// met first decodes both texts: `ABC form text` in p02, `ZYX page text`
    /// in p03 (`reports/documents.md` §3.3; DU-8 (a): accepted for the
    /// RUSTSEC-2026-0187 fix, no release has one without the other). The
    /// expectation is the correct text; the test runs when a release fixes it.
    #[test]
    #[ignore = "pdf-extract 0.12 regression (DU-8): a Form XObject's own /F1 is decoded with \
                the page's font (0.12.1's per-page cache reset, #152, leaves the within-page \
                case); no upstream issue: filing it with these two fixtures is the owner's \
                (programme-plan §5 item 14, pending)"]
    fn a_form_xobject_reads_its_text_with_its_own_fonts() {
        const TIMES_ZYX: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman \
            /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding \
            /Differences [65 /Z /Y /X] >> >>";
        let cases: [(&str, &[u8]); 2] = [
            (
                "p02, page text first",
                b"BT /F1 12 Tf 72 700 Td (ABC page text) Tj ET\n/X1 Do",
            ),
            (
                "p03, form first",
                b"/X1 Do\nBT /F1 12 Tf 72 700 Td (ABC page text) Tj ET",
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        let mut misread = Vec::new();
        for (case, content) in cases {
            let pdf = build_pdf(&[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> \
                  /XObject << /X1 7 0 R >> >> /Contents 4 0 R >>"
                    .to_vec(),
                pdf_stream(b"", content),
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                    .to_vec(),
                TIMES_ZYX.to_vec(),
                pdf_stream(
                    b"/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                      /Resources << /Font << /F1 6 0 R >> >> ",
                    b"BT /F1 12 Tf 72 600 Td (ABC form text) Tj ET",
                ),
            ]);
            let path = dir.path().join("form.pdf");
            std::fs::write(&path, pdf).unwrap();
            let (text, error) = extract_text(path.to_str().unwrap(), "application/pdf", 10000);
            assert_eq!(error, None, "{case}");
            let text = text.unwrap_or_default();
            if !(text.contains("ABC page text") && text.contains("ZYX form text")) {
                misread.push(format!("{case}: {text:?}"));
            }
        }
        assert!(
            misread.is_empty(),
            "a text was not read with its own /F1: {misread:#?}"
        );
    }
}
