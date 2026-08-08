// tools/validator/src/schema_check.rs
//
// Schema-level checks — no network. Runs against every *.json file
// under sources/verified/, sources/community/, and sources/private/
// (excluding special-sites.json, which has its own tiny shape and is
// checked separately by parse-only since it's not a SiteConfig).

use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct FileCheck {
    pub path: PathBuf,
    pub id_from_filename: String,
    pub parsed: Option<streamx_indexer::schema::SiteConfig>,
    pub errors: Vec<String>,
}

pub fn run(
    schema_path: &Path,
    sources_dir: &Path,
    only: &[String],
) -> Result<Vec<FileCheck>> {
    let schema_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(schema_path)
            .with_context(|| format!("reading {}", schema_path.display()))?,
    )?;
    let compiled = jsonschema::JSONSchema::compile(&schema_json)
        .map_err(|e| anyhow::anyhow!("schema/source.schema.json itself is invalid: {e}"))?;

    let mut results = Vec::new();
    let mut seen_ids: HashMap<String, PathBuf> = HashMap::new();

    for dir_name in ["verified", "community", "private"] {
        let dir = sources_dir.join(dir_name);
        if !dir.exists() {
            continue;
        }
        let mut entries: Vec<_> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
            .collect();
        entries.sort();

        for path in entries {
            let filename_stem = path.file_stem().unwrap().to_string_lossy().to_string();
            if !only.is_empty() && !only.contains(&filename_stem) {
                continue;
            }

            let mut errors = Vec::new();
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;

            let value: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(format!("invalid JSON: {e}"));
                    results.push(FileCheck {
                        path,
                        id_from_filename: filename_stem,
                        parsed: None,
                        errors,
                    });
                    continue;
                }
            };

            if let Err(schema_errors) = compiled.validate(&value) {
                for err in schema_errors {
                    errors.push(format!("{} at {}", err, err.instance_path));
                }
            }

            let parsed: Option<streamx_indexer::schema::SiteConfig> =
                serde_json::from_value(value.clone()).ok();

            if let Some(cfg) = &parsed {
                if cfg.id != filename_stem {
                    errors.push(format!(
                        "id '{}' does not match filename '{}.json'",
                        cfg.id, filename_stem
                    ));
                }
                if let Some(existing) = seen_ids.get(&cfg.id) {
                    errors.push(format!(
                        "duplicate id '{}' — also defined in {}",
                        cfg.id,
                        existing.display()
                    ));
                } else {
                    seen_ids.insert(cfg.id.clone(), path.clone());
                }
                if cfg.mirrors.is_empty() {
                    errors.push("mirrors[] must not be empty".to_string());
                }
            } else if errors.is_empty() {
                errors.push("failed to deserialize into SiteConfig despite passing JSON Schema — this is a schema/schema.rs drift bug, please report it".to_string());
            }

            results.push(FileCheck {
                path,
                id_from_filename: filename_stem,
                parsed,
                errors,
            });
        }
    }

    Ok(results)
}

/// Prints a summary line per file and returns true if everything passed.
pub fn report(results: &[FileCheck]) -> bool {
    let mut all_ok = true;
    for r in results {
        if r.errors.is_empty() {
            println!("  {} {}", "✓".green(), r.id_from_filename);
        } else {
            all_ok = false;
            println!("  {} {}", "✗".red().bold(), r.id_from_filename);
            for e in &r.errors {
                println!("      {} {}", "-".red(), e);
            }
        }
    }
    if results.is_empty() {
        println!("  {}", "(no source files found)".dimmed());
    }
    all_ok
}
