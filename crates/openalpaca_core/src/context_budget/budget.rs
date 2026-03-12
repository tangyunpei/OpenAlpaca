/// A rendered prompt section with its estimated token count.
#[derive(Debug, Clone)]
pub struct RenderedSection {
    pub content: String,
    pub token_estimate: usize,
}

impl RenderedSection {
    /// Create a new section, estimating tokens as `bytes / 4`.
    pub fn new(content: String) -> Self {
        let token_estimate = content.len() / 4;
        Self {
            content,
            token_estimate,
        }
    }

    /// Create with an explicit token estimate (e.g., from API response).
    pub fn with_token_estimate(content: String, token_estimate: usize) -> Self {
        Self {
            content,
            token_estimate,
        }
    }

    /// Empty section (zero tokens).
    pub fn empty() -> Self {
        Self {
            content: String::new(),
            token_estimate: 0,
        }
    }
}
