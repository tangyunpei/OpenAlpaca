use serde_yml::Value;

/// A config migration that transforms raw YAML values.
pub struct Migration {
    pub id: &'static str,
    pub description: &'static str,
    pub apply: fn(&mut Value, &mut Vec<String>),
}

/// Ordered list of all config migrations.
pub fn migrations() -> &'static [Migration] {
    &[
        // No migrations yet — Rust port starts fresh.
        // Future example:
        // Migration {
        //     id: "001-rename-foo",
        //     description: "Renamed foo.bar to foo.baz",
        //     apply: |raw, changes| { ... },
        // },
    ]
}

/// Apply all pending migrations to a raw config value.
/// Returns list of human-readable change descriptions. Empty if nothing changed.
pub fn apply_migrations(raw: &mut Value) -> Vec<String> {
    let mut changes = Vec::new();
    for migration in migrations() {
        (migration.apply)(raw, &mut changes);
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_migrations_empty() {
        let mut value = serde_yml::from_str::<Value>("gateway:\n  port: 3777").unwrap();
        let changes = apply_migrations(&mut value);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_migration_framework() {
        // Test that the migration mechanism works by manually running a migration
        let test_migration = Migration {
            id: "test-001",
            description: "Test migration",
            apply: |raw, changes| {
                if let Value::Mapping(map) = raw {
                    let key = Value::String("legacy_field".into());
                    if map.contains_key(&key) {
                        map.remove(&key);
                        changes.push("removed legacy_field".into());
                    }
                }
            },
        };

        let mut value =
            serde_yml::from_str::<Value>("legacy_field: true\ngateway:\n  port: 3777").unwrap();
        let mut changes = Vec::new();
        (test_migration.apply)(&mut value, &mut changes);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0], "removed legacy_field");

        // Verify the field was actually removed
        let map = value.as_mapping().unwrap();
        assert!(!map.contains_key(&Value::String("legacy_field".into())));
        assert!(map.contains_key(&Value::String("gateway".into())));
    }
}
