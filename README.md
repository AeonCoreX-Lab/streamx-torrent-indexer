# streamx-torrent-indexer

Modular, config-driven torrent indexer engine for
[StreamX Ultra](https://github.com/AeonCoreX-Lab/streamx-ultra). Every
site is a plain JSON file, not code — adding a source is a PR, not a
release. Supports both anonymous public sites and private trackers
that require a per-user login (see "Private trackers" below).

## Why this exists

StreamX Ultra's torrent indexer used to be config-driven already
(`indexer-config.default.json` bundled inside the app repo), but that
setup had three problems this repo fixes:

1. **Not contributable.** The config lived inside the main app repo, so
   adding a source meant a PR against the whole Android app. This repo
   is scoped to just indexer definitions — a much smaller, much safer
   surface for outside contributors to touch.
2. **Not self-verifying.** A dead mirror or a redesigned site's
   selectors just... silently returned zero results in production,
   with nobody finding out until a user complained. `tools/validator`
   now runs a live smoke-test against every source's actual selectors
   in CI.
3. **Not synced with upstream.** Most of these site definitions started
   as ports of [Jackett](https://github.com/Jackett/Jackett) Cardigann
   YAML files, and Jackett's own maintainers are constantly chasing
   domain changes across hundreds of trackers. Re-doing that work by
   hand, forever, is wasted effort — `tools/jackett-sync` runs daily,
   diffs our `mirrors[]` against Jackett's current `links:` for the
   same site, and opens a PR automatically when they drift. If Jackett
   removes an indexer entirely (like it did with `torrentqq.yml`), the
   affected source gets flagged (`origin.upstream_removed: true`)
   instead of silently going stale.
4. **Not automatically released.** A merged source fix used to just...
   sit in the repo until someone remembered to cut a release.
   `tools/cli`'s `build-registry` assembles `sources/` into a single
   `dist/registry.json`, and `.github/workflows/release-registry.yml`
   runs daily, cutting a date-based GitHub Release
   (`v2026.07.31`, or `v2026.07.31.1` for a same-day second change)
   with that file attached — but only on a day the underlying site
   data actually changed, so there's no version-number noise on a
   no-op day. StreamX Ultra fetches the "latest release" URL at
   runtime (see `docs/CONSUMING.md`), so any merge — a community PR, a
   jackett-sync fix, a manual edit — reaches users on their next app
   launch, with nobody having to remember to publish anything.

## Structure

```
crate/               Rust library (streamx-indexer) — the actual scraping engine
  src/schema.rs         canonical types — the JSON shape is generated FROM these
  src/registry.rs       merges sources/ into one IndexerRegistry (embedded or runtime-fetched)
  src/engine.rs          search_all / search_dubbed / search_drama / search_anime_* etc.
  src/generic_html.rs    generic CSS-selector-driven scraper (GET or POST search, magnet or .torrent-file results)
  src/generic_json.rs    generic JSON-API-driven scraper (GET or POST search)
  src/special/           bespoke scrapers for sites too irregular for the generic engine
  build.rs               auto-discovers sources/**/*.json at compile time — no manifest to edit

schema/source.schema.json   JSON Schema for one source file (editor autocomplete + CI validation)

sources/
  verified/*.json     sites maintained/blessed by AeonCoreX-Lab — ship in every release
  community/*.json     sites contributed by anyone — fully functional once CI passes
  private/*.json        sites that require a per-user login (private trackers) — see "Private trackers" below
  special-sites.json   mirror lists for the crate::special sites (kdrama, nyaa, tokyotosho, eztvco)

tools/
  validator/          CI tool — schema check + live selector/domain smoke-test (GET and POST search both supported)
  jackett-sync/        scheduled tool — syncs mirrors against upstream Jackett YAML automatically
  cli/                  misc maintenance CLI (assembles dist/registry.json for hosting)

.github/workflows/
  validate-sources.yml   runs on every PR touching sources/
  jackett-sync.yml        runs daily, auto-opens a PR on domain drift
  release-registry.yml    runs daily, auto-cuts a date-based GitHub Release when sources/ data actually changed

docs/
  CONTRIBUTING.md       how to add a source (JSON, not YAML) — public sites and private trackers both
  CONSUMING.md           how StreamX Ultra (or anything else) pulls this crate in
```

## Capabilities

- **Two response formats**: `kind: "html"` (CSS-selector scraping) or
  `kind: "json"` (JSON-API sites), same as before.
- **Two search HTTP methods**: `search_method: "get"` (default, query
  string built from `search_path`) or `"post"` (form-encoded body from
  `search_body`) — added to support sites (mostly private trackers)
  whose search endpoint only accepts a POST, like Cardigann's own
  `method: post` search blocks.
- **Two result-download shapes**: a real `magnet:` URI (the default,
  every public source), or `download_type: "torrent_file"` for sites
  that only expose an authenticated `.torrent` file link on their
  listing page instead — the norm for private trackers, since that
  download hit is how they track ratio/membership. See
  `TorrentResult::torrent_file_url` / `requires_torrent_auth` in
  `crate/src/types.rs`; the consuming app is responsible for fetching
  that URL with the site's cookie attached and handing the resulting
  bytes to its torrent client instead of a magnet.
- **Optional per-site cookie auth** (`request.auth`, `method: "cookie"`)
  for private trackers — the schema only ever describes *that* a site
  needs a cookie and *how to check* one still works
  (`login_check_path`/`login_check_selector`); the actual per-user
  cookie value is supplied by the consuming app at search time via
  `AuthProvider` (see `crate/src/dispatch.rs`) and is never stored in
  this repo. How the app itself obtains that cookie (e.g. an in-app
  WebView login) is entirely up to the app — this crate has no opinion
  on it.

## Private trackers

`sources/private/` holds sites whose `request.auth` is set — same
merge/validation rules as `verified/`/`community/`, kept in a separate
folder purely so it's obvious at a glance which sites need a login
before they'll return results. A private tracker with no cookie
supplied for it still gets searched like any other site, just without
a `Cookie` header attached — this normally comes back as zero results
(or a login-page response) rather than an error, so one missing cookie
never breaks the overall search. Public sites in the same search are
completely unaffected either way — see `dispatch.rs`'s
`requires_auth()` check, which only ever looks up a cookie for sites
that actually declared they need one.

Currently ported: **HD-Torrents**, **MySpleen**, **TorrentBD** (the
latter exercising `search_method: "post"`) — all three ported directly
from Jackett's own definitions, which is the recommended way to add
another. See
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md#1-porting-from-jackett-recommended)
for how.

## Contributing a source

**Before writing a new source from scratch, check
[Jackett's own indexer definitions](https://github.com/Jackett/Jackett/tree/master/src/Jackett.Common/Definitions)
first.** Jackett ships ~550+ definitions covering most public trackers
and a large number of private ones — the overwhelming majority of any
site you'd want to add already has a working Cardigann YAML definition
sitting there, maintained by Jackett's own contributors. **Porting one
of those is the recommended, default way to add a source here** — it's
faster, and the selectors/login flow have already been fought over and
proven against the real site by someone else.

Only if a site genuinely has **no** Jackett definition (rare) should
you analyze the site yourself and write selectors from scratch — and
even then, you must still follow this repo's schema exactly
(`schema/source.schema.json` — CI rejects anything that doesn't) and
match the shape of an already-ported source in `sources/verified/` or
`sources/private/` as your reference, rather than invent a different
structure. See
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md#1-porting-from-jackett-recommended)
for the full walkthrough of both paths.

Short version either way: copy an existing file in `sources/verified/`
(or `sources/private/` if the site needs a login), adjust the
selectors for your site, run `cargo run -- check --live --only
<your-id>` from `tools/validator/`, open a PR.

## Using this from another app

See [`docs/CONSUMING.md`](docs/CONSUMING.md) for the embedded vs.
hosted-fetch-with-fallback pattern.

## License

MIT.
