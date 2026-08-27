//! Per-tile bincode sidecar for the `--osm-tile-dir` slippy-tile cache (phase-5 A2/A3).
//!
//! Each `osm_g1_z{z}_{x}_{y}.json` tile may have a paired `.osmbin` holding the
//! bincode of the exact `Vec<OsmElement>` serde_json produced for that tile,
//! verbatim in file order — no sorting, no per-tile dedup, no tag filtering —
//! so decoding the sidecar yields element-for-element what decoding the JSON
//! would, minus the JSON tokenizer cost.
//!
//! Trust model: the source `.json` stays the single source of truth. A sidecar
//! is only used when (a) its header carries the xxh3 + byte length of the exact
//! JSON bytes just read from disk (re-hashed on EVERY read — content hash,
//! never mtime), and (b) its verified bit is set, which only verify-at-bake
//! stamps: the baker re-decodes its own freshly written file and compares
//! element-for-element (f64 bit-wise via `to_bits`, never `PartialEq`) against
//! the Vec it just decoded from JSON. Any mismatch, short file, or decode
//! error on read falls through silently to the JSON path and re-bakes.
//! Deleting every `.osmbin` is a complete rollback.

use crate::osm_parser::OsmElement;
use bincode::Options;
use std::fs;
use std::io::{Seek, SeekFrom, Write};

/// Sidecar codec version. NEVER derive this from `CARGO_PKG_VERSION`.
///
/// MUST be bumped on:
///  - any serde-shape change to `OsmElement` / `OsmMember` (field added,
///    removed, renamed, reordered, or retyped), AND
///  - any bincode crate version bump or any change to `codec()` below —
///    bincode is not self-describing and its wire bytes can change with zero
///    Rust-shape change; an old sidecar under a new bincode either fails open
///    (fine) or misdecodes silently, fleet-wide, invisible to verify-at-bake,
///    which only ever ran under the old binary. bincode is on the
///    output-affecting pin list (plan G5) for the same reason.
pub const OSM_SIDECAR_CODEC_VERSION: u32 = 1;

/// 8-byte file magic. "ARNOSMB" + a format generation digit.
const MAGIC: [u8; 8] = *b"ARNOSMB1";

/// Fixed header layout (little-endian throughout):
///   0..8   magic
///   8..12  OSM_SIDECAR_CODEC_VERSION (u32)
///  12..20  xxh3_64 of the exact source .json bytes (u64)
///  20..28  byte length of the exact source .json bytes (u64)
///  28..36  element count (u64)
///  36      verified bit (0 until verify-at-bake passes, then 1)
///  37..40  reserved, zero
const HEADER_LEN: usize = 40;
const VERIFIED_OFFSET: u64 = 36;

/// The one bincode configuration both bake and read use. Any change to this
/// chain is a wire-format change and MUST bump `OSM_SIDECAR_CODEC_VERSION`.
/// The limit caps what a corrupted length prefix can ask the allocator for
/// (the header hash covers the source JSON, not the payload bytes).
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(1 << 31)
}

/// `<tile>.json` -> `<tile>.osmbin` (paired files, same directory, same stem —
/// Meld's TTL/prune/cleanup deletes them together).
/// Are sidecars enabled at all? `ARNIS_OSM_SIDECARS=0` disables both the read and the
/// bake, so a cache-size-sensitive install never grows an .osmbin next to its tiles.
/// Anything else (unset included) keeps the default: enabled. Read once.
pub fn sidecars_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("ARNIS_OSM_SIDECARS")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

pub fn sidecar_path(json_path: &str) -> String {
    match json_path.strip_suffix(".json") {
        Some(stem) => format!("{stem}.osmbin"),
        None => format!("{json_path}.osmbin"),
    }
}

fn header_bytes(json_bytes: &[u8], element_count: u64, verified: bool) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..8].copy_from_slice(&MAGIC);
    h[8..12].copy_from_slice(&OSM_SIDECAR_CODEC_VERSION.to_le_bytes());
    h[12..20].copy_from_slice(&xxhash_rust::xxh3::xxh3_64(json_bytes).to_le_bytes());
    h[20..28].copy_from_slice(&(json_bytes.len() as u64).to_le_bytes());
    h[28..36].copy_from_slice(&element_count.to_le_bytes());
    h[VERIFIED_OFFSET as usize] = verified as u8;
    h
}

