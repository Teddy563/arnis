//! EXPERIMENTAL: Leaf / Luminol **B_Linear v3** region container (`r.X.Z.b_linear`),
//! selected by `--region-format blinear`. Off by default; Anvil `.mca` stays the format.
//!
//! The same uncompressed chunk NBT the Anvil writer hands to zlib, framed differently:
//! a region's 1024 chunks are grouped into 16 buckets of 64 and each bucket is one zstd
//! frame. Readable by Leaf 1.21.11 (June 2026 builds) and newer and all Leaf 26.x
//! (`misc.region-format.format-name: B_LINEAR`), and by Meld's `region-convert`.
//! Paper, older Leaf and the vanilla client cannot open it.
//!
//! Wire format, all integers big-endian, as read by Leaf's
//! `me.earthme.luminol.data.BufferedLinearRegionFile` (layout ported from Meld's fork,
//! `src/world_editor/blinear.rs`, and its `region-convert` reader):
//!
//! ```text
//! [0,8)     i64  superblock = -0x2008_1225_0269
//! [8]       u8   version    = 3
//! [9]       u8   zstd level (informational)
//! [10,14)   u32  xxh32 seed = 0x0721 (Leaf hardcodes it; region-convert reads it)
//! [14,142)  16 x u64 absolute offset of each bucket record, 0 = bucket absent
//! [142,EOF) bucket records: i32 rawLen | i32 compressedLen | zstd frame
//! ```
//!
//! A decompressed bucket is 64 slots in ascending chunk index (`x + z * 32`), each an
//! `i32` length (0 = absent) and then that many bytes:
//! `i32 nbtLen | i64 timestampMillis | u32 xxh32(nbt, 0x0721) | nbt`.
//! Leaf checks the superblock, the version and every chunk hash. No footer.

use rayon::prelude::*;
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const SUPERBLOCK: i64 = -0x0000_2008_1225_0269;
const VERSION: u8 = 3;
const HASH_SEED: u32 = 0x0721;
const BUCKET_COUNT: usize = 16;
const BUCKET_SIZE: usize = 64;
const CHUNKS_PER_REGION: usize = BUCKET_COUNT * BUCKET_SIZE;
const HEADER_SIZE: usize = 14;
const DATA_START: usize = HEADER_SIZE + BUCKET_COUNT * 8;

/// Holds one region's chunk NBT until [`finish`](Self::finish). Buckets compress as
/// units, so nothing can be written before the region is complete.
pub(crate) struct BlinearRegionWriter {
    chunks: Vec<Option<Vec<u8>>>,
    level: i32,
    out_path: PathBuf,
}

impl BlinearRegionWriter {
    pub(crate) fn create(
        world_dir: &Path,
        region_x: i32,
        region_z: i32,
        level: i32,
    ) -> Result<Self, BoxError> {
        let region_dir = world_dir.join("region");
        std::fs::create_dir_all(&region_dir)?;
        Ok(Self {
            chunks: vec![None; CHUNKS_PER_REGION],
            level: level.clamp(1, 22),
            out_path: region_dir.join(format!("r.{region_x}.{region_z}.b_linear")),
        })
    }

    /// Same index convention as `fastanvil::Region::write_chunk`.
    pub(crate) fn write_chunk(&mut self, chunk_x: usize, chunk_z: usize, nbt: &[u8]) {
        self.chunks[(chunk_x & 31) + (chunk_z & 31) * 32] = Some(nbt.to_vec());
    }

    /// Compress and publish via a sibling temp + rename, so a reader never sees a torn
    /// file. The temp (`r.X.Z.tmp<pid>`) does not match `r.*.b_linear`.
    pub(crate) fn finish(self) -> Result<(), BoxError> {
        let bytes = encode(&self.chunks, self.level, now_millis())?;
        crate::world_utils::replace_file_atomically(&self.out_path, &bytes)?;
        Ok(())
    }
}

