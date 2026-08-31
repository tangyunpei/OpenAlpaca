use crate::middleware::user::{UserDocument, user_to_prompt_block};
use crate::prompt_ctx::section::{
    ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};

pub struct UserProfileSource {
    user_document: Arc<RwLock<Option<UserDocument>>>,
}

impl UserProfileSource {
    pub fn new(user_document: Arc<RwLock<Option<UserDocument>>>) -> Self {
        Self { user_document }
    }
}

#[async_trait]
impl ContextSource for UserProfileSource {
    fn name(&self) -> &'static str {
        "user_profile"
    }

    async fn resolve(&self, _request: &ContextRequest) -> Vec<ContextSection> {
        let content = match self.user_document.read() {
            Ok(guard) => match guard.as_ref() {
                Some(doc) => {
                    let block = user_to_prompt_block(doc, None);
                    if block.is_empty() {
                        return vec![];
                    }
                    block
                }
                None => return vec![],
            },
            Err(poisoned) => {
                tracing::warn!("UserProfileSource: user_document lock poisoned; recovering");
                match poisoned.into_inner().as_ref() {
                    Some(doc) => {
                        let block = user_to_prompt_block(doc, None);
                        if block.is_empty() {
                            return vec![];
                        }
                        block
                    }
                    None => return vec![],
                }
            }
        };

        let token_estimate = content.len() / 4;
        vec![ContextSection {
            source: "user_profile",
            kind: ContextKind::UserProfile,
            content,
            token_estimate,
            priority: SectionPriority::Normal,
            relevance: 1.0,
            key: ContextKey::UserProfile,
            injection: InjectionMode::SystemMessage,
        }]
    }

}
