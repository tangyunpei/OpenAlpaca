use super::budget::RenderedSection;

#[test]
fn test_rendered_section_creation() {
    let section = RenderedSection::new("Hello world".to_string());
    assert_eq!(section.content, "Hello world");
    assert_eq!(section.token_estimate, 2);
}

#[test]
fn test_rendered_section_empty() {
    let section = RenderedSection::new(String::new());
    assert_eq!(section.token_estimate, 0);
}

#[test]
fn test_rendered_section_with_explicit_tokens() {
    let section = RenderedSection::with_token_estimate("content".to_string(), 500);
    assert_eq!(section.token_estimate, 500);
    assert_eq!(section.content, "content");
}