fn encode(chunks: &[Option<Vec<u8>>], level: i32, timestamp: i64) -> Result<Vec<u8>, BoxError> {
    // Buckets are independent: compress them in parallel (matters on the single
    // background flush thread, where one slow region stalls eviction).
    let frames = chunks
        .par_chunks(BUCKET_SIZE)
        .map(|bucket| encode_bucket(bucket, level, timestamp))
        .collect::<Result<Vec<_>, BoxError>>()?;

    let mut out = Vec::with_capacity(
        DATA_START
            + frames
                .iter()
                .flatten()
                .map(|(_, c)| 8 + c.len())
                .sum::<usize>(),
    );
    out.extend_from_slice(&SUPERBLOCK.to_be_bytes());
    out.push(VERSION);
    out.push(level as u8);
    out.extend_from_slice(&HASH_SEED.to_be_bytes());
    out.resize(DATA_START, 0);
    for (i, frame) in frames.into_iter().enumerate() {
        let Some((raw_len, compressed)) = frame else {
            continue; // all 64 slots empty: offset stays 0
        };
        let at = HEADER_SIZE + i * 8;
        let here = out.len() as u64;
        out[at..at + 8].copy_from_slice(&here.to_be_bytes());
        out.extend_from_slice(&i32::try_from(raw_len)?.to_be_bytes());
        out.extend_from_slice(&i32::try_from(compressed.len())?.to_be_bytes());
        out.extend_from_slice(&compressed);
    }
    Ok(out)
}

/// `(raw length, zstd frame)` of one bucket, or `None` when it holds no chunk.
fn encode_bucket(
    bucket: &[Option<Vec<u8>>],
    level: i32,
    timestamp: i64,
) -> Result<Option<(usize, Vec<u8>)>, BoxError> {
    if bucket.iter().all(Option::is_none) {
        return Ok(None);
    }
    let raw_len: usize = bucket
        .iter()
        .map(|c| 4 + c.as_ref().map_or(0, |nbt| 16 + nbt.len()))
        .sum();
    let mut raw = Vec::with_capacity(raw_len);
    for chunk in bucket {
        let Some(nbt) = chunk else {
            raw.extend_from_slice(&0i32.to_be_bytes());
            continue;
        };
        let nbt_len = i32::try_from(nbt.len())
            .ok()
            .filter(|n| *n <= i32::MAX - 16)
            .ok_or("chunk NBT exceeds the b_linear section limit")?;
        raw.extend_from_slice(&(nbt_len + 16).to_be_bytes());
        raw.extend_from_slice(&nbt_len.to_be_bytes());
        raw.extend_from_slice(&timestamp.to_be_bytes());
        raw.extend_from_slice(&xxh32(nbt, HASH_SEED).to_be_bytes());
        raw.extend_from_slice(nbt);
    }
    Ok(Some((raw_len, zstd::bulk::compress(&raw, level)?)))
}

/// Leaf ignores the timestamp; converters treat values below 10^10 as seconds, so
/// write milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// XXH32 (one-shot), the chunk checksum Leaf verifies. Inline rather than a crate:
/// it is the only hash the format needs.
fn xxh32(input: &[u8], seed: u32) -> u32 {
    const P1: u32 = 0x9E37_79B1;
    const P2: u32 = 0x85EB_CA77;
    const P3: u32 = 0xC2B2_AE3D;
    const P4: u32 = 0x27D4_EB2F;
    const P5: u32 = 0x1656_67B1;
    let read = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let round = |acc: u32, lane: u32| {
        acc.wrapping_add(lane.wrapping_mul(P2))
            .rotate_left(13)
            .wrapping_mul(P1)
    };

    let mut stripes = input.chunks_exact(16);
    let mut h = if input.len() >= 16 {
        let mut v = [
            seed.wrapping_add(P1).wrapping_add(P2),
            seed.wrapping_add(P2),
            seed,
            seed.wrapping_sub(P1),
        ];
        for stripe in &mut stripes {
            for (i, acc) in v.iter_mut().enumerate() {
                *acc = round(*acc, read(&stripe[i * 4..]));
            }
        }
        v[0].rotate_left(1)
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18))
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(input.len() as u32);

    let mut tail = stripes.remainder();
    while tail.len() >= 4 {
        h = h
            .wrapping_add(read(tail).wrapping_mul(P3))
            .rotate_left(17)
            .wrapping_mul(P4);
        tail = &tail[4..];
    }
    for &byte in tail {
        h = h
            .wrapping_add(u32::from(byte).wrapping_mul(P5))
            .rotate_left(11)
            .wrapping_mul(P1);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^ (h >> 16)
}

