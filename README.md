# streamx-torrent-indexer

Modular, config-driven torrent indexer engine for
[StreamX Ultra](https://github.com/AeonCoreX-Lab/streamx-ultra). Every
site is a plain JSON file, not code — adding a source is a PR, not a
release.

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
  src/generic_html.rs    generic CSS-selector-driven scraper
  src/generic_json.rs    generic JSON-API-driven scraper
  src/special/           bespoke scrapers for sites too irregular for the generic engine
  build.rs               auto-discovers sources/**/*.json at compile time — no manifest to edit

schema/source.schema.json   JSON Schema for one source file (editor autocomplete + CI validation)

sources/
  verified/*.json     sites maintained/blessed by AeonCoreX-Lab — ship in every release
  community/*.json     sites contributed by anyone — fully functional once CI passes
  special-sites.json   mirror lists for the crate::special sites (kdrama, nyaa, tokyotosho, eztvco)

tools/
  validator/          CI tool — schema check + live selector/domain smoke-test
  jackett-sync/        scheduled tool — syncs mirrors against upstream Jackett YAML automatically
  cli/                  misc maintenance CLI (assembles dist/registry.json for hosting)

.github/workflows/
  validate-sources.yml   runs on every PR touching sources/
  jackett-sync.yml        runs daily, auto-opens a PR on domain drift
  release-registry.yml    runs daily, auto-cuts a date-based GitHub Release when sources/ data actually changed

docs/
  CONTRIBUTING.md       how to add a source (JSON, not YAML)
  CONSUMING.md           how StreamX Ultra (or anything else) pulls this crate in
```

## Contributing a source

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). Short version: copy
an existing file in `sources/verified/`, adjust the selectors for your
site, run `cargo run -- check --live --only <your-id>` from
`tools/validator/`, open a PR.

## Using this from another app

See [`docs/CONSUMING.md`](docs/CONSUMING.md) for the embedded vs.
hosted-fetch-with-fallback pattern.

## License

MIT.
