//! Leaf / Luminol **B_Linear v3** region container (`r.X.Z.b_linear`).
//!
//! An alternative to the Anvil `.mca` container for the same chunk NBT. Where Anvil
//! zlib-compresses each chunk into 4 KiB sectors behind an 8 KiB header table, B_Linear
//! groups the region's 1024 chunks into 16 buckets of 64 and zstd-compresses each bucket
//! as one frame. Chunk payloads inside a bucket are the plain, uncompressed NBT bytes —
//! byte-for-byte what the Anvil writer hands to zlib — so the two containers carry
//! identical worlds and a converter can move between them without touching NBT.
//!
//! Server support: this is a Leaf-family format. `misc.region-format.format-name:
//! B_LINEAR` exists in **Leaf 1.21.11 (June 2026 builds) and newer, plus all 26.x**.
//! Older Leaf (1.21.8 and below), Paper, Purpur and the vanilla Java client cannot open
//! these worlds.
//!
//! Wire format (all integers big-endian), mirroring Leaf's
//! `me.earthme.luminol.data.BufferedLinearRegionFile`:
//!
//! ```text
//! [0,8)     i64  superblock = -0x2008_1225_0269
//! [8]       u8   version    = 0x03
//! [9]       u8   zstd level (informational; Leaf ignores it on read)
//! [10,14)   u32  hash seed  = 0x0721 (informational for Leaf, which hardcodes the
//!                             same seed; region_converter does read this field)
//! [14,142)  16 × u64 absolute offset of each bucket record, 0 = bucket absent
//! [142,EOF) bucket records: i32 rawLen | i32 compressedLen | zstd frame
//! ```
//!
//! A decompressed bucket is exactly 64 slots in ascending chunk index. Each slot is an
//! `i32` length (0 = chunk absent) followed by that many bytes of section:
//! `i32 nbtLen | i64 timestampMillis | u32 xxh32(nbt, seed 0x0721) | nbt`.
//!
//! Leaf verifies the superblock, the version byte and every chunk's xxh32 strictly; it
//! ignores the timestamp and the two informational header fields. There is no footer.

use rayon::prelude::*;
use std::io::Write;
use std::path::{Path, PathBuf};
use xxhash_rust::xxh32::xxh32;

/// File magic shared by every B_Linear version.
const SUPERBLOCK: i64 = -0x0000_2008_1225_0269;
/// Bucketed layout. Version 2 was a single whole-region zstd stream.
const VERSION: u8 = 3;
/// Leaf hardcodes this seed when verifying chunk hashes and ignores the header field,
/// so a writer that hashes with anything else produces chunks Leaf refuses to load.
const HASH_SEED: u32 = 0x0721;
const BUCKET_COUNT: usize = 16;
const BUCKET_SIZE: usize = 64;
const CHUNKS_PER_REGION: usize = BUCKET_COUNT * BUCKET_SIZE;
const HEADER_SIZE: u64 = 14;
const DATA_START: u64 = HEADER_SIZE + (BUCKET_COUNT as u64) * 8;

/// Accumulates one region's chunk NBT, then writes it out as a B_Linear v3 file.
///
/// Buffering the whole region is inherent to the format: buckets are compressed as
/// units and chunks arrive in hash-map order, so no bucket is known to be complete
/// until the region is. Peak cost is the region's raw NBT, released bucket by bucket
/// as [`finish`](Self::finish) compresses.
pub(crate) struct BlinearRegionWriter {
    slots: Vec<Option<Vec<u8>>>,
    level: i32,
    out_path: PathBuf,
}

impl BlinearRegionWriter {
    /// Prepare a writer for `world_dir/region/r.{region_x}.{region_z}.b_linear`.
    pub(crate) fn create(
        world_dir: &Path,
        region_x: i32,
        region_z: i32,
        level: i32,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let region_dir = world_dir.join("region");
        std::fs::create_dir_all(&region_dir)?;
        Ok(Self {
            slots: (0..CHUNKS_PER_REGION).map(|_| None).collect(),
            level,
            out_path: region_dir.join(format!("r.{}.{}.b_linear", region_x, region_z)),
        })
    }

