// src/indexer/types.rs
//
// Unified torrent search result returned by all indexer sites.
// Serialized to JSON and passed to Kotlin via JNI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentResult {
    pub title:      String,
    pub magnet:     String,
    pub size:       String,
    pub seeds:      u32,
    pub peers:      u32,
    pub source:     String,
    pub audio_tags: Vec<String>,
    pub quality:    String,
    /// True when this result was returned because it carried a
    /// recognized dub/language tag (see parse_tags()). False for
    /// results surfaced through search_dubbed()'s untagged fallback
    /// path — i.e. "best guess, language not confirmed" rather than
    /// "confirmed dub". Always true for search_all()/search_drama()/
    /// search_anime_*() results, which don't apply this distinction.
    /// UI should visually distinguish these (e.g. a "best match" label
    /// instead of a language chip) so users aren't misled into thinking
    /// every result is a confirmed Hindi/Tamil/etc. dub.
    #[serde(default = "default_true")]
    pub is_confirmed_dub: bool,
}

fn default_true() -> bool { true }

impl Default for TorrentResult {
    fn default() -> Self {
        Self {
            title: String::new(),
            magnet: String::new(),
            size: String::new(),
            seeds: 0,
            peers: 0,
            source: String::new(),
            audio_tags: Vec::new(),
            quality: String::new(),
            // Defaults to true: every call site that builds a
            // TorrentResult via `..Default::default()` is a normal,
            // tag-confirmed result UNLESS explicitly marked otherwise —
            // see search_dubbed()'s fallback path in engine.rs, which is
            // the only place this is set to false.
            is_confirmed_dub: true,
        }
    }
}

impl TorrentResult {
    /// Parse audio/language and quality tags from release title.
    /// Mirrors Jackett's title-normalisation filters in the YAML definitions.
    ///
    /// Covers three broad groups:
    ///   1. South-Asian dub terms (Hindi/Tamil/Telugu/etc. — original scope)
    ///   2. Drama-specific language/subtitle terms (Korean/Chinese/Turkish —
    ///      these titles usually say "ENG SUB", "English Dub", or name the
    ///      origin language explicitly rather than "Dubbed")
    ///   3. Anime dub/sub conventions ("Dual Audio [ENG-JAP]", "Dub", "Multi-Sub")
    pub fn parse_tags(&mut self) {
        let t = self.title.to_lowercase();

        // Order: more-specific patterns before generic ones to avoid double-tagging
        let audio_patterns: &[(&str, &str)] = &[
            // ── South Asian dubs (original scope) ──────────────────────────
            ("hindi dubbed",       "Hindi Dubbed"),
            ("hindi dub",          "Hindi Dubbed"),
            ("dual audio",         "Dual Audio"),
            ("dual.audio",         "Dual Audio"),
            ("multi audio",        "Multi Audio"),
            ("multi lang",         "Multi Lang"),
            ("dubbed",             "Dubbed"),
            ("hindi",               "Hindi"),
            ("tamil",                "Tamil"),
            ("telugu",               "Telugu"),
            ("bengali",              "Bengali"),
            ("bangla",               "Bangla"),
            ("malayalam",            "Malayalam"),
            ("kannada",              "Kannada"),
            ("marathi",              "Marathi"),

            // ── K-drama / C-drama / Turkish drama terms ─────────────────────
            // These sites/titles almost never say "Dubbed" — they name the
            // origin language and/or subtitle language explicitly.
            ("english dub",         "English Dub"),
            ("eng dub",             "English Dub"),
            ("eng sub",             "English Sub"),
            ("english sub",         "English Sub"),
            ("esub",                "English Sub"),
            ("multi-sub",           "Multi Sub"),
            ("multisub",            "Multi Sub"),
            ("hardsub",             "English Sub"),
            ("softsub",             "English Sub"),
            ("korean drama",        "Korean"),
            ("k-drama",             "Korean"),
            ("kdrama",              "Korean"),
            (" kor ",               "Korean"),
            ("chinese drama",       "Chinese"),
            ("c-drama",             "Chinese"),
            ("cdrama",              "Chinese"),
            ("mandarin",            "Chinese"),
            ("cantonese",           "Chinese"),
            ("turkish drama",       "Turkish"),
            ("turkish series",      "Turkish"),
            ("dizi",                "Turkish"), // "dizi" = Turkish for "series"
            ("turkce",              "Turkish"),
            ("türkçe",              "Turkish"),

            // ── Anime dub/sub conventions ────────────────────────────────────
            ("eng-jap",             "Dual Audio"),
            ("jap-eng",             "Dual Audio"),
            ("[dual audio]",        "Dual Audio"),
            ("dub]",                "English Dub"), // e.g. "[Dub]" tag suffix
            ("(dub)",               "English Dub"),
            ("[sub]",               "English Sub"),
            ("(sub)",               "English Sub"),
            ("raw]",                "Raw"),
            ("japanese",            "Japanese"),
        ];

        let mut tags: Vec<String> = Vec::new();
        for (pat, label) in audio_patterns {
            if t.contains(pat) {
                let l = label.to_string();
                if !tags.contains(&l) {
                    tags.push(l);
                }
            }
        }
        self.audio_tags = tags;

        self.quality = if t.contains("2160p") || t.contains("4k") || t.contains("uhd") {
            "4K"
        } else if t.contains("1080p") {
            "1080p"
        } else if t.contains("720p") {
            "720p"
        } else if t.contains("480p") {
            "480p"
        } else {
            "SD"
        }
        .to_string();
    }

    pub fn is_dubbed(&self) -> bool {
        !self.audio_tags.is_empty()
    }

    /// True if this release looks like adult/XXX content based on its
    /// title. This is a title-level heuristic — it can't inspect the
    /// actual video, so it errs toward being reasonably broad rather
    /// than narrow, since a false positive (skipping a legitimate result
    /// with an unlucky title) is far less harmful than a false negative
    /// (adult content leaking into a general search — see the "Supergirl
    /// XXX iMAGESET" / "ConorCoxxxClips" results that prompted this).
    ///
    /// Every search_*() entry point in engine.rs applies this filter
    /// unconditionally — there is no user-facing toggle to disable it.
    pub fn is_adult_content(&self) -> bool {
        let t = self.title.to_lowercase();

        // Whole-word / bounded markers. Using simple `contains` on short
        // strings like "xxx" is intentional and safe here because these
        // patterns essentially never appear as substrings of legitimate
        // release tags (unlike e.g. "sex" which could appear in unrelated
        // words) — every pattern below was chosen to be a strong signal
        // on its own in torrent-release-title conventions specifically.
        const MARKERS: &[&str] = &[
            "xxx",
            "porn",
            "1080p.xxx",
            "hentai",
            "jav ",
            "jav.",
            "jav-",
            "onlyfans",
            "brazzers",
            "naughtyamerica",
            "realitykings",
            "bangbros",
            "pornhub",
            "xvideos",
            "camrip.xxx",
            "adult.",
            "nsfw",
            "18+.",
            "erotic",
            "fetish",
            "imageset",     // near-exclusively used for adult photo-set releases
            "clips4sale",
            "manyvids",
            "babes.com",
            "digitalplayground",
            "wicked.",
        ];

        MARKERS.iter().any(|m| t.contains(m))
    }
}
