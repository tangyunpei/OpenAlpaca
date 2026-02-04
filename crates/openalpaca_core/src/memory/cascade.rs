use crate::memory::preferences::PreferenceStore;

pub enum ScopeLevel {
    Conversation,
    Workspace,
    Global,
}

pub struct ResolutionTrace {
    pub key: String,
    pub final_value: serde_json::Value,
    pub winning_scope: String,
    pub why: String,
}

pub struct PreferenceCascade {
    // We might hold references to different stores or just global + config
    // For now, simplify logic
    #[allow(dead_code)]
    global_store: PreferenceStore,
    // Context is dynamic
}

impl PreferenceCascade {
    // TODO: Implement full cascade logic
}
