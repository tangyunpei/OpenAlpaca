use tracing::Span;

/// Create a span for a top-level subsystem.
pub fn subsystem_span(name: &str) -> Span {
    tracing::info_span!("subsystem", name = name)
}

/// Create a span for a channel subsystem.
pub fn channel_span(channel: &str, account: &str) -> Span {
    tracing::info_span!("channel", channel = channel, account = account)
}

/// Create a span for the gateway subsystem.
pub fn gateway_span() -> Span {
    tracing::info_span!("gateway")
}

/// Create a span for an agent run.
pub fn agent_span(agent_id: &str, session_key: &str) -> Span {
    tracing::info_span!("agent", agent_id = agent_id, session_key = session_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_subscriber<F: FnOnce()>(f: F) {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
    }

    #[test]
    fn test_subsystem_span_created() {
        with_subscriber(|| {
            let span = subsystem_span("config");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_channel_span_fields() {
        with_subscriber(|| {
            let span = channel_span("telegram", "bot1");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_gateway_span() {
        with_subscriber(|| {
            let span = gateway_span();
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_agent_span() {
        with_subscriber(|| {
            let span = agent_span("assistant", "session-123");
            assert!(!span.is_disabled());
        });
    }
}
