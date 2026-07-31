// src/indexer/proxy/client.rs
//
// Builds a reqwest::Client with the given ProxyConfig applied, or a
// plain client when no proxy is configured/enabled. This replaces
// engine.rs's single static HTTP client with a swappable one — see
// mod.rs's get_client() for how callers obtain whichever is current.

use std::time::Duration;
use super::config::{ProxyConfig, ProxyKind};

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Builds a client with no proxy — identical to engine.rs's previous
/// static HTTP client. Used whenever proxy is unset/disabled.
pub fn build_plain_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .expect("failed to build plain indexer HTTP client")
}

/// Builds a client that routes every request through the given proxy.
/// Returns an error (rather than panicking) if the proxy config is
/// malformed (bad host/port producing an unparsable URL) or if
/// building the underlying connector fails (e.g. a SOCKS scheme
/// requested without the `socks` reqwest feature — always enabled
/// here, but kept as an error path rather than an unwrap for safety).
pub fn build_proxied_client(config: &ProxyConfig) -> anyhow::Result<reqwest::Client> {
    let mut proxy = reqwest::Proxy::all(config.proxy_url())
        .map_err(|e| anyhow::anyhow!("invalid proxy address: {e}"))?;

    // Mirrors Prowlarr's separation of proxy address vs proxy auth
    // (HttpProxySettings takes host/port and username/password as
    // distinct fields, not a combined userinfo URL) — same reasoning
    // documented on ProxyConfig::proxy_url().
    if config.has_auth() {
        let user = config.username.as_deref().unwrap_or_default();
        let pass = config.password.as_deref().unwrap_or_default();
        proxy = proxy.basic_auth(user, pass);
    }

    reqwest::Client::builder()
        .timeout(Duration::from_secs(20)) // proxied requests get a little more slack
        .user_agent(USER_AGENT)
        .proxy(proxy)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build proxied client: {e}"))
}

/// Convenience used by tests/diagnostics — human-readable label for
/// logging which proxy kind is active, without leaking host/port/creds
/// into logs.
pub fn kind_label(kind: ProxyKind) -> &'static str {
    match kind {
        ProxyKind::Http => "HTTP",
        ProxyKind::Socks4 => "SOCKS4",
        ProxyKind::Socks5 => "SOCKS5",
    }
}