    /// Stage one chunk's uncompressed NBT at its region-local coordinates.
    ///
    /// Same signature and index convention as `fastanvil::Region::write_chunk`, so the
    /// two containers are drop-in alternatives at the call site.
    pub(crate) fn write_chunk(
        &mut self,
        chunk_x: usize,
        chunk_z: usize,
        nbt: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let index = (chunk_x & 31) + (chunk_z & 31) * 32;
        self.slots[index] = Some(nbt.to_vec());
        Ok(())
    }

    /// Compress the staged chunks and publish the region file.
    ///
    /// The file is built in memory, written to a dot-prefixed temp beside its
    /// destination and renamed into place, so a reader either sees the previous file or
    /// the complete new one. The temp name deliberately does not match `r.*.b_linear`:
    /// Meld's export progress counts regions by globbing for that pattern.
    pub(crate) fn finish(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let out_path = std::mem::take(&mut self.out_path);
        let bytes = self.encode()?;

        let dir = out_path
            .parent()
            .ok_or("b_linear output path has no parent directory")?;
        let file_name = out_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("b_linear output path has no file name")?;
        let tmp_path = dir.join(format!(
            ".{}.tmp-{}-{}",
            file_name,
            std::process::id(),
            next_temp_counter()
        ));

        // Scope the handle so Windows lets the rename through.
        {
            let mut tmp = std::fs::File::create(&tmp_path)?;
            tmp.write_all(&bytes)?;
            tmp.flush()?;
        }

        if let Err(rename_err) = std::fs::rename(&tmp_path, &out_path) {
            // Windows refuses a rename onto an existing file; clear and retry once,
            // then make sure a failed publish never strands the temp.
            if out_path.exists()
                && std::fs::remove_file(&out_path).is_ok()
                && std::fs::rename(&tmp_path, &out_path).is_ok()
            {
                return Ok(());
            }
            let _ = std::fs::remove_file(&tmp_path);
            return Err(Box::new(rename_err));
        }

        Ok(())
    }

    /// Build the complete file image.
    ///
    /// Buckets are independent, so they compress in parallel: this runs on the single
    /// background flush thread during streaming eviction, where a slow region write
    /// stalls the whole eviction queue and pushes the generator's peak memory up.
    /// Each bucket's raw chunk NBT is dropped as soon as its frame exists.
    fn encode(&mut self) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let timestamp_millis = now_millis();
        let level = self.level.clamp(1, 22);

        // Hand each bucket its own 64 slots so the raw NBT can be released per bucket
        // rather than being held for the whole region.
        let mut rest = std::mem::take(&mut self.slots);
        let mut buckets: Vec<Vec<Option<Vec<u8>>>> = Vec::with_capacity(BUCKET_COUNT);
        for _ in 0..BUCKET_COUNT {
            let tail = rest.split_off(BUCKET_SIZE);
            buckets.push(rest);
            rest = tail;
        }

        let frames: Vec<BucketFrame> = buckets
            .into_par_iter()
            .map(|slots| encode_bucket(slots, timestamp_millis, level))
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(1 << 20);
        out.extend_from_slice(&SUPERBLOCK.to_be_bytes());
        out.push(VERSION);
        out.push(level as u8);
        out.extend_from_slice(&HASH_SEED.to_be_bytes());
        out.extend_from_slice(&[0u8; BUCKET_COUNT * 8]);
        debug_assert_eq!(out.len() as u64, DATA_START);

        let mut offsets = [0u64; BUCKET_COUNT];
        for (bucket_index, frame) in frames.into_iter().enumerate() {
            let Some((raw_len, compressed)) = frame else {
                continue; // empty bucket: its offset stays 0
            };
            offsets[bucket_index] = out.len() as u64;
            out.extend_from_slice(
                &i32::try_from(raw_len)
                    .map_err(|_| "b_linear bucket exceeds the 2 GiB raw size limit")?
                    .to_be_bytes(),
            );
            out.extend_from_slice(
                &i32::try_from(compressed.len())
                    .map_err(|_| "b_linear bucket exceeds the 2 GiB compressed size limit")?
                    .to_be_bytes(),
            );
            out.extend_from_slice(&compressed);
        }

        // Backfill the offset table now that every bucket has a home.
        for (bucket_index, offset) in offsets.iter().enumerate() {
            let at = HEADER_SIZE as usize + bucket_index * 8;
            out[at..at + 8].copy_from_slice(&offset.to_be_bytes());
        }

