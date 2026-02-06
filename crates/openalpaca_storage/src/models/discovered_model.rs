use serde::{Deserialize, Serialize};

/// A model discovered from a provider API and cached in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub model_id: String,
    pub provider: String,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
    pub context_window: u32,
}
