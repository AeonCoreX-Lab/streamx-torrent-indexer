// src/indexer/proxy/mod.rs
//
// Coordinates the app's single, optional, user-configured proxy for
// indexer HTTP requests. Deliberately simplified vs Prowlarr's model:
// Prowlarr lets a user define multiple named proxies and assign a
// different one per-indexer; we support exactly one active proxy
// applied globally to all indexer requests. This matches the actual
// use case here — a user routing their OWN traffic through their OWN
// VPN/proxy to reach a site blocked in their region/network, not
// operating a multi-tenant indexer farm like Prowlarr's server
// deployments — so the added complexity of per-site proxy assignment
// wasn't worth it for v1. If that need comes up later, ProxyConfig and
// the state below would need to become a per-site_id map instead of a
// single Option.
//
// STORAGE: the actual ProxyConfig (host/port/credentials) is NEVER
// persisted by this Rust module — it lives only in memory for the
// process lifetime, set via set_proxy() (called from lib.rs's JNI
// bridge, which itself is called from Kotlin after reading the user's
// saved config out of EncryptedSharedPreferences on each app launch).
// This mirrors how indexer/config/loader.rs treats the remote site
// config — Rust holds a runtime cache, Kotlin/Android owns the
// at-rest storage — but unlike that config (which is not sensitive
// and copied into a bundled fallback asset), there is no bundled
// fallback for proxy credentials: no proxy configured simply means
// no proxy is used, which is the correct and safe default.

pub mod config;
pub mod client;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::sync::Arc;

use config::ProxyConfig;

struct ProxyState {
    config: Option<ProxyConfig>,
    client: Arc<reqwest::Client>,
}

static STATE: Lazy<RwLock<ProxyState>> = Lazy::new(|| {
    RwLock::new(ProxyState {
        config: None,
        client: Arc::new(client::build_plain_client()),
    })
});

/// Sets (or replaces) the active proxy and rebuilds the shared client
/// to use it. Returns an error if the config is invalid — the PREVIOUS
/// client (proxied or plain) remains active in that case, so a bad
/// proxy entry never silently breaks indexer search entirely.
pub fn set_proxy(new_config: ProxyConfig) -> anyhow::Result<()> {
    if !new_config.enabled {
        return clear_proxy();
    }
    let new_client = client::build_proxied_client(&new_config)?;
    let mut state = STATE.write();
    log::info!(
        "[proxy] activating {} proxy {}:{}",
        client::kind_label(new_config.kind), new_config.host, new_config.port
    );
    state.client = Arc::new(new_client);
    state.config = Some(new_config);
    Ok(())
}

/// Disables the proxy — subsequent requests use a plain (direct)
/// client again. Always succeeds.
pub fn clear_proxy() -> anyhow::Result<()> {
    let mut state = STATE.write();
    log::info!("[proxy] cleared — indexer requests will connect directly");
    state.client = Arc::new(client::build_plain_client());
    state.config = None;
    Ok(())
}

/// The client every indexer site module should use. Returns the
/// proxied client if one is active, otherwise a plain direct client —
/// callers never need to branch on whether a proxy is configured.
pub fn get_client() -> Arc<reqwest::Client> {
    STATE.read().client.clone()
}

/// Whether a proxy is currently active — exposed for a status
/// indicator in Settings UI if desired later.
pub fn is_active() -> bool {
    STATE.read().config.is_some()
}

/// Human-readable summary for a "Proxy: SOCKS5 12.34.56.78:1080"-style
/// status line — never includes credentials.
pub fn status_summary() -> String {
    match &STATE.read().config {
        Some(cfg) => format!(
            "{} {}:{}",
            client::kind_label(cfg.kind), cfg.host, cfg.port
        ),
        None => "Direct (no proxy)".to_string(),
    }
}