/// Try to load the sidecar at `path` for a source `.json` whose exact bytes are
/// `json_bytes`. Returns `None` (silent fall-through to the JSON path) on ANY
/// discrepancy: missing/short file, bad magic, codec-version mismatch,
/// unverified bit, byte-length or content-hash mismatch against the JSON bytes
/// just read, bincode decode error, or element-count mismatch.
pub fn read_verified(path: &str, json_bytes: &[u8]) -> Option<Vec<OsmElement>> {
    let buf = fs::read(path).ok()?;
    if buf.len() < HEADER_LEN || buf[0..8] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().ok()?);
    if version != OSM_SIDECAR_CODEC_VERSION {
        return None;
    }
    let json_hash = u64::from_le_bytes(buf[12..20].try_into().ok()?);
    let json_len = u64::from_le_bytes(buf[20..28].try_into().ok()?);
    let element_count = u64::from_le_bytes(buf[28..36].try_into().ok()?);
    if buf[VERIFIED_OFFSET as usize] != 1 {
        return None;
    }
    if json_len != json_bytes.len() as u64 {
        return None;
    }
    // Content hash of the source bytes, recomputed on EVERY read — never mtime.
    if json_hash != xxhash_rust::xxh3::xxh3_64(json_bytes) {
        return None;
    }
    let elements: Vec<OsmElement> = codec().deserialize(&buf[HEADER_LEN..]).ok()?;
    if elements.len() as u64 != element_count {
        return None;
    }
    Some(elements)
}

/// Bake a sidecar for `json_bytes` -> `elements` (the Vec just decoded from
/// that JSON). Best-effort and infallible from the caller's view: the caller
/// already holds the decoded Vec, so every failure here is swallowed and only
/// means "no sidecar this time".
///
/// Verify-at-bake: the tmp file is re-read and re-decoded, compared
/// element-for-element (f64 via `to_bits`) against `elements`, and ONLY then
/// stamped verified and published via atomic rename. A sidecar that fails its
/// own verify is deleted, not stamped. First-writer-wins: on Windows a rename
/// onto an open destination fails — a lost race is swallowed silently.
pub fn bake(path: &str, json_bytes: &[u8], elements: &[OsmElement]) {
    let tmp = format!("{path}.tmp{}", std::process::id());
    let baked = (|| -> std::io::Result<()> {
        let f = fs::File::create(&tmp)?;
        // BufWriter is load-bearing: bincode's serialize_into issues many tiny
        // writes, and unbuffered that is a syscall per field (measured 23s vs
        // sub-second for a 99 MB sidecar).
        let mut w = std::io::BufWriter::with_capacity(4 << 20, f);
        w.write_all(&header_bytes(json_bytes, elements.len() as u64, false))?;
        codec()
            .serialize_into(&mut w, elements)
            .map_err(std::io::Error::other)?;
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_data()?;
        Ok(())
    })()
    .is_ok();
    if !baked {
        let _ = fs::remove_file(&tmp);
        return;
    }
    // Verify-at-bake: re-decode the freshly written bytes end-to-end.
    if !read_back_matches(&tmp, elements) {
        // Failed its own verify: deleted, never stamped.
        let _ = fs::remove_file(&tmp);
        return;
    }
    // Stamp the verified bit, then publish atomically.
    let stamped = (|| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new().write(true).open(&tmp)?;
        f.seek(SeekFrom::Start(VERIFIED_OFFSET))?;
        f.write_all(&[1u8])?;
        f.sync_data()?;
        Ok(())
    })()
    .is_ok();
    if !stamped || fs::rename(&tmp, path).is_err() {
        // Lost the first-writer-wins race (or stamping failed): swallow, the
        // caller keeps using the Vec it decoded from JSON.
        let _ = fs::remove_file(&tmp);
    }
}

/// Decode the not-yet-stamped tmp sidecar at `path` and compare it
/// element-for-element against `expected`.
fn read_back_matches(path: &str, expected: &[OsmElement]) -> bool {
    let Ok(buf) = fs::read(path) else {
        return false;
    };
    if buf.len() < HEADER_LEN {
        return false;
    }
    let Ok(decoded) = codec().deserialize::<Vec<OsmElement>>(&buf[HEADER_LEN..]) else {
        return false;
    };
    elements_match(&decoded, expected)
}

