use super::*;
use crate::test_util::test_db;

#[test]
fn test_agent_crud() {
    let db = test_db();
    let repo = AgentRepository::new(&db);

    let agent = Agent {
        id: "test-1".to_string(),
        name: "Test Agent".to_string(),
        persona: Some("A helpful assistant".to_string()),
        config: None,
        created_at: Utc::now(),
    };

    // Create
    repo.create(&agent).unwrap();

    // Read
    let fetched = repo.get("test-1").unwrap().unwrap();
    assert_eq!(fetched.name, "Test Agent");

    // List
    let list = repo.list().unwrap();
    assert_eq!(list.len(), 1);

    // Delete
    repo.delete("test-1").unwrap();
    assert!(repo.get("test-1").unwrap().is_none());
}
