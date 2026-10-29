//! The process's one rustls crypto provider: aws-lc-rs (P41).
//!
//! Every TLS connection the daemon opens goes through rustls 0.23 — reqwest
//! 0.13's clients (the LLM providers, MCP's streamable-HTTP transport,
//! `web_fetch`) and twilight's HTTP client and gateway socket (Discord).
//! reqwest builds with the process default when one is installed and with
//! aws-lc-rs otherwise; twilight-http and tokio-websockets call
//! `rustls::ClientConfig::builder()`, which takes the process default or,
//! with none installed, the one provider rustls's crate features name — and
//! panics when they name two. Installed here, before anything builds a
//! client, aws-lc-rs is the provider of every connection whatever the
//! dependency graph turns on: teloxide 0.17 over rustls (P28) enables
//! `rustls/ring` through reqwest 0.12, which also takes the process default
//! when one is installed.
//!
//! A library installs no provider; the binary does. Until P41 the Discord
//! connector installed ring when its loop started, so a twilight client built
//! before that — `connector_bridge`'s, for a stored token while the connector
//! is off — panicked, and every reqwest client built after it ran on ring.

use tracing::warn;

/// Install aws-lc-rs as the process's rustls crypto provider.
///
/// `main` calls this once, right after the logger and before anything builds
/// a TLS client; a test that builds one calls it the same way. rustls keeps
/// the first provider installed and has no way to replace it, so a provider
/// already there stays, and the warning names it (`AwsLcRs` or `Ring`) — in
/// the daemon nothing installs one before `main` does.
pub(crate) fn install_crypto_provider() {
    // `install_default` hands back the provider it was given, never the one
    // already installed; that one is `get_default()`'s.
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        let installed = rustls::crypto::CryptoProvider::get_default().map(|p| p.secure_random);
        warn!(
            "rustls: a crypto provider ({installed:?}) was already installed; it stays the process default"
        );
    }
}

#[cfg(test)]
mod tests {
    use rustls::crypto::{CryptoProvider, aws_lc_rs};

    /// After the install the process default is aws-lc-rs as
    /// `default_provider()` configures it — `CryptoProvider` has no identity
    /// of its own, and its `Debug` names every cipher suite, key-exchange
    /// group and implementation it holds — and a second install leaves it so.
    /// The assertion reads `get_default()`, not the install's own result:
    /// another test in this binary may have installed it first.
    #[test]
    fn the_process_default_is_aws_lc_rs() {
        super::install_crypto_provider();
        super::install_crypto_provider();

        let installed = CryptoProvider::get_default().expect("a process default");
        assert_eq!(
            format!("{installed:?}"),
            format!("{:?}", aws_lc_rs::default_provider())
        );
    }
}