/// Reader for tests: makes the checks Leaf makes (superblock, version, chunk hash)
/// and returns the 1024 slots' NBT.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
    let be32 = |b: &[u8], at: usize| i32::from_be_bytes(b[at..at + 4].try_into().unwrap());
    assert_eq!(&bytes[0..8], &SUPERBLOCK.to_be_bytes());
    assert_eq!(bytes[8], VERSION);
    let seed = u32::from_be_bytes(bytes[10..14].try_into().unwrap());
    let mut slots = vec![None; CHUNKS_PER_REGION];
    for b in 0..BUCKET_COUNT {
        let at = HEADER_SIZE + b * 8;
        let offset = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        if offset == 0 {
            continue;
        }
        let raw_len = be32(bytes, offset) as usize;
        let comp_len = be32(bytes, offset + 4) as usize;
        let raw = zstd::bulk::decompress(&bytes[offset + 8..offset + 8 + comp_len], raw_len)
            .expect("bucket decompresses");
        assert_eq!(raw.len(), raw_len);
        let mut cursor = 0;
        for slot in 0..BUCKET_SIZE {
            let n = be32(&raw, cursor) as usize;
            cursor += 4;
            if n == 0 {
                continue;
            }
            let nbt_len = be32(&raw, cursor) as usize;
            assert_eq!(n, nbt_len + 16);
            let hash = u32::from_be_bytes(raw[cursor + 12..cursor + 16].try_into().unwrap());
            let nbt = &raw[cursor + 16..cursor + n];
            assert_eq!(xxh32(nbt, seed), hash, "chunk hash");
            slots[b * BUCKET_SIZE + slot] = Some(nbt.to_vec());
            cursor += n;
        }
        assert_eq!(cursor, raw.len(), "bucket fully consumed");
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxh32_matches_reference_vectors() {
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        assert_eq!(
            xxh32(b"Nobody inspects the spammish repetition", 0),
            0xE229_3B2F
        );
    }

    #[test]
    fn header_matches_the_leaf_contract() {
        let bytes = encode(&vec![None; CHUNKS_PER_REGION], 9, 0).unwrap();
        assert_eq!(bytes.len(), 142, "empty region is header + zero table");
        assert_eq!(
            &bytes[0..8],
            &[0xFF, 0xFF, 0xDF, 0xF7, 0xED, 0xDA, 0xFD, 0x97]
        );
        assert_eq!(bytes[8], 3);
        assert_eq!(bytes[9], 9);
        assert_eq!(&bytes[10..14], &[0x00, 0x00, 0x07, 0x21]);
        assert!(bytes[14..].iter().all(|&b| b == 0));
    }

    #[test]
    fn chunks_round_trip_into_their_slots() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = BlinearRegionWriter::create(dir.path(), -3, 7, 6).unwrap();
        let payloads = [
            (9, 0, vec![1u8; 3]),
            (0, 0, b"first".to_vec()),
            (31, 1, vec![0xAB; 5000]), // index 63, last slot of bucket 0
            (0, 2, b"bucket one".to_vec()),
            (31, 31, b"last".to_vec()),
        ];
        for (x, z, nbt) in &payloads {
            w.write_chunk(*x, *z, nbt);
        }
        w.finish().unwrap();

        let region = dir.path().join("region");
        let names: Vec<_> = std::fs::read_dir(&region)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["r.-3.7.b_linear"], "published, no temp left");

        let bytes = std::fs::read(region.join("r.-3.7.b_linear")).unwrap();
        let slots = decode(&bytes);
        for (x, z, nbt) in &payloads {
            assert_eq!(slots[x + z * 32].as_ref(), Some(nbt));
        }
        assert_eq!(slots.iter().flatten().count(), payloads.len());
        let offset = |b: usize| &bytes[HEADER_SIZE + b * 8..HEADER_SIZE + b * 8 + 8];
        assert!((2..BUCKET_COUNT - 1).all(|b| offset(b) == [0; 8]));
    }
}
