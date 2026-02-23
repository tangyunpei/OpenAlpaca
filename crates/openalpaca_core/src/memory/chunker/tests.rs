use super::*;

#[test]
fn test_chunk_headings() {
    let md = "## Introduction\nHello world.\n\n## Details\nSome details here.";
    let chunks = chunk_markdown(md, "test.md", 500);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].section.as_deref(), Some("## Introduction"));
    assert!(chunks[0].content.contains("Hello world"));
    assert_eq!(chunks[1].section.as_deref(), Some("## Details"));
    assert!(chunks[1].content.contains("Some details"));
}

#[test]
fn test_chunk_long_section() {
    let long_paragraph = "A".repeat(600);
    let md = format!("## Big Section\n{}", long_paragraph);
    let chunks = chunk_markdown(&md, "test.md", 500);
    assert!(
        chunks.len() >= 2,
        "Long section should be split: got {} chunks",
        chunks.len()
    );
}

#[test]
fn test_chunk_metadata() {
    let md = "## Section One\nContent here.\n\n### Sub Section\nMore content.";
    let chunks = chunk_markdown(md, "docs/readme.md", 500);
    for chunk in &chunks {
        assert_eq!(chunk.source_file, "docs/readme.md");
    }
    assert!(
        chunks
            .iter()
            .any(|c| c.section.as_deref() == Some("## Section One"))
    );
}

#[test]
fn test_empty_content() {
    let chunks = chunk_markdown("", "empty.md", 500);
    assert!(chunks.is_empty());
}

#[test]
fn test_no_headings() {
    let md = "Just some plain text without any headings.\n\nAnother paragraph.";
    let chunks = chunk_markdown(md, "plain.md", 500);
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].section.is_none());
}
