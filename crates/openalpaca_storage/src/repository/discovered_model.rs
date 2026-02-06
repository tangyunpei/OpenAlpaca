use crate::models::DiscoveredModel;
use crate::Database;
use anyhow::{Context, Result};

/// Repository for discovered model cache (discovered_models table).
pub struct DiscoveredModelRepository<'a> {
    db: &'a Database,
}

impl<'a> DiscoveredModelRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// List all cached discovered models.
    pub fn list(&self) -> Result<Vec<DiscoveredModel>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT model_id, provider, input_price_per_million, output_price_per_million, context_window
                 FROM discovered_models
                 ORDER BY provider ASC, model_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(DiscoveredModel {
                    model_id: row.get(0)?,
                    provider: row.get(1)?,
                    input_price_per_million: row.get(2)?,
                    output_price_per_million: row.get(3)?,
                    context_window: row.get::<_, i32>(4)? as u32,
                })
            })?;

            let mut models = Vec::new();
            for row in rows {
                models.push(row.context("Failed to read discovered model row")?);
            }
            Ok(models)
        })
    }

    /// List cached models for a specific provider.
    pub fn list_by_provider(&self, provider: &str) -> Result<Vec<DiscoveredModel>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT model_id, provider, input_price_per_million, output_price_per_million, context_window
                 FROM discovered_models
                 WHERE provider = ?
                 ORDER BY model_id ASC",
            )?;
            let rows = stmt.query_map([provider], |row| {
                Ok(DiscoveredModel {
                    model_id: row.get(0)?,
                    provider: row.get(1)?,
                    input_price_per_million: row.get(2)?,
                    output_price_per_million: row.get(3)?,
                    context_window: row.get::<_, i32>(4)? as u32,
                })
            })?;

            let mut models = Vec::new();
            for row in rows {
                models.push(row.context("Failed to read discovered model row")?);
            }
            Ok(models)
        })
    }

    /// Replace all models for a provider with a new list.
    /// Only call this when the provider returned a non-empty list.
    pub fn replace_provider_models(&self, provider: &str, models: &[DiscoveredModel]) -> Result<()> {
        self.db.with_connection(|conn| {
            // Delete existing models for this provider
            conn.execute(
                "DELETE FROM discovered_models WHERE provider = ?",
                [provider],
            )
            .context("Failed to delete old models for provider")?;

            // Insert new models
            let mut stmt = conn.prepare(
                "INSERT INTO discovered_models (model_id, provider, input_price_per_million, output_price_per_million, context_window, discovered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            )?;

            for model in models {
                stmt.execute((
                    &model.model_id,
                    &model.provider,
                    model.input_price_per_million,
                    model.output_price_per_million,
                    model.context_window as i32,
                ))
                .context("Failed to insert discovered model")?;
            }

            Ok(())
        })
    }

    /// Clear all cached models.
    pub fn clear_all(&self) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM discovered_models", [])
                .context("Failed to clear discovered models")?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_list_empty() {
        let db = test_db();
        let repo = DiscoveredModelRepository::new(&db);
        let models = repo.list().unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_replace_and_list() {
        let db = test_db();
        let repo = DiscoveredModelRepository::new(&db);

        let models = vec![
            DiscoveredModel {
                model_id: "claude-sonnet".to_string(),
                provider: "anthropic".to_string(),
                input_price_per_million: 3.0,
                output_price_per_million: 15.0,
                context_window: 200_000,
            },
            DiscoveredModel {
                model_id: "claude-haiku".to_string(),
                provider: "anthropic".to_string(),
                input_price_per_million: 0.8,
                output_price_per_million: 4.0,
                context_window: 200_000,
            },
        ];

        repo.replace_provider_models("anthropic", &models).unwrap();

        let result = repo.list().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].model_id, "claude-haiku");
        assert_eq!(result[1].model_id, "claude-sonnet");
    }

    #[test]
    fn test_replace_does_not_affect_other_providers() {
        let db = test_db();
        let repo = DiscoveredModelRepository::new(&db);

        let anthropic = vec![DiscoveredModel {
            model_id: "claude-sonnet".to_string(),
            provider: "anthropic".to_string(),
            input_price_per_million: 3.0,
            output_price_per_million: 15.0,
            context_window: 200_000,
        }];
        let openai = vec![DiscoveredModel {
            model_id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            input_price_per_million: 2.5,
            output_price_per_million: 10.0,
            context_window: 128_000,
        }];

        repo.replace_provider_models("anthropic", &anthropic).unwrap();
        repo.replace_provider_models("openai", &openai).unwrap();

        // Replace anthropic with new list — openai should stay
        let new_anthropic = vec![DiscoveredModel {
            model_id: "claude-opus".to_string(),
            provider: "anthropic".to_string(),
            input_price_per_million: 15.0,
            output_price_per_million: 75.0,
            context_window: 200_000,
        }];
        repo.replace_provider_models("anthropic", &new_anthropic).unwrap();

        let all = repo.list().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].model_id, "claude-opus");
        assert_eq!(all[1].model_id, "gpt-4o");
    }

    #[test]
    fn test_empty_replace_not_called_preserves_cache() {
        let db = test_db();
        let repo = DiscoveredModelRepository::new(&db);

        let models = vec![DiscoveredModel {
            model_id: "claude-sonnet".to_string(),
            provider: "anthropic".to_string(),
            input_price_per_million: 3.0,
            output_price_per_million: 15.0,
            context_window: 200_000,
        }];
        repo.replace_provider_models("anthropic", &models).unwrap();

        // Simulate: API returns empty — we simply don't call replace_provider_models
        // Verify existing data is preserved
        let result = repo.list_by_provider("anthropic").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].model_id, "claude-sonnet");
    }

    #[test]
    fn test_clear_all() {
        let db = test_db();
        let repo = DiscoveredModelRepository::new(&db);

        let models = vec![DiscoveredModel {
            model_id: "model-a".to_string(),
            provider: "anthropic".to_string(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
        }];
        repo.replace_provider_models("anthropic", &models).unwrap();
        assert_eq!(repo.list().unwrap().len(), 1);

        repo.clear_all().unwrap();
        assert!(repo.list().unwrap().is_empty());
    }
}
