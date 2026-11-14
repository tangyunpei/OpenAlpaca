use super::*;

/// TEMPLATE-TEMP-OPTIONAL (item 24 of the owner's answer of 2026-11-14):
/// the creation screen no longer asks for a temperature, which nothing
/// sends to a model, so the body it posts carries none — at any depth, the
/// cost constraint or not.
#[test]
fn the_screens_create_body_carries_no_temperature() {
    let skills = vec!["research".to_string()];
    for max_cost in [0.0, 2.5] {
        let body = create_agent_body(
            "Reader",
            "Reads things",
            "some-model",
            "You read.",
            &skills,
            max_cost,
        );
        assert!(!body.to_string().contains("temperature"), "{body}");
        assert_eq!(body["config"]["name"], "Reader");
        assert_eq!(body["config"]["persona"], "You read.");
    }
}
