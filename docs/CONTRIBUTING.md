# Contributing a source

StreamX Ultra's indexer is Rust-based and does **not** support Jackett's
YAML (Cardigann) format directly. Instead, every site is a plain JSON
file matching `schema/source.schema.json`. If you've written a Jackett
YAML definition before, this will feel familiar — it's the same ideas
(selectors, mirrors, field mapping), just JSON instead of YAML, and a
flatter shape.

You do **not** need to touch any Rust code to add a source. Drop a JSON
file in the right folder, open a PR, and CI does the rest.

## 1. Pick a starting point

**If the site already has a Jackett YAML definition** (check
[Jackett's Definitions folder](https://github.com/Jackett/Jackett/tree/master/src/Jackett.Common/Definitions)),
port it — this is the common case and the fastest path:

- `id:` → `id` (keep it identical to Jackett's own id where possible —
  this is what lets `jackett-sync` track domain changes for you
  automatically forever after)
- `links:` → `mirrors` (use `links:`, not `legacylinks:` — those are
  dead on purpose)
- The Cardigann `search.paths` / selector blocks map onto
  `selectors` (HTML) or `json_fields` (JSON) — see the annotated
  example below and the existing files in `sources/verified/` for real
  ported examples (`sources/verified/x1337x.json` is a good one to
  read first).

**If you're analyzing a site yourself** (no Jackett definition exists,
or you found a better/faster site), that's also fully supported — set
`origin.kind` to `"manual"` instead of `"jackett"` and skip
`jackett_id`. You'll just be responsible for noticing if the site
changes, since the sync bot has nothing to check it against.

## 2. Write the file

Create `sources/community/<id>.json`. The filename (minus `.json`)
**must** equal the `id` field inside it — CI rejects a mismatch.

Minimal HTML-site example:

```json
{
  "id": "example_site",
  "display_name": "Example Site",
  "kind": "html",
  "mirrors": ["https://example-torrents.com"],
  "search_path": "/search?q={query}",
  "selectors": {
    "row": "table.results > tbody > tr",
    "title": "a.torrent-title",
    "magnet_location": "listing",
    "magnet": "a[href^=\"magnet:?xt=\"]",
    "magnet_attr": "href",
    "size": "td.size",
    "seeds": "td.seeds",
    "peers": "td.peers"
  },
  "origin": {
    "kind": "jackett",
    "jackett_id": "example-site"
  }
}
```

A JSON-API site example, and the full field reference for every
optional selector (detail-page magnet fetches, title-fallback-via-href
for sites that truncate long titles, category-to-audio-tag folding,
etc.), are documented directly in the schema:
[`schema/source.schema.json`](../schema/source.schema.json). Your
editor will give you autocomplete + inline docs for every field if it
points `$schema` there — most editors (VS Code, JetBrains) do this
automatically for any JSON file that starts with a `$schema` key, or
you can associate the schema URL manually in your editor settings.

## 3. Validate locally before opening a PR

```bash
cd tools/validator
cargo run -- check --root ../.. --only example_site         # schema only, instant
cargo run -- check --root ../.. --live --only example_site  # + real fetch against the site
```

The `--live` run actually hits the site with a test query and confirms
your selectors resolve to real data — this is the same check CI runs
against just the files your PR touches, so running it locally first
saves a round-trip.

## 4. Open the PR

Just the one JSON file under `sources/community/`. CI runs:

1. **Schema check** (instant, every PR) — valid JSON, matches the
   schema, `id` matches the filename, no duplicate `id` across
   `verified/` + `community/`.
2. **Live smoke-test** (only for files your PR actually touches) —
   fetches the site for real and checks your selectors match.

Both green → mergeable. A maintainer may later `git mv` your file from
`community/` to `verified/` once it's proven stable across a few
scheduled sync cycles — that move alone doesn't change how the app
uses it; both directories are live in every release.

## What happens after merge

- **Daily**, `.github/workflows/jackett-sync.yml` re-fetches your
  source's upstream Jackett YAML (if `origin.jackett_id` is set) and
  auto-PRs a `mirrors[]` update if the domain moved. You don't have to
  do anything for a plain mirror change ever again.
- If Jackett deletes the upstream definition entirely, the sync bot
  flags your file with `origin.upstream_removed: true` instead of
  guessing — it never silently breaks or deletes your source. A
  maintainer (or you) then decides whether to keep it going as
  `"manual"` or retire it.
- If the site's **HTML structure** changes (not just its domain), the
  sync bot can't fix that automatically — a changed CSS class isn't
  something you can diff out of a YAML file. The next scheduled
  `--live` validator run (or the next PR that touches the file) will
  catch it and fail loudly, and it becomes a normal "fix the
  selectors" PR like your original contribution.
