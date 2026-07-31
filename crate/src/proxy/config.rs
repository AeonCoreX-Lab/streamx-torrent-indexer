// src/indexer/proxy/config.rs
//
// Proxy configuration types. Modeled on Prowlarr's IndexerProxies
// design (NzbDrone.Core/IndexerProxies/{Http,Socks4,Socks5}) — verified
// against Prowlarr's actual source: each proxy type there is just a
// host/port/username/password bundle that gets applied to the
// ClientBuilder's HttpProxySettings before a request. Prowlarr's fourth
// proxy type, FlareSolverr, is NOT ported here — it requires an
// externally-running browser-automation server (a separate Docker
// container), which cannot run inside an Android app. See the
// FlareSolverr research notes elsewhere in this project for the full
// reasoning; Prowlarr's own FlareSolverr.cs confirms it is purely an
// HTTP client that POSTs to that external server, not an in-process
// bypass — there's nothing in it we could "port" that would work
// without the same external dependency.
//
// IMPORTANT — this is deliberately NOT part of indexer-config.json
// (the GitHub-hosted remote config). Proxy credentials are per-user
// secrets (their own VPN/proxy service login), never shared config —
// they're supplied by the user via Settings, stored encrypted on-device
// (see IndexerProxyNative.kt for the Kotlin/EncryptedSharedPreferences
// side), and passed into Rust only in-memory per session.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKind {
    Http,
    Socks4,
    Socks5,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// If true, apply this proxy to every indexer request. If false,
    /// the proxy is configured but not currently active — mirrors
    /// Prowlarr's per-definition proxy toggle, except here it's a
    /// single global on/off since we don't have Prowlarr's concept of
    /// multiple named proxies assigned per-indexer (out of scope for
    /// v1 — see the module doc in mod.rs for the simplification
    /// rationale).
    #[serde(default)]
    pub enabled: bool,
}

impl ProxyConfig {
    /// Builds the URL scheme string reqwest::Proxy::all() expects,
    /// e.g. "socks5://host:port" or "http://host:port". Credentials are
    /// NOT embedded in this URL — they're applied separately via
    /// `.basic_auth()` in client.rs, matching Prowlarr's own separation
    /// of "proxy address" from "proxy auth" (see Http.cs/Socks5.cs,
    /// which both build the HttpProxySettings with host/port and
    /// username/password as distinct fields, never concatenated into
    /// a single userinfo-style URL).
    pub fn proxy_url(&self) -> String {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Socks4 => "socks4",
            ProxyKind::Socks5 => "socks5",
        };
        format!("{scheme}://{}:{}", self.host, self.port)
    }

    pub fn has_auth(&self) -> bool {
        self.username.as_deref().is_some_and(|u| !u.is_empty())
    }
}
