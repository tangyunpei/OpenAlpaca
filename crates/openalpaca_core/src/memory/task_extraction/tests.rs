use super::*;

#[test]
fn test_parse_json_response_raw() {
    let input = r#"{"extractions": [{"content": "test", "kind": "fact"}]}"#;
    let result = parse_json_response(input);
    assert!(result.is_some());
    let arr = result.unwrap()["extractions"].as_array().unwrap().len();
    assert_eq!(arr, 1);
}

#[test]
fn test_parse_json_response_fenced() {
    let input = "```json\n{\"extractions\": []}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_plain_fence() {
    let input = "```\n{\"extractions\": [{\"content\": \"hello\"}]}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_invalid() {
    let input = "This is not JSON at all";
    assert!(parse_json_response(input).is_none());
}

#[test]
fn test_parse_json_response_with_whitespace() {
    let input = "  \n  {\"extractions\": []}  \n  ";
    let result = parse_json_response(input);
    assert!(result.is_some());
}