        Ok(out)
    }
}

/// One framed bucket: its raw length and the zstd frame holding it. `None` marks a
/// bucket whose 64 slots are all empty, which is written as an absent offset.
type BucketFrame = Option<(usize, Vec<u8>)>;

/// Frame one bucket, compressing its 64 slots into a single zstd frame.
fn encode_bucket(
    slots: Vec<Option<Vec<u8>>>,
    timestamp_millis: i64,
    level: i32,
) -> Result<BucketFrame, Box<dyn std::error::Error + Send + Sync>> {
    // An all-empty bucket is represented by its offset staying 0. Leaf skips those
    // without reading anything, so emitting a record would be wasted.
    if slots.iter().all(Option::is_none) {
        return Ok(None);
    }

    let mut raw: Vec<u8> = Vec::with_capacity(1 << 19);
    for slot in slots {
        match slot {
            Some(nbt) => {
                let nbt_len = i32::try_from(nbt.len())
                    .map_err(|_| "chunk NBT exceeds the 2 GiB b_linear section limit")?;
                // Section length covers the 16-byte section header plus the NBT.
                raw.extend_from_slice(&(nbt_len + 16).to_be_bytes());
                raw.extend_from_slice(&nbt_len.to_be_bytes());
                raw.extend_from_slice(&timestamp_millis.to_be_bytes());
                raw.extend_from_slice(&xxh32(&nbt, HASH_SEED).to_be_bytes());
                raw.extend_from_slice(&nbt);
            }
            None => raw.extend_from_slice(&0i32.to_be_bytes()),
        }
    }

    let compressed = zstd::bulk::compress(&raw, level)?;
    Ok(Some((raw.len(), compressed)))
}

