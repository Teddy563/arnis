//! Target Minecraft version as a first-class input.
//!
//! Everything version-dependent — the `DataVersion` stamped into every chunk, whether the
//! world may declare an extended height at all, which chunk layout the writer must use —
//! is resolved through [`capabilities`] and never hardcoded at a call site.
//!
//! The table lives in `assets/mc_versions.json` and carries one rule: **no value in it is
//! written from memory.** Rows ship only with numbers read out of a real world, recorded
//! in `verified_from`. A wrong `DataVersion` produces a world that loads and then quietly
//! misbehaves, so an unknown version is refused rather than guessed at.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

static TABLE_JSON: &str = include_str!("../assets/mc_versions.json");

/// Which shape `pack.mcmeta` and `dimension_type` must take for a version.
///
/// Not cosmetic: 26.1.2 rejects the 1.21.x metadata outright — "Failed to read pack
/// metadata" — and the world then refuses to load entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatapackSchema {
    /// 1.21.4-1.21.10: integer `pack_format` with `supported_formats` / `min_format` /
    /// `max_format`, and the multi-overlay dimension_type tree.
    Legacy,
    /// 26.x: DECIMAL `pack_format` / `min_format` / `max_format` (e.g. 101.1) and the
    /// attributes/timelines dimension_type schema.
    Modern,
}

/// How chunks are laid out in the region file. Pre-1.18 is a genuinely different writer
/// path (a `Level` compound with `Level.Sections` and int-array biomes), not a variation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkLayout {
    /// 1.18+: root-level `sections`, per-section `block_states`/`biomes` palettes, `yPos`.
    Flat,
    /// Pre-1.18: `Level` compound. Not implemented by this writer.
    Legacy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionCaps {
    pub id: String,
    #[serde(default)]
    pub data_version: Option<i32>,
    pub extended_height: bool,
    pub chunk_layout: ChunkLayout,
    /// Shape of the extended-height datapack for this version.
    #[serde(default = "default_schema")]
    pub datapack_schema: DatapackSchema,
    /// Decimal `pack_format` for a `Modern` row. `None` = not verified, so extended
    /// height is refused rather than guessing a number.
    #[serde(default)]
    pub datapack_format: Option<f64>,
    #[serde(default)]
    pub note: Option<String>,
    /// Where this row's numbers were read from. Not consumed by generation — it exists so
    /// provenance travels with the value, and a test asserts every row carrying a
    /// DataVersion has one.
    #[allow(dead_code)]
    #[serde(default)]
    pub verified_from: Option<String>,
}

fn default_schema() -> DatapackSchema {
    DatapackSchema::Legacy
}

#[derive(Debug, Deserialize)]
struct PackEnvelope {
    pack_format: i32,
    min_inclusive: i32,
    max_inclusive: i32,
}

#[derive(Debug, Deserialize)]
struct DefaultDataVersion {
    data_version: i32,
}

#[derive(Debug, Deserialize)]
struct Table {
    #[serde(rename = "_pack_format_envelope")]
    pack_format_envelope: PackEnvelope,
    #[serde(rename = "_default_data_version")]
    default_data_version: DefaultDataVersion,
    versions: Vec<VersionCaps>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(TABLE_JSON)
            .expect("assets/mc_versions.json is malformed (checked in, so this is a build bug)")
    })
}

fn index() -> &'static HashMap<String, VersionCaps> {
    static I: OnceLock<HashMap<String, VersionCaps>> = OnceLock::new();
    I.get_or_init(|| {
        table()
            .versions
            .iter()
            .map(|v| (v.id.to_ascii_lowercase(), v.clone()))
            .collect()
    })
}

/// Capabilities of a target version, or `None` when the version is not in the table.
///
/// `None` means "we have never verified this version's constants", and the caller must
/// refuse rather than fall back to a default — that is the whole point of the table.
pub fn capabilities(version: &str) -> Option<&'static VersionCaps> {
    index().get(&version.trim().to_ascii_lowercase())
}

/// Every version we can currently target, in table order. For the UI's selector and for
/// error messages that need to say what IS supported.
pub fn known_versions() -> Vec<&'static str> {
    table().versions.iter().map(|v| v.id.as_str()).collect()
}

/// The `DataVersion` to stamp into chunks for this target, or the writer's historical
/// default when no version was requested.
pub fn data_version_for(version: Option<&str>) -> Result<i32, String> {
    match version {
        None => Ok(table().default_data_version.data_version),
        Some(v) => {
            let caps = capabilities(v).ok_or_else(|| unknown_version_message(v))?;
            caps.data_version.ok_or_else(|| {
                format!(
                    "Minecraft {} has no verified DataVersion in assets/mc_versions.json{}. \
                     Generate a world in that version and copy Data.Version.Id from its \
                     level.dat before targeting it.",
                    caps.id,
                    caps.note
                        .as_ref()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                )
            })
        }
    }
}

