# Consuming this from StreamX Ultra

This repo produces a plain Rust library crate (`streamx-indexer`) — no
Android/JNI/cdylib artifacts live here. StreamX Ultra's own
`app/src/main/rust` depends on it like any other crate and keeps
owning the JNI boundary, the Android cache directory, and the
network-fetch-with-fallback policy for `dist/registry.json`. This
split is deliberate: this repo doesn't know it's being used by an
Android app, which is what lets `tools/validator` and
`tools/jackett-sync` run as plain CI jobs with no Android SDK involved.

## Two ways to get a registry at runtime

### 1. Embedded (zero network, always works)

```rust
let registry = streamx_indexer::registry::load_embedded();
```

Everything under `sources/verified/` and `sources/community/` at
**build time** is baked in via `include_str!` (see `crate/build.rs`).
This is what ships in the APK and guarantees the app works offline and
on a fresh install with no first-run fetch required. Its downside is
obvious: it's frozen at whatever commit the app was built from.

### 2. Hosted fetch with fallback (recommended — this is what gets you
   no-APK-update fixes)

`.github/workflows/release-registry.yml` runs daily and, only when the
actual site data changed since the last release (not on a no-op day),
cuts a date-based GitHub Release (`v2026.07.31`, or `v2026.07.31.1` for
a same-day second release) with a built `registry.json` attached. You
don't have to track version numbers on the app side — GitHub always
redirects the "latest" alias to whichever release is newest:

```
https://github.com/AeonCoreX-Lab/streamx-torrent-indexer/releases/latest/download/registry.json
```

```rust
const HOSTED_REGISTRY_URL: &str =
    "https://github.com/AeonCoreX-Lab/streamx-torrent-indexer/releases/latest/download/registry.json";

async fn load_registry(client: &reqwest::Client, cache_dir: &Path) -> IndexerRegistry {
    match fetch_and_cache(client, cache_dir, HOSTED_REGISTRY_URL).await {
        Ok(raw) => match streamx_indexer::registry::load_from_json(&raw) {
            Ok(reg) => return reg,
            Err(e) => log::warn!("hosted registry.json failed to parse: {e}, falling back"),
        },
        Err(e) => log::warn!("hosted registry.json fetch failed: {e}, falling back"),
    }
    // Try the last-known-good cached copy before giving up entirely.
    if let Some(cached) = read_cache(cache_dir) {
        if let Ok(reg) = streamx_indexer::registry::load_from_json(&cached) {
            return reg;
        }
    }
    streamx_indexer::registry::load_embedded()
}
```

This three-tier fallback (hosted → last-known-good cache → embedded)
is exactly the pattern the app's original `indexer/config/loader.rs`
already used for the single `indexer-config.default.json` file — the
only change is that the hosted JSON now comes from an automated GitHub
Release instead of being hand-maintained inside the app repo, so a
community-contributed source or a `jackett-sync` domain fix reaches
users the next time this daily job runs, not their next Play Store
update.

Check `registry.updated` (the date-based version string, e.g.
`"2026.07.31"`) if you want to log or display which registry version
the app is currently running — it's informational only, nothing in the
crate depends on comparing it.

## Calling the engine

Once you have a registry, everything else is unchanged from before the
split — `engine.rs`'s public functions now just take the registry as an
explicit parameter instead of reading a global:

```rust
let client = reqwest::Client::new();
let results = streamx_indexer::engine::search_dubbed(&client, &registry, &query, imdb_id.as_deref()).await;
```

See `crate/src/engine.rs` for the full function list
(`search_all`, `search_drama`, `search_anime_english`,
`search_anime_other_dub`, etc.) — the signatures and behavior are a
direct port of the app's original `indexer/engine.rs`, just registry-
parameterized.

## Version pinning

Until this crate is published anywhere, pin it as a git dependency in
StreamX Ultra's `app/src/main/rust/Cargo.toml`:

```toml
[dependencies]
streamx-indexer = { git = "https://github.com/AeonCoreX-Lab/streamx-torrent-indexer", branch = "main" }
```

Pin to a commit or tag instead of `branch = "main"` for release builds,
so a mid-cycle merge here can't change what a release build produces
without an explicit bump.