/// Chunk save time. Leaf reads this field and currently ignores it; converters treat
/// values below 10^10 as seconds and rescale them, so milliseconds is the safe unit.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Keeps concurrent region writes from colliding on a temp name within one process.
fn next_temp_counter() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Minimal reader used only by tests to prove what the writer emitted. Mirrors the
/// checks Leaf performs: superblock, version, per-chunk hash.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
    assert!(
        bytes.len() >= DATA_START as usize,
        "file shorter than header"
    );
    assert_eq!(
        i64::from_be_bytes(bytes[0..8].try_into().unwrap()),
        SUPERBLOCK
    );
    assert_eq!(bytes[8], VERSION);
    let seed = u32::from_be_bytes(bytes[10..14].try_into().unwrap());
    assert_eq!(seed, HASH_SEED);

    let mut slots: Vec<Option<Vec<u8>>> = (0..CHUNKS_PER_REGION).map(|_| None).collect();
    for bucket_index in 0..BUCKET_COUNT {
        let at = HEADER_SIZE as usize + bucket_index * 8;
        let offset = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        if offset == 0 {
            continue;
        }
        assert!(offset >= DATA_START as usize, "offset inside header");
        let raw_len = i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let comp_len =
            i32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let payload = &bytes[offset + 8..offset + 8 + comp_len];
        let raw = zstd::bulk::decompress(payload, raw_len).expect("bucket decompresses");
        assert_eq!(raw.len(), raw_len);

        let mut cursor = 0usize;
        for slot in 0..BUCKET_SIZE {
            let n = i32::from_be_bytes(raw[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            if n == 0 {
                continue;
            }
            let section = &raw[cursor..cursor + n];
            cursor += n;
            let nbt_len = i32::from_be_bytes(section[0..4].try_into().unwrap()) as usize;
            let hash = u32::from_be_bytes(section[12..16].try_into().unwrap());
            let nbt = &section[16..16 + nbt_len];
            assert_eq!(n, nbt_len + 16, "section length covers header + payload");
            assert_eq!(xxh32(nbt, seed), hash, "chunk hash matches");
            slots[bucket_index * BUCKET_SIZE + slot] = Some(nbt.to_vec());
        }
        assert_eq!(cursor, raw.len(), "bucket fully consumed");
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer(level: i32) -> BlinearRegionWriter {
        BlinearRegionWriter {
            slots: (0..CHUNKS_PER_REGION).map(|_| None).collect(),
            level,
            out_path: PathBuf::from("r.0.0.b_linear"),
        }
    }

    #[test]
    fn empty_region_is_header_and_zero_table() {
        let bytes = writer(6).encode().unwrap();
        assert_eq!(bytes.len() as u64, DATA_START);
        assert_eq!(bytes.len(), 142);
        assert!(decode(&bytes).iter().all(Option::is_none));
    }

    #[test]
    fn chunks_round_trip_byte_for_byte() {
        let mut w = writer(6);
        let payloads: Vec<(usize, usize, Vec<u8>)> = vec![
            (0, 0, b"first-chunk".to_vec()),
            (31, 0, vec![0xAB; 5000]),
            (0, 1, b"row-one".to_vec()),
            (17, 13, b"middle".to_vec()),
            (31, 31, b"last-chunk".to_vec()),
        ];
        for (x, z, nbt) in &payloads {
            w.write_chunk(*x, *z, nbt).unwrap();
        }

        let slots = decode(&w.encode().unwrap());
        for (x, z, nbt) in &payloads {
            assert_eq!(slots[x + z * 32].as_deref(), Some(nbt.as_slice()));
        }
        assert_eq!(slots.iter().filter(|s| s.is_some()).count(), payloads.len());
    }

    #[test]
    fn bucket_boundaries_land_in_the_right_bucket() {
        // Bucket b covers chunk indices [64b, 64b+64) — two full Z rows.
        let mut w = writer(6);
        w.write_chunk(63 % 32, 63 / 32, b"end-of-bucket-0").unwrap();
        w.write_chunk(64 % 32, 64 / 32, b"start-of-bucket-1")
            .unwrap();
        let bytes = w.encode().unwrap();

        let offsets: Vec<u64> = (0..BUCKET_COUNT)
            .map(|i| {
                let at = HEADER_SIZE as usize + i * 8;
                u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap())
            })
            .collect();
        assert!(offsets[0] > 0 && offsets[1] > 0, "both buckets written");
        assert!(
            offsets[2..].iter().all(|&o| o == 0),
            "untouched buckets stay absent"
        );

        let slots = decode(&bytes);
        assert_eq!(slots[63].as_deref(), Some(b"end-of-bucket-0".as_slice()));
        assert_eq!(slots[64].as_deref(), Some(b"start-of-bucket-1".as_slice()));
    }

    #[test]
    fn full_region_writes_every_bucket() {
        let mut w = writer(3);
        for index in 0..CHUNKS_PER_REGION {
            let nbt = format!("chunk-{index}").into_bytes();
            w.write_chunk(index % 32, index / 32, &nbt).unwrap();
        }
        let slots = decode(&w.encode().unwrap());
        for (index, slot) in slots.iter().enumerate() {
            assert_eq!(slot.as_deref(), Some(format!("chunk-{index}").as_bytes()));
        }
    }

    #[test]
    fn header_fields_match_the_leaf_contract() {
        let bytes = writer(9).encode().unwrap();
        assert_eq!(
            &bytes[0..8],
            &[0xFF, 0xFF, 0xDF, 0xF7, 0xED, 0xDA, 0xFD, 0x97],
            "superblock bytes"
        );
        assert_eq!(bytes[8], 3, "version byte");
        assert_eq!(bytes[9], 9, "compression level is recorded verbatim");
        assert_eq!(&bytes[10..14], &[0x00, 0x00, 0x07, 0x21], "hash seed");
    }

    #[test]
    fn publishes_atomically_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = BlinearRegionWriter::create(dir.path(), -3, 7, 6).unwrap();
        w.write_chunk(1, 2, b"payload").unwrap();
        w.finish().unwrap();

        let region_dir = dir.path().join("region");
        let published = region_dir.join("r.-3.7.b_linear");
        assert!(
            published.is_file(),
            "region file published under its final name"
        );

        let leftovers: Vec<_> = std::fs::read_dir(&region_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "r.-3.7.b_linear")
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files left behind: {leftovers:?}"
        );

        let slots = decode(&std::fs::read(&published).unwrap());
        assert_eq!(slots[1 + 2 * 32].as_deref(), Some(b"payload".as_slice()));
    }
}