/// The historical default `DataVersion`, used when the caller names no version.
pub fn default_data_version() -> i32 {
    table().default_data_version.data_version
}

/// Capabilities to assume when the caller names no version: the writer's historical
/// behaviour — a modern (1.18+) flat-chunk world that may declare an extended height.
/// Deliberately carries no version id of its own, because we have not verified which
/// release the default DataVersion belongs to.
pub fn default_caps() -> &'static VersionCaps {
    static D: OnceLock<VersionCaps> = OnceLock::new();
    D.get_or_init(|| VersionCaps {
        id: "default".to_string(),
        data_version: Some(default_data_version()),
        extended_height: true,
        chunk_layout: ChunkLayout::Flat,
        note: Some("no --mc-version given; writer default".to_string()),
        verified_from: None,
        // The historical default predates the 26.x metadata change, so the legacy pack
        // shape is what has always been shipped for it.
        datapack_schema: DatapackSchema::Legacy,
        datapack_format: None,
    })
}

/// The verified `pack.mcmeta` format envelope for emitted datapacks.
/// Returned as `(pack_format, min_inclusive, max_inclusive)`.
pub fn pack_format_envelope() -> (i32, i32, i32) {
    let e = &table().pack_format_envelope;
    (e.pack_format, e.min_inclusive, e.max_inclusive)
}

/// The message for a version we have no verified row for. States what is supported and
/// how to add the missing one, rather than silently picking something close.
pub fn unknown_version_message(version: &str) -> String {
    format!(
        "Unknown Minecraft version '{version}'. Known: {}. Add a row to \
         assets/mc_versions.json with the DataVersion read from a real world's level.dat \
         (Data.Version.Id) — these are never written from memory.",
        known_versions().join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_has_the_verified_rows() {
        // Read out of real level.dat files (2026-08-05) and Mojang's own version.json inside
        // the client jars (2026-08-09); see `verified_from` in the JSON.
        for (id, dv) in [
            ("1.21.4", 4189),
            ("1.21.5", 4325),
            ("1.21.10", 4556),
            ("1.21.11", 4671),
            ("26.1", 4786),
            ("26.1.1", 4788),
            ("26.1.2", 4790),
            ("26.2", 4903),
        ] {
            let c = capabilities(id).unwrap_or_else(|| panic!("missing row {id}"));
            assert_eq!(c.data_version, Some(dv), "{id}");
            assert!(c.extended_height, "{id} should support extended height");
            assert_eq!(c.chunk_layout, ChunkLayout::Flat);
            assert!(
                c.verified_from.is_some(),
                "{id} must record where it was verified"
            );
        }
    }

    #[test]
    fn every_row_with_a_data_version_records_its_source() {
        for v in &table().versions {
            if v.data_version.is_some() {
                assert!(
                    v.verified_from.is_some(),
                    "{} carries a DataVersion with no verified_from — that is exactly the \
                     value we must never write from memory",
                    v.id
                );
            }
        }
    }

    #[test]
    fn every_modern_row_carries_a_verified_pack_format() {
        // A Modern row without one refuses extended height at generation time, which is the
        // failure sewer hit on 26.2. Every 26.x row we ship now has a verified number.
        for v in &table().versions {
            if v.datapack_schema == DatapackSchema::Modern {
                assert!(
                    v.datapack_format.is_some(),
                    "{} uses the 26.x schema but has no verified pack_format, so extended \
                     height would be refused for it",
                    v.id
                );
            }
        }
    }

    #[test]
    fn pre_117_is_present_and_marked_unsupported() {
        let c = capabilities("1.16.5").unwrap();
        assert!(!c.extended_height);
        assert_eq!(c.chunk_layout, ChunkLayout::Legacy);
        // It deliberately has no DataVersion: we refuse before we would ever need one.
        assert!(c.data_version.is_none());
    }

    #[test]
    fn unknown_version_is_refused_not_guessed() {
        assert!(capabilities("1.42.7").is_none());
        let err = data_version_for(Some("1.42.7")).unwrap_err();
        assert!(err.contains("Unknown Minecraft version"));
        assert!(
            err.contains("1.21.4"),
            "the error should list what IS known"
        );
    }

    #[test]
    fn lookup_is_case_and_whitespace_insensitive() {
        assert!(capabilities(" 1.21.4 ").is_some());
        assert!(capabilities("26.1.2").is_some());
    }

    #[test]
    fn default_data_version_matches_what_the_writer_has_always_emitted() {
        assert_eq!(default_data_version(), 4440);
        assert_eq!(data_version_for(None).unwrap(), 4440);
    }

    #[test]
    fn pack_envelope_matches_the_bundled_pack() {
        assert_eq!(pack_format_envelope(), (61, 61, 101));
    }
}
