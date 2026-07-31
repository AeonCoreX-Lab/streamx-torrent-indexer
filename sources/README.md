# sources/

Each file here is one torrent-site definition, matching
`schema/source.schema.json` and `crate/src/schema.rs::SiteConfig`.

- **verified/** — sites reviewed and maintained by AeonCoreX-Lab. These
  ship in every StreamX Ultra release.
- **community/** — sites contributed by anyone via PR. Fully functional
  as soon as CI's validator passes (schema check + live selector
  smoke-test) — the split from `verified/` is a review-status label,
  not a functionality gate. A maintainer promotes a file from
  `community/` to `verified/` (a plain `git mv`) once it's proven
  stable across a few `jackett-sync` cycles.

`special-sites.json` is different: it holds mirror lists (and
`origin.jackett_id` provenance) for the handful of sites whose scraping
logic is too bespoke for the generic engine and lives as hand-written
Rust under `crate/src/special/` instead (kdrama, nyaa, tokyotosho,
eztvco). You still edit this file the same way to fix a dead mirror —
you just can't *add a new site* here without also writing a Rust
module, which is why regular sources go through the JSON-only path
above instead.

See `docs/CONTRIBUTING.md` for the full walkthrough of adding a source.