/// Element-for-element equality with every f64 compared bit-wise via
/// `to_bits`, never `PartialEq` (NaN != NaN would false-fail a NaN roundtrip;
/// -0.0 == 0.0 would false-pass a sign-flipping codec bug).
pub fn elements_match(a: &[OsmElement], b: &[OsmElement]) -> bool {
    fn bits(v: Option<f64>) -> Option<u64> {
        v.map(f64::to_bits)
    }
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.r#type == y.r#type
                && x.id == y.id
                && bits(x.lat) == bits(y.lat)
                && bits(x.lon) == bits(y.lon)
                && x.nodes == y.nodes
                && x.tags == y.tags
                && x.members.len() == y.members.len()
                && x.members.iter().zip(y.members.iter()).all(|(m, n)| {
                    m.r#type == n.r#type && m.r#ref == n.r#ref && m.r#role == n.r#role
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::OsmMember;
    use std::collections::HashMap;

    fn node(id: u64, lat: f64, lon: f64) -> OsmElement {
        OsmElement {
            r#type: "node".to_string(),
            id,
            lat: Some(lat),
            lon: Some(lon),
            nodes: None,
            tags: None,
            members: Vec::new(),
        }
    }

    /// Bake to a temp dir and read back with the same json bytes; asserts the
    /// read succeeds and matches bit-wise, then returns the decoded Vec.
    fn roundtrip(json: &[u8], elements: &[OsmElement]) -> Vec<OsmElement> {
        let dir = tempfile::tempdir().unwrap();
        let path = sidecar_path(&format!("{}/tile.json", dir.path().display()));
        bake(&path, json, elements);
        let decoded = read_verified(&path, json).expect("verified sidecar must read back");
        assert!(elements_match(&decoded, elements));
        decoded
    }

    #[test]
    fn sidecar_path_maps_json_to_osmbin() {
        assert_eq!(
            sidecar_path("a/b/osm_g1_z11_1_2.json"),
            "a/b/osm_g1_z11_1_2.osmbin"
        );
        assert_eq!(sidecar_path("weird_name"), "weird_name.osmbin");
    }

    #[test]
    fn roundtrip_tags_none_vs_some_empty() {
        let mut with_empty = node(2, 3.0, 4.0);
        with_empty.tags = Some(HashMap::new());
        let els = vec![node(1, 1.0, 2.0), with_empty];
        let decoded = roundtrip(b"{}", &els);
        assert!(decoded[0].tags.is_none());
        assert_eq!(decoded[1].tags.as_ref().map(|t| t.len()), Some(0));
        // and the compare itself must not conflate the two variants
        let mut some_empty = node(1, 1.0, 2.0);
        some_empty.tags = Some(HashMap::new());
        assert!(!elements_match(&[some_empty], &[node(1, 1.0, 2.0)]));
    }

    #[test]
    fn roundtrip_nodes_none_vs_some_empty() {
        let way_empty_nodes = OsmElement {
            r#type: "way".to_string(),
            id: 2,
            lat: None,
            lon: None,
            nodes: Some(Vec::new()),
            tags: None,
            members: Vec::new(),
        };
        let els = vec![node(1, 1.0, 2.0), way_empty_nodes];
        let decoded = roundtrip(b"x", &els);
        assert!(decoded[0].nodes.is_none());
        assert_eq!(decoded[1].nodes.as_ref().map(|n| n.len()), Some(0));
        // compare must not conflate None with Some(empty)
        let mut some_empty = node(1, 1.0, 2.0);
        some_empty.nodes = Some(Vec::new());
        assert!(!elements_match(&[some_empty], &[node(1, 1.0, 2.0)]));
    }

    #[test]
    fn roundtrip_members_default_empty_and_populated() {
        let plain = node(1, 0.5, 0.5); // members default-empty
        let rel = OsmElement {
            r#type: "relation".to_string(),
            id: 9,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(HashMap::from([(
                "type".to_string(),
                "multipolygon".to_string(),
            )])),
            members: vec![
                OsmMember {
                    r#type: "way".to_string(),
                    r#ref: 77,
                    r#role: "outer".to_string(),
                },
                OsmMember {
                    r#type: "way".to_string(),
                    r#ref: 78,
                    r#role: "inner".to_string(),
                },
            ],
        };
        let decoded = roundtrip(b"json bytes", &[plain, rel]);
        assert!(decoded[0].members.is_empty());
        assert_eq!(decoded[1].members.len(), 2);
        assert_eq!(decoded[1].members[1].r#ref, 78);
        assert_eq!(decoded[1].members[1].r#role, "inner");
    }

    #[test]
    fn roundtrip_non_ascii_strings() {
        let mut el = node(5, 44.4, 26.1);
        el.tags = Some(HashMap::from([
            ("name".to_string(), "Piața Universității".to_string()),
            ("name:ja".to_string(), "ブカレスト大学広場".to_string()),
            ("note:höhe".to_string(), "Straße — ✓ асфальт".to_string()),
        ]));
        let decoded = roundtrip("β-json".as_bytes(), &[el]);
        let tags = decoded[0].tags.as_ref().unwrap();
        assert_eq!(tags["name"], "Piața Universității");
        assert_eq!(tags["name:ja"], "ブカレスト大学広場");
        assert_eq!(tags["note:höhe"], "Straße — ✓ асфальт");
    }

    #[test]
    fn roundtrip_unknown_type_string() {
        let mut el = node(6, 1.0, 1.0);
        el.r#type = "changeset-θ".to_string();
        let decoded = roundtrip(b"j", &[el]);
        assert_eq!(decoded[0].r#type, "changeset-θ");
    }

    #[test]
    fn roundtrip_f64_bit_patterns() {
        // NaN with payload, negative zero, infinities, subnormal: every one
        // must survive bit-exactly (compared via to_bits, never PartialEq).
        let patterns: [u64; 6] = [
            0x7ff8_0000_dead_beef, // quiet NaN with payload
            (-0.0f64).to_bits(),   // negative zero
            f64::INFINITY.to_bits(),
            f64::NEG_INFINITY.to_bits(),
            0x0000_0000_0000_0001,              // smallest subnormal
            26.084_776_318_136_82f64.to_bits(), // ordinary coordinate
        ];
        let els: Vec<OsmElement> = patterns
            .iter()
            .enumerate()
            .map(|(i, &bits)| node(i as u64, f64::from_bits(bits), f64::from_bits(bits ^ 1)))
            .collect();
        let decoded = roundtrip(b"f64", &els);
        for (i, &bits) in patterns.iter().enumerate() {
            assert_eq!(decoded[i].lat.map(f64::to_bits), Some(bits));
            assert_eq!(decoded[i].lon.map(f64::to_bits), Some(bits ^ 1));
        }
    }

    #[test]
    fn read_rejects_stale_json_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = sidecar_path(&format!("{}/t.json", dir.path().display()));
        bake(&path, b"original json bytes", &[node(1, 1.0, 2.0)]);
        assert!(read_verified(&path, b"original json bytes").is_some());
        // same length, different content
        assert!(read_verified(&path, b"ORIGINAL json bytes").is_none());
        // different length
        assert!(read_verified(&path, b"original json bytes!").is_none());
    }

    #[test]
    fn read_rejects_short_truncated_and_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = sidecar_path(&format!("{}/t.json", dir.path().display()));
        bake(&path, b"j", &[node(1, 1.0, 2.0), node(2, 3.0, 4.0)]);
        let good = fs::read(&path).unwrap();
        // short file (header cut off)
        fs::write(&path, &good[..HEADER_LEN - 1]).unwrap();
        assert!(read_verified(&path, b"j").is_none());
        // truncated payload (header intact) -> decode error
        fs::write(&path, &good[..good.len() - 3]).unwrap();
        assert!(read_verified(&path, b"j").is_none());
        // bad magic
        let mut bad = good.clone();
        bad[0] ^= 0xFF;
        fs::write(&path, &bad).unwrap();
        assert!(read_verified(&path, b"j").is_none());
        // wrong codec version
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&(OSM_SIDECAR_CODEC_VERSION + 1).to_le_bytes());
        fs::write(&path, &bad).unwrap();
        assert!(read_verified(&path, b"j").is_none());
        // verified bit cleared
        let mut bad = good.clone();
        bad[VERIFIED_OFFSET as usize] = 0;
        fs::write(&path, &bad).unwrap();
        assert!(read_verified(&path, b"j").is_none());
        // untouched original still reads
        fs::write(&path, &good).unwrap();
        assert!(read_verified(&path, b"j").is_some());
        // missing file
        fs::remove_file(&path).unwrap();
        assert!(read_verified(&path, b"j").is_none());
    }
}
