# Contributing a source

StreamX Ultra's indexer is Rust-based and does **not** support Jackett's
YAML (Cardigann) format directly. Instead, every site is a plain JSON
file matching `schema/source.schema.json`. If you've written a Jackett
YAML definition before, this will feel familiar — it's the same ideas
(selectors, mirrors, field mapping, login), just JSON instead of YAML,
and a flatter shape.

You do **not** need to touch any Rust code to add a source. Drop a JSON
file in the right folder, open a PR, and CI does the rest.

This guide covers **public sites** first (the common case), then
[private trackers](#adding-a-private-tracker) (sites that require a
per-user login) further down — the file format is the same either way,
private trackers just have a couple of extra fields.

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
- Check the Cardigann `search:` block for a `method: post` — if
  present, set `search_method: "post"` and `search_body` (see
  [Two search methods](#two-search-methods) below) instead of the
  default GET.
- Check the row's download link. If Cardigann's `download:` selector
  points at a `magnet:?xt=` href, this is a normal magnet site — use
  `download_type: "magnet"` (the default, can be omitted). If it points
  at something like `download.php?id=` or `.torrent` instead, the site
  only offers an authenticated file download, not a magnet — this is
  the norm for private trackers and needs `download_type:
  "torrent_file"` plus a `request.auth` block; see
  [Adding a private tracker](#adding-a-private-tracker) below.

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

### Two search methods

Every site defaults to `search_method: "get"` — search params are
built into the URL from `search_path`, exactly like the example above.
This covers the large majority of sites, public and private alike.

Some sites (mostly private trackers with an AJAX-style search endpoint)
only accept a POST with a form-encoded body instead. Set
`search_method: "post"` and add `search_body`, using the same
`{query}`/`{page}` placeholders as `search_path`:

```json
{
  "search_method": "post",
  "search_path": "/ajax/search.php",
  "search_body": "query={query}&page={page}&category=movies"
}
```

`search_path` is still required either way — in POST mode it's just the
endpoint the body gets sent to, with no query string of its own needed
(though `?foo=bar` on it is fine too, if the site expects that).

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

## Adding a private tracker

A private tracker is a normal source file with two additions: a
`request.auth` block, and (almost always) `download_type:
"torrent_file"` instead of a magnet — see why in
[Capabilities](../README.md#capabilities) in the README. Everything
else in this guide (mirrors, search selectors, validating, opening a
PR) applies exactly the same way.

**File location**: `sources/private/<id>.json` instead of
`sources/community/`. Same id-matches-filename rule, same merge
priority rules (`verified/` > `community/` > `private/` if an id
somehow collides across directories, which CI rejects before merge
regardless).

**What the `auth` block does — and doesn't — describe**: it tells the
app *that* this site needs a cookie and *how to verify* one still
works. It never contains a real cookie, username, or password — those
are supplied by the app itself at search time, from wherever the app's
own login flow stores them (StreamX Ultra uses an in-app WebView login
+ on-device encrypted storage; see that repo's `PrivateTrackerCookieStore`
and `TrackerLoginScreen` if you want a reference implementation, though
this crate has no dependency on either).

A real, trimmed example (`sources/private/hdtorrents.json`):

```json
{
  "id": "hdtorrents",
  "display_name": "HD-Torrents",
  "kind": "html",
  "mirrors": ["https://hd-torrents.org", "https://hd-torrents.net"],
  "search_path": "/torrents.php?search={query}&category%5B%5D=70",
  "request": {
    "auth": {
      "method": "cookie",
      "instructions": "Log in with your HD-Torrents account — the app captures your session automatically once login succeeds.",
      "login_check_path": "/",
      "login_check_selector": "a[href^=\"logout.php?check_hash=\"]"
    }
  },
  "selectors": {
    "row": "table.mainblockcontenttt > tbody > tr:has(a[href^=\"details.php?id=\"])",
    "title": "a[href^=\"details.php?id=\"]",
    "download_type": "torrent_file",
    "magnet_location": "listing",
    "magnet": "a[href^=\"download.php?id=\"]",
    "magnet_attr": "href",
    "size": "td:nth-child(8)",
    "seeds": "td:nth-last-child(3)",
    "peers": "td:nth-last-child(2)"
  }
}
```

Notes on the two new pieces:

- **`request.auth.login_check_path` / `login_check_selector`**: a page
  and CSS selector that's only present when genuinely logged in — a
  logout link, a "my torrents" nav item, anything reliably absent from
  a logged-out response. This isn't used during search itself; it
  exists so a consuming app can verify a stored cookie is still good
  *before* running a real search with it, instead of a confusing
  zero-results response when a session has quietly expired.
- **`selectors.download_type: "torrent_file"`**: reuses the *same*
  `magnet`/`magnet_attr` (or `detail_link`/`detail_magnet_selector`)
  fields you'd use for a real magnet site — just pointed at the site's
  `.torrent` download link instead. The engine writes the extracted URL
  into `TorrentResult::torrent_file_url` rather than `::magnet` when
  this is set, and flags `requires_torrent_auth: true` so the consuming
  app knows to attach the site's cookie when it fetches that URL (the
  download itself needs the same authenticated session the search did
  — that download hit is how private trackers track ratio).

If the site's search endpoint needs a POST (common for private
trackers with an AJAX search box), combine this with `search_method:
"post"` / `search_body` from the [Two search methods](#two-search-methods)
section above — `sources/private/torrentbd.json` is a real example that
does both at once.

**Validating a private tracker locally is different**: `tools/validator`
has no cookie to log in with, so `--live` can't meaningfully exercise
the authenticated path — CI's `validate-sources.yml` knows this and
deliberately skips the live smoke-test for anything under
`sources/private/`, leaving a PR comment saying so instead of running
it unauthenticated and reporting a confusing false failure. The
schema-check job still runs and still gates the PR — that part is
identical to a public source. Confirm the selectors are actually
correct by hand against a real logged-in browser session, and note
that you've done so in the PR description (a screenshot of a matching
search result is the easiest way to show a reviewer).

```bash
cd tools/validator
cargo run -- check --root ../.. --only hdtorrents   # schema check — this is what actually gates the PR
```

`--live` still works locally if you want to sanity-check the mirror is
reachable at all, but expect it to report selector mismatches (a
logged-out response looks nothing like a logged-in one) — that's
expected for a private tracker, not a sign your file is wrong.

## What happens after merge

Everything below applies the same way to `sources/private/` as it does
to `verified/`/`community/` — `jackett-sync` only ever touches
`mirrors[]`, so it's blind to whether a site needs auth or not.

- **Daily**, `.github/workflows/jackett-sync.yml` re-fetches your
  source's upstream Jackett YAML (if `origin.jackett_id` is set) and
  auto-PRs a `mirrors[]` update if the domain moved. You don't have to
  do anything for a plain mirror change ever again.
- If one of your mirrors turns out to be genuinely dead (not just
  temporarily bot-challenge-blocked — see the next section) but
  Jackett's own upstream `links:` hasn't been cleaned up yet, add it to
  `excluded_mirrors` instead of just deleting it from `mirrors[]` — a
  plain deletion gets silently undone by the next `jackett-sync` run,
  since as far as that tool can tell nothing changed. `excluded_mirrors`
  is a permanent blocklist that survives every future sync:
  ```json
  "mirrors": ["https://example-torrents.com"],
  "excluded_mirrors": ["https://example-torrents-dead-mirror.com"]
  ```
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

## Blocked vs. dead mirrors — don't confuse the two

A mirror behind Cloudflare (or a similar bot-challenge wall) will
return HTTP 403/503/429 to the validator's automated fetch even though
it works completely fine in a real browser — the site is alive, it's
just refusing this specific unverified request. `tools/validator`
treats this as **blocked**, not dead: it tries the next mirror instead
of giving up, and if every mirror is merely blocked (none actually
unreachable), that alone won't fail a PR (`--tolerate-blocked` is
always passed in CI, since GitHub Actions runner IPs commonly sit on
Cloudflare's own datacenter blocklists regardless of headers).

Only add a mirror to `excluded_mirrors` once you've confirmed it's
**genuinely dead** — DNS failure (`DNS_PROBE_FINISHED_NXDOMAIN`),
connection refused, or a plain non-challenge error status, checked in
an actual browser, not just "the validator got a 403." If you're not
sure which one you're looking at, open the mirror in a normal browser
tab: a Cloudflare challenge page (even a "checking your browser..."
interstitial) means it's blocked, not dead.

