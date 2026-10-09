//! Pre-generated LOD data for the [Distant Horizons](https://gitlab.com/distant-horizons-team/distant-horizons)
//! mod (Java worlds only).
//!
//! Distant Horizons keeps a level's LODs in `DistantHorizons.sqlite` in that
//! level's data folder (`ServerLevelWrapper.getMcSaveFolder`, the level's
//! `DimensionDataStorage` folder): `<world>/data/` for the overworld before
//! Minecraft 26.1, `<world>/dimensions/minecraft/overworld/data/` from 26.1.
//! Left alone it builds them as you fly around, or with its world generator,
//! both reading back the region files Arnis just wrote. This pass does that
//! once, after the world is written: it reads the region files back and writes
//! the block-detail rows DH would write for the same chunks, flagged so DH
//! builds the coarser levels itself.
//!
//! Everything here follows DH 3.3.3 (core `b02c66d7`). Its 1.21.1 and 26.2
//! builds carry byte-identical core classes and SQL scripts, and serialise
//! blocks and biomes the same way, so one database serves both; only the
//! folder differs. 3.3.4 changes none of the storage code.
//!
//! - The schema is what DH's `DatabaseUpdater` leaves after its twelve
//!   `sqlScripts/` files, and the `Schema` table lists all twelve, so DH runs
//!   none of them again.
//! - A `FullData` row is one 64x64-column `FullDataSourceV2` at detail level 0,
//!   keyed by `floor(block / 64)`. Its blobs are encoded as
//!   `FullDataSourceV2DTO` writes them, each its own zstd frame
//!   (`CompressionMode` 4, `Z_STD_BLOCK`, DH's default).
//! - The coarser levels 1 to 8 are written too, merged from the rows below as
//!   DH's `FullDataSourceV2.updateFromOneBelowDetailLevel` merges them. DH
//!   draws anything past a few hundred blocks from those levels
//!   (`LodQuadTree.calcDetailLevelFromDistance`) and shows nothing where a
//!   level has no row; its own propagator builds them a few rows a second,
//!   so a large area would stay blank for hours.
//! - `ApplyToParent = 1` on the block-detail rows, the flag DH's own chunk
//!   path sets, still lets DH redo the coarser levels in the background.
//! - Columns read from a full chunk are marked `LIGHT`, the last generation
//!   step, so DH never queues them for world generation. Columns of chunks
//!   that do not exist stay `EMPTY`, which DH generates as usual.
//!
//! The pass reads the files on disk, never memory, so a section straddling the
//! edge of the area still gets its neighbours' columns, and One World pieces
//! need no coordination: the coordinator runs it once over the whole job after
//! every piece has finished, as the only writer.

use fastnbt::{ByteArray, LongArray};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::world_utils::WorldLayout;

/// Columns per side of one `FullDataSourceV2`.
const WIDTH: usize = 64;
/// `EDhApiWorldGenerationStep.EMPTY` and `.LIGHT`.
const GEN_EMPTY: u8 = 0;
const GEN_LIGHT: u8 = 9;
/// `EDhApiDataCompressionMode.Z_STD_BLOCK`.
const COMPRESSION_ZSTD: u8 = 4;
/// `FullDataSourceV2DTO.DATA_FORMAT.V2_LATEST`.
const DATA_FORMAT_V2: u8 = 2;
/// The level `DhDataOutputStream` compresses with.
const ZSTD_LEVEL: i32 = 3;
/// `FullDataPointIdMap.BLOCK_STATE_SEPARATOR_STRING`.
const PAIR_SEPARATOR: &str = "_DH-BSW_";
/// `BlockStateWrapper.STATE_STRING_SEPARATOR` and `AIR_STRING`.
const STATE_SEPARATOR: &str = "_STATE_";
const AIR: &str = "AIR";
const DEFAULT_BIOME: &str = "minecraft:plains";
/// `FullDataSourceProviderV2.ROOT_SECTION_DETAIL_LEVEL`, as a `DetailLevel`.
const TOP_LEVEL: u8 = 8;

/// The tables, columns (in order) and indexes DH 3.3.3's update scripts leave
/// behind, checked against a database DH created itself.
const SCHEMA: &str = r#"
CREATE TABLE Schema (
    SchemaVersionId INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    ScriptName TEXT NOT NULL UNIQUE,
    AppliedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP
);
CREATE TABLE Legacy_FullData_V1 (
    DhSectionPos TEXT NOT NULL PRIMARY KEY,
    DataDetailLevel TINYINT NULL,
    Checksum INT NULL,
    DataVersion BIGINT NULL,
    WorldGenStep NVARCHAR(32) NULL,
    DataType NVARCHAR(48) NULL,
    BinaryDataFormatVersion TINYINT NULL,
    Data BLOB NULL,
    CreatedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP,
    LastModifiedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP,
    MigrationFailed BIT NOT NULL DEFAULT 0
);
CREATE TABLE FullData (
    DetailLevel TINYINT NOT NULL,
    PosX INT NOT NULL,
    PosZ INT NOT NULL,
    MinY INT NOT NULL,
    DataChecksum INT NOT NULL,
    Data BLOB NULL,
    ColumnGenerationStep BLOB NULL,
    ColumnWorldCompressionMode BLOB NULL,
    Mapping BLOB NULL,
    DataFormatVersion TINYINT NULL,
    CompressionMode TINYINT NULL,
    ApplyToParent BIT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    ApplyToChildren BIT NULL,
    NorthAdjData BLOB NULL,
    SouthAdjData BLOB NULL,
    EastAdjData BLOB NULL,
    WestAdjData BLOB NULL,
    Regenerate bit NULL,
    PRIMARY KEY (DetailLevel, PosX, PosZ)
);
CREATE TABLE ChunkHash (
    ChunkPosX INT NOT NULL,
    ChunkPosZ INT NOT NULL,
    ChunkHash INT NOT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    PRIMARY KEY (ChunkPosX, ChunkPosZ)
);
CREATE TABLE BeaconBeam (
    BlockPosX INT NOT NULL,
    BlockPosY INT NOT NULL,
    BlockPosZ INT NOT NULL,
    ColorR INT NOT NULL,
    ColorG INT NOT NULL,
    ColorB INT NOT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    PRIMARY KEY (BlockPosX, BlockPosY, BlockPosZ)
);
CREATE INDEX FullDataUpdatedIndex on FullData (ApplyToParent) where ApplyToParent = 1;
CREATE INDEX FullDataApplyToChildrenIndex on FullData (ApplyToChildren) where ApplyToChildren = 1;
CREATE INDEX FullDataRegenerateIndex on FullData (Regenerate) where Regenerate = 1;
"#;

/// `sqlScripts/scriptList.txt`, as DH records each in `Schema.ScriptName`.
const SCRIPTS: [&str; 12] = [
    "0010-sqlite-createInitialDataTables.sql",
    "0020-sqlite-createFullDataSourceV2Tables.sql",
    "0030-sqlite-changeTableJournaling.sql",
    "0031-sqlite-useSqliteWalJournaling.sql",
    "0040-sqlite-removeRenderCache.sql",
    "0050-sqlite-addApplyToParentIndex.sql",
    "0060-sqlite-createChunkHashTable.sql",
    "0070-sqlite-createBeaconBeamTable.sql",
    "0080-sqlite-addApplyToChildrenColumn.sql",
    "0090-sqlite-addAdjacentFullDataColumns.sql",
    "0100-sqlite-deleteLowDetailDataForRegen.sql",
    "0110-sqlite-addApplyToParentIndex.sql",
];

/// The `FullData` columns a row needs; a database from an older DH lacks some.
const NEEDED_COLUMNS: [&str; 6] = [
    "ApplyToChildren",
    "NorthAdjData",
    "SouthAdjData",
    "EastAdjData",
    "WestAdjData",
    "Regenerate",
];

/// Where DH keeps a world's overworld LODs, by the layout `level.dat` declares.
pub fn database_path(world_dir: &Path) -> PathBuf {
    database_in(WorldLayout::of(world_dir), world_dir)
}

fn database_in(layout: WorldLayout, world_dir: &Path) -> PathBuf {
    layout
        .overworld_dir(world_dir)
        .join("data")
        .join("DistantHorizons.sqlite")
}

/// The inclusive block rectangle `[min_x, min_z, max_x, max_z]` of every
/// overworld `.mca` region file, for a pass over a whole existing world.
pub fn region_extent(world_dir: &Path) -> Option<[i32; 4]> {
    let dir = WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region");
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (x, z) = name
                .strip_prefix("r.")?
                .strip_suffix(".mca")?
                .split_once('.')?;
            Some((x.parse::<i32>().ok()?, z.parse::<i32>().ok()?))
        })
        .map(|(x, z)| [x * 512, z * 512, x * 512 + 511, z * 512 + 511])
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}

/// What one pass wrote.
#[derive(Debug, Default)]
pub struct Stats {
    pub sections: u64,
    pub columns: u64,
    /// Rows of the coarser levels.
    pub coarser: u64,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} sections ({} columns) and {} coarser sections",
            self.sections, self.columns, self.coarser
        )
    }
}

/// `--dh-lod` after a world (or a One World job) is written: the LODs for
/// `rect`, reported on the console. A failure is a warning, as the world
/// itself is fine.
pub fn run(world_dir: &Path, rect: &crate::coordinate_system::cartesian::XZBBox, fresh: bool) {
    let rect = [rect.min_x(), rect.min_z(), rect.max_x(), rect.max_z()];
    match write_lods(world_dir, rect, fresh) {
        Ok(s) => println!(
            "Distant Horizons LODs: {s} written to {}.",
            database_path(world_dir).display()
        ),
        Err(e) => eprintln!("Warning: Failed to write the Distant Horizons LODs: {e}"),
    }
}

/// Writes DH's block-detail LODs for every 64-block section touching the
/// inclusive block rectangle `[min_x, min_z, max_x, max_z]`, from the region
/// files in `world_dir`. `fresh` drops the database first, so a regenerated
/// world never shows LODs of what it replaced; otherwise the sections are
/// replaced in place and the rest of the database is kept.
pub fn write_lods(world_dir: &Path, rect: [i32; 4], fresh: bool) -> Result<Stats, String> {
    let layout = WorldLayout::of(world_dir);
    let db_path = database_in(layout, world_dir);
    // Minecraft 26.1+ moves a legacy world's region files into `dimensions/`
    // but leaves `data/DistantHorizons.sqlite` behind, and DH then starts an
    // empty database there. A legacy world gets the database under that name
    // too, which the upgrade keeps (checked against the 26.2 server).
    let upgraded =
        (layout == WorldLayout::Legacy).then(|| database_in(WorldLayout::Dimensions, world_dir));
    if fresh {
        remove_database(&db_path)?;
    }
    if let Some(path) = &upgraded {
        remove_database(path)?;
    }
    let conn = Mutex::new(open_database(&db_path)?);
    let region_dir = layout.overworld_dir(world_dir).join("region");

    let [min_x, min_z, max_x, max_z] = rect;
    let regions: Vec<(i32, i32)> = (min_x.div_euclid(512)..=max_x.div_euclid(512))
        .flat_map(|rx| (min_z.div_euclid(512)..=max_z.div_euclid(512)).map(move |rz| (rx, rz)))
        .collect();
    // The 8x8 sections of region `r` along one axis that touch `min..=max`.
    let sections = |min: i32, max: i32, r: i32| {
        min.div_euclid(64).max(r * 8)..=max.div_euclid(64).min(r * 8 + 7)
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);

    // SQLite takes one writer: regions are read and encoded in parallel, and
    // each one's rows go in under the lock as a single transaction.
    let totals = Mutex::new(Stats::default());
    let written = Mutex::new(Vec::new());
    regions.par_iter().try_for_each(|&(rx, rz)| {
        let path = region_dir.join(format!("r.{rx}.{rz}.mca"));
        // Minecraft leaves empty region files around; they hold no chunks.
        let Some(file) = File::open(&path)
            .ok()
            .filter(|f| f.metadata().is_ok_and(|m| m.len() > 0))
        else {
            return Ok(());
        };
        let mut region = fastanvil::Region::from_stream(file)
            .map_err(|e| format!("{} is not a readable region file: {e:?}", path.display()))?;
        let mut rows = Vec::new();
        for sx in sections(min_x, max_x, rx) {
            for sz in sections(min_z, max_z, rz) {
                let mut source = Source::new();
                for dx in 0..4 {
                    for dz in 0..4 {
                        let (cx, cz) = ((sx * 4 + dx) as usize & 31, (sz * 4 + dz) as usize & 31);
                        if let Some(chunk) = region
                            .read_chunk(cx, cz)
                            .ok()
                            .flatten()
                            .and_then(|data| Chunk::parse(&data))
                        {
                            source.add_chunk(&chunk, dx as usize * 16, dz as usize * 16);
                        }
                    }
                }
                if source.columns > 0 {
                    rows.push((sx, sz, source.columns, source.encode()?));
                }
            }
        }
        let mut conn = lock(&conn);
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (sx, sz, _, row) in &rows {
            upsert(&tx, 0, *sx, *sz, row, now).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn);
        let mut totals = lock(&totals);
        totals.sections += rows.len() as u64;
        totals.columns += rows.iter().map(|r| r.2 as u64).sum::<u64>();
        lock(&written).extend(rows.iter().map(|r| (r.0, r.1)));
        Ok::<(), String>(())
    })?;
    let mut totals = totals.into_inner().unwrap_or_else(|p| p.into_inner());
    totals.coarser = write_coarser_levels(&conn, written.into_inner().unwrap_or_default(), now)?;
    // Closing the last connection folds the WAL into the file before it is
    // linked.
    drop(conn);
    if let Some(path) = &upgraded {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::hard_link(&db_path, path)
            .or_else(|_| fs::copy(&db_path, path).map(drop))
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(totals)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Levels 1 to [`TOP_LEVEL`] over the block-detail sections `written`, each
/// parent merged from its four children as they now stand in the database.
/// Returns the rows written.
fn write_coarser_levels(
    conn: &Mutex<Connection>,
    mut written: Vec<(i32, i32)>,
    now: i64,
) -> Result<u64, String> {
    let mut count = 0;
    for level in 1..=TOP_LEVEL {
        // `DhSectionPos.getParentPos`: halved, rounding down.
        written = written.iter().map(|&(x, z)| (x >> 1, z >> 1)).collect();
        written.sort_unstable();
        written.dedup();
        // Batches bound the encoded rows held at once.
        for batch in written.chunks(1024) {
            let rows: Vec<(i32, i32, Row)> = batch
                .par_iter()
                .filter_map(|&(x, z)| {
                    // Updated in place, as DH updates a parent: a quarter
                    // whose child this module cannot read (another compression
                    // mode, an older format) keeps what it had.
                    let mut parent =
                        read_source(&lock(conn), level, x, z).unwrap_or_else(Source::new);
                    let mut any = false;
                    for (dx, dz) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        if let Some(child) =
                            read_source(&lock(conn), level - 1, 2 * x + dx, 2 * z + dz)
                        {
                            parent.merge_child(&child, dx as usize, dz as usize);
                            any = true;
                        }
                    }
                    any.then(|| parent.compact().encode().ok().map(|row| (x, z, row)))
                        .flatten()
                })
                .collect();
            let mut conn = lock(conn);
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            for (x, z, row) in &rows {
                upsert(&tx, level, *x, *z, row, now).map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            count += rows.len() as u64;
        }
    }
    Ok(count)
}

/// A row as [`Source`], if it is one this module writes: zstd blobs in
/// `DATA_FORMAT.V2_LATEST`.
fn read_source(conn: &Connection, level: u8, x: i32, z: i32) -> Option<Source> {
    let blobs: [Vec<u8>; 7] = conn
        .query_row(
            "SELECT Data, ColumnGenerationStep, Mapping,
                NorthAdjData, SouthAdjData, EastAdjData, WestAdjData
             FROM FullData WHERE DetailLevel = ?1 AND PosX = ?2 AND PosZ = ?3
                AND CompressionMode = ?4 AND DataFormatVersion = ?5",
            params![level, x, z, COMPRESSION_ZSTD, DATA_FORMAT_V2],
            |r| {
                Ok([
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ])
            },
        )
        .ok()?;
    Source::decode(&blobs)
}

/// Deletes a database with its journal files; a missing one is fine.
fn remove_database(path: &Path) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        if let Err(e) = fs::remove_file(&p) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("could not replace {}: {e}", path.display()));
            }
        }
    }
    Ok(())
}

/// Opens the database, laying down DH's schema in a new one.
fn open_database(path: &Path) -> Result<Connection, String> {
    let sql = |e: rusqlite::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let conn = Connection::open(path).map_err(sql)?;
    // Minecraft holding the world open is the only other writer there can be.
    conn.busy_timeout(std::time::Duration::from_secs(60))
        .map_err(sql)?;
    let has_schema: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'Schema'",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if has_schema == 0 {
        // What scripts 0030 and 0031 leave set.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .map_err(sql)?;
        let tx = conn.unchecked_transaction().map_err(sql)?;
        tx.execute_batch(SCHEMA).map_err(sql)?;
        for script in SCRIPTS {
            tx.execute(
                "INSERT INTO Schema (ScriptName) VALUES (?1)",
                [format!("sqlScripts/{script}")],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
    }
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('FullData')")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .map_err(sql)?;
    if let Some(missing) = NEEDED_COLUMNS
        .iter()
        .find(|c| !columns.iter().any(|have| have == *c))
    {
        return Err(format!(
            "{} is from an older Distant Horizons (FullData has no {missing}); open the world once with the current DH first, or delete the file",
            path.display()
        ));
    }
    Ok(conn)
}

/// DH's own upsert (`FullDataSourceV2Repo.createUpsertStatement`) with every
/// flag present. Only block-detail rows ask DH to update their parents.
fn upsert(
    conn: &Connection,
    level: u8,
    sx: i32,
    sz: i32,
    row: &Row,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO FullData (DetailLevel, PosX, PosZ, MinY, DataChecksum,
            Data, ColumnGenerationStep, ColumnWorldCompressionMode, Mapping,
            NorthAdjData, SouthAdjData, EastAdjData, WestAdjData,
            DataFormatVersion, CompressionMode, ApplyToParent, ApplyToChildren, Regenerate,
            LastModifiedUnixDateTime, CreatedUnixDateTime)
         VALUES (?15, ?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?16, 0, 0, ?14, ?14)
         ON CONFLICT(DetailLevel, PosX, PosZ) DO UPDATE SET
            DataChecksum = excluded.DataChecksum, Data = excluded.Data,
            ColumnGenerationStep = excluded.ColumnGenerationStep,
            ColumnWorldCompressionMode = excluded.ColumnWorldCompressionMode,
            Mapping = excluded.Mapping,
            NorthAdjData = excluded.NorthAdjData, SouthAdjData = excluded.SouthAdjData,
            EastAdjData = excluded.EastAdjData, WestAdjData = excluded.WestAdjData,
            DataFormatVersion = excluded.DataFormatVersion,
            CompressionMode = excluded.CompressionMode,
            ApplyToParent = excluded.ApplyToParent, ApplyToChildren = 0, Regenerate = 0,
            LastModifiedUnixDateTime = excluded.LastModifiedUnixDateTime",
        params![
            sx,
            sz,
            row.checksum,
            row.data,
            row.gen_steps,
            row.world_compression,
            row.mapping,
            row.adjacent[0],
            row.adjacent[1],
            row.adjacent[2],
            row.adjacent[3],
            DATA_FORMAT_V2,
            COMPRESSION_ZSTD,
            now,
            level,
            level == 0,
        ],
    )?;
    Ok(())
}

// ------------------------------------------------------------- chunk reading

#[derive(Deserialize)]
struct ChunkNbt {
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "yPos")]
    y_pos: Option<i32>,
    #[serde(default)]
    sections: Vec<SectionNbt>,
}

#[derive(Deserialize)]
struct SectionNbt {
    #[serde(rename = "Y")]
    y: i8,
    block_states: Option<Paletted<BlockNbt>>,
    biomes: Option<Paletted<String>>,
    #[serde(rename = "BlockLight")]
    block_light: Option<ByteArray>,
    #[serde(rename = "SkyLight")]
    sky_light: Option<ByteArray>,
}

#[derive(Deserialize)]
struct Paletted<T> {
    palette: Vec<T>,
    data: Option<LongArray>,
}

#[derive(Deserialize)]
struct BlockNbt {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: HashMap<String, String>,
}

/// One chunk section, unpacked.
struct Section {
    /// Palette index per `y * 256 + z * 16 + x`.
    blocks: Vec<u16>,
    /// DH serial strings of the block palette.
    block_names: Vec<String>,
    /// Palette index per `(y / 4) * 16 + (z / 4) * 4 + x / 4`.
    biomes: Vec<u16>,
    biome_names: Vec<String>,
    block_light: Option<Vec<u8>>,
    sky_light: Option<Vec<u8>>,
}

/// A full chunk, sections bottom to top.
struct Chunk {
    /// The world floor DH measures datapoint bottoms from.
    min_y: i32,
    sections: Vec<(i32, Section)>,
}

impl Chunk {
    /// `None` for anything Minecraft would not show: a proto-chunk or
    /// unreadable data.
    fn parse(data: &[u8]) -> Option<Self> {
        let nbt: ChunkNbt = fastnbt::from_bytes(data).ok()?;
        if !matches!(nbt.status.as_deref(), Some("minecraft:full" | "full")) {
            return None;
        }
        let mut sections: Vec<(i32, Section)> = nbt
            .sections
            .into_iter()
            .map(|s| (i32::from(s.y), Section::from_nbt(s)))
            .collect();
        sections.sort_by_key(|(y, _)| *y);
        let lowest = sections.first()?.0;
        Some(Self {
            min_y: nbt.y_pos.unwrap_or(lowest) * 16,
            sections,
        })
    }
}

impl Section {
    fn from_nbt(s: SectionNbt) -> Self {
        let (block_names, blocks) = match s.block_states {
            Some(p) => {
                let names: Vec<String> = p.palette.iter().map(block_serial).collect();
                let bits = bits_for(names.len()).max(4);
                (names, unpack(p.data.as_deref(), bits, 4096))
            }
            None => (vec![AIR.to_string()], vec![0; 4096]),
        };
        let (biome_names, biomes) = match s.biomes {
            Some(p) if !p.palette.is_empty() => {
                let idx = unpack(p.data.as_deref(), bits_for(p.palette.len()), 64);
                (p.palette, idx)
            }
            _ => (vec![DEFAULT_BIOME.to_string()], vec![0; 64]),
        };
        let nibbles = |a: Option<ByteArray>| {
            a.filter(|a| a.len() == 2048)
                .map(|a| a.iter().map(|&b| b as u8).collect())
        };
        Self {
            blocks,
            block_names,
            biomes,
            biome_names,
            block_light: nibbles(s.block_light),
            sky_light: nibbles(s.sky_light),
        }
    }
}

/// Bits per index for a palette of `len`, as Minecraft packs it.
fn bits_for(len: usize) -> u32 {
    if len <= 1 {
        0
    } else {
        usize::BITS - (len - 1).leading_zeros()
    }
}

/// Minecraft's packed arrays since 1.16: indices never straddle two longs.
fn unpack(data: Option<&[i64]>, bits: u32, count: usize) -> Vec<u16> {
    let (Some(data), true) = (data, bits > 0) else {
        return vec![0; count];
    };
    let per_long = (64 / bits) as usize;
    let mask = (1u64 << bits) - 1;
    (0..count)
        .map(|i| {
            let word = data.get(i / per_long).copied().unwrap_or(0) as u64;
            ((word >> ((i % per_long) as u32 * bits)) & mask) as u16
        })
        .collect()
}

/// `BlockStateWrapper.serialize`: `namespace:path_STATE_{name:value}...` with
/// the properties sorted by name, and `AIR` for every air. DH reads a state
/// whose properties do not match one of the block's exactly as the block's
/// default state.
fn block_serial(block: &BlockNbt) -> String {
    let name = if block.name.contains(':') {
        block.name.clone()
    } else {
        format!("minecraft:{}", block.name)
    };
    if matches!(
        name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    ) {
        return AIR.to_string();
    }
    let mut props: Vec<_> = block.properties.iter().collect();
    props.sort();
    let mut out = name + STATE_SEPARATOR;
    for (k, v) in props {
        out.push_str(&format!("{{{k}:{v}}}"));
    }
    out
}

fn nibble(array: &[u8], i: usize) -> u8 {
    (array[i >> 1] >> ((i & 1) * 4)) & 15
}

// ------------------------------------------------------------ the data source

/// One `FullDataPointUtil` datapoint; `bottom` counts from the world floor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    id: u32,
    height: u32,
    bottom: u32,
    block_light: u8,
    sky_light: u8,
}

/// A `FullDataSourceV2` under construction: 64x64 columns (index `x * 64 + z`),
/// each top-down.
struct Source {
    points: Vec<Vec<Point>>,
    gen_steps: Vec<u8>,
    ids: HashMap<String, u32>,
    mapping: Vec<String>,
    columns: usize,
}

/// A section's encoded blobs, ready for its row.
struct Row {
    checksum: i32,
    data: Vec<u8>,
    gen_steps: Vec<u8>,
    world_compression: Vec<u8>,
    mapping: Vec<u8>,
    /// North, south, east, west.
    adjacent: [Vec<u8>; 4],
}

impl Source {
    fn new() -> Self {
        Self {
            points: vec![Vec::new(); WIDTH * WIDTH],
            gen_steps: vec![GEN_EMPTY; WIDTH * WIDTH],
            ids: HashMap::new(),
            mapping: Vec::new(),
            columns: 0,
        }
    }

    /// `FullDataPointIdMap.addIfNotPresentAndGetId`: ids are per source, in
    /// order of first use.
    fn id(&mut self, biome: &str, block: &str) -> u32 {
        self.key_id(&format!("{biome}{PAIR_SEPARATOR}{block}"))
    }

    fn key_id(&mut self, key: &str) -> u32 {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        let id = self.mapping.len() as u32;
        self.mapping.push(key.to_string());
        self.ids.insert(key.to_string(), id);
        id
    }

    /// A row's blobs (`Data`, `ColumnGenerationStep`, `Mapping`, then the
    /// north, south, east and west strips), as `FullDataSourceV2DTO` reads
    /// them. `None` for anything malformed.
    fn decode(blobs: &[Vec<u8>; 7]) -> Option<Self> {
        let raw = |b: &[u8]| zstd::decode_all(b).ok();
        let w = WIDTH as u32;
        let mut source = Self::new();
        // The middle, then the strips that hold the edge columns.
        let areas = [
            (0, (1, w - 1), (1, w - 1)),
            (3, (0, w), (0, 1)),
            (4, (0, w), (w - 1, w)),
            (5, (w - 1, w), (0, w)),
            (6, (0, 1), (0, w)),
        ];
        for (blob, xs, zs) in areas {
            let cols = decode_points(&raw(&blobs[blob])?, xs, zs)?;
            let positions = (xs.0..xs.1).flat_map(|x| (zs.0..zs.1).map(move |z| (x, z)));
            for ((x, z), col) in positions.zip(cols) {
                source.points[x as usize * WIDTH + z as usize] = col;
            }
        }
        source.gen_steps = raw(&blobs[1])?;
        let mapping = raw(&blobs[2])?;
        let count = i32::from_be_bytes(mapping.get(..4)?.try_into().ok()?);
        let mut at = 4;
        for _ in 0..count {
            let len = u16::from_be_bytes(mapping.get(at..at + 2)?.try_into().ok()?) as usize;
            let entry = String::from_utf8_lossy(mapping.get(at + 2..at + 2 + len)?);
            source.key_id(&entry);
            at += 2 + len;
        }
        let max_id = source.points.iter().flatten().map(|p| p.id).max();
        (source.gen_steps.len() == WIDTH * WIDTH && max_id.is_none_or(|id| id < count as u32))
            .then_some(source)
    }

    /// `updateFromOneBelowDetailLevel`: `child`, the one `dx`, `dz` (0 or 1)
    /// along, into its quarter of this source, each 2x2 of its columns
    /// merged into one.
    fn merge_child(&mut self, child: &Source, dx: usize, dz: usize) {
        let (off_x, off_z) = (dx * WIDTH / 2, dz * WIDTH / 2);
        for x in (0..WIDTH).step_by(2) {
            for z in (0..WIDTH).step_by(2) {
                let quad =
                    [(x, z), (x, z + 1), (x + 1, z), (x + 1, z + 1)].map(|(x, z)| x * WIDTH + z);
                let target = (off_x + x / 2) * WIDTH + off_z + z / 2;
                self.gen_steps[target] = quad
                    .iter()
                    .map(|&i| child.gen_steps[i])
                    .min()
                    .unwrap_or(GEN_EMPTY);
                let column = merge_columns(quad.map(|i| child.points[i].as_slice()), |id| {
                    child.mapping.get(id as usize).is_some_and(|k| {
                        k.strip_suffix(AIR)
                            .is_some_and(|k| k.ends_with(PAIR_SEPARATOR))
                    })
                });
                self.points[target] = column
                    .into_iter()
                    .map(|p| Point {
                        id: self.key_id(&child.mapping[p.id as usize]),
                        ..p
                    })
                    .collect();
            }
        }
    }

    /// `removeUnusedIdsAndRemap`: only the ids in use, numbered in the order
    /// the columns use them.
    fn compact(mut self) -> Self {
        let old = std::mem::take(&mut self.mapping);
        self.ids.clear();
        let mut points = std::mem::take(&mut self.points);
        for p in points.iter_mut().flatten() {
            p.id = self.key_id(&old[p.id as usize]);
        }
        self.points = points;
        self
    }

    /// `LodDataBuilder.createFromChunk` for the chunk at column offset
    /// `(off_x, off_z)`: each column walked down from the top of the chunk, one
    /// datapoint per run of the same block and biome, lit by the light just
    /// above the run's top block.
    fn add_chunk(&mut self, chunk: &Chunk, off_x: usize, off_z: usize) {
        let top = chunk.sections.last().map_or(0, |(y, _)| y + 1) * 16;
        let bottom = chunk.sections.first().map_or(0, |(y, _)| *y) * 16;
        let by_y: HashMap<i32, &Section> = chunk.sections.iter().map(|(y, s)| (*y, s)).collect();
        let block_at = |x: usize, y: i32, z: usize| -> (&str, &str) {
            let Some(s) = by_y.get(&(y >> 4)) else {
                return (AIR, DEFAULT_BIOME);
            };
            let ly = (y & 15) as usize;
            let b = s.blocks[ly * 256 + z * 16 + x] as usize;
            let bi = s.biomes[(ly >> 2) * 16 + (z >> 2) * 4 + (x >> 2)] as usize;
            (
                s.block_names.get(b).map_or(AIR, String::as_str),
                s.biome_names.get(bi).map_or(DEFAULT_BIOME, String::as_str),
            )
        };
        // Stored light where the chunk has it. Without it: full sky down to
        // the first block and dark below, which is what the top faces DH
        // draws from this value would show.
        let light_at = |x: usize, y: i32, z: usize, open: bool| -> (u8, u8) {
            if y >= top {
                return (0, 15);
            }
            let i = ((y & 15) as usize) * 256 + z * 16 + x;
            match by_y.get(&(y >> 4)).map(|s| (&s.block_light, &s.sky_light)) {
                Some((Some(b), Some(sky))) => (nibble(b, i), nibble(sky, i)),
                Some((None, Some(sky))) => (0, nibble(sky, i)),
                _ => (0, if open { 15 } else { 0 }),
            }
        };

        for x in 0..16 {
            for z in 0..16 {
                let mut column = Vec::new();
                let (_, top_biome) = block_at(x, top - 1, z);
                let mut current = (AIR, top_biome);
                let mut id = self.id(top_biome, AIR);
                let mut light = (0u8, 15u8);
                let mut last_y = top;
                // No block above the current `y` yet.
                let mut open = true;
                let mut y = top - 1;
                while y >= bottom {
                    let (block, biome) = block_at(x, y, z);
                    // Air above the ground is one run whatever its biome, as
                    // DH skips straight down to the heightmap.
                    if block != current.0 || (biome != current.1 && !open) {
                        column.push(Point {
                            id,
                            height: (last_y - y) as u32,
                            bottom: (y + 1 - chunk.min_y) as u32,
                            block_light: light.0,
                            sky_light: light.1,
                        });
                        current = (block, biome);
                        id = self.id(biome, block);
                        light = light_at(x, y + 1, z, open);
                        last_y = y;
                    }
                    open &= block == AIR;
                    y -= 1;
                }
                column.push(Point {
                    id,
                    height: (last_y - y) as u32,
                    bottom: (y + 1 - chunk.min_y) as u32,
                    block_light: light.0,
                    sky_light: light.1,
                });
                let index = (off_x + x) * WIDTH + off_z + z;
                self.points[index] = column;
                self.gen_steps[index] = GEN_LIGHT;
                self.columns += 1;
            }
        }
    }

    fn encode(&self) -> Result<Row, String> {
        let z =
            |raw: &[u8]| zstd::bulk::compress(raw, ZSTD_LEVEL).map_err(|e| format!("zstd: {e}"));
        let w = WIDTH as u32;
        let data = encode_points(&self.points, (1, w - 1), (1, w - 1));
        let mut mapping = (self.mapping.len() as i32).to_be_bytes().to_vec();
        for entry in &self.mapping {
            // `DataOutputStream.writeUTF`; every serial string is ASCII.
            mapping.extend_from_slice(&(entry.len() as u16).to_be_bytes());
            mapping.extend_from_slice(entry.as_bytes());
        }
        Ok(Row {
            checksum: fnv32(&data),
            data: z(&data)?,
            gen_steps: z(&self.gen_steps)?,
            world_compression: z(&[0u8; WIDTH * WIDTH])?,
            mapping: z(&mapping)?,
            // `FullDataMinMaxPosUtil.getEncodedMinMaxPos`, x then z ranges.
            adjacent: [
                z(&encode_points(&self.points, (0, w), (0, 1)))?,
                z(&encode_points(&self.points, (0, w), (w - 1, w)))?,
                z(&encode_points(&self.points, (w - 1, w), (0, w)))?,
                z(&encode_points(&self.points, (0, 1), (0, w)))?,
            ],
        })
    }
}

/// `FullDataSourceV2DTO.writeDataSourceDataArrayToBlobV2` over the columns in
/// the half-open ranges `xs` by `zs`, x outer: counts, then ids with their lit
/// and discontinuity flags, heights, mispredicted bottoms, and packed light.
fn encode_points(points: &[Vec<Point>], xs: (u32, u32), zs: (u32, u32)) -> Vec<u8> {
    let columns = || {
        (xs.0..xs.1)
            .flat_map(move |x| (zs.0..zs.1).map(move |z| &points[x as usize * WIDTH + z as usize]))
    };
    let mut out = Vec::new();
    for col in columns() {
        varint(&mut out, col.len() as u32);
    }
    let mut previous = 0i32;
    for p in columns().flatten() {
        let lit = (p.block_light | p.sky_light) != 0;
        let gap = p.bottom as i32 != previous - p.height as i32;
        previous = p.bottom as i32;
        varint(
            &mut out,
            (p.id << 2) | (u32::from(lit) << 1) | u32::from(gap),
        );
    }
    for p in columns().flatten() {
        varint(&mut out, p.height);
    }
    previous = 0;
    for p in columns().flatten() {
        let error = p.bottom as i32 - (previous - p.height as i32);
        if error != 0 {
            varint(&mut out, ((error << 1) ^ (error >> 31)) as u32);
        }
        previous = p.bottom as i32;
    }
    for p in columns().flatten() {
        let packed = (p.block_light << 4) | p.sky_light;
        if packed != 0 {
            out.push(packed);
        }
    }
    out
}

/// `readBlobToDataSourceDataArrayV2`, the reverse of [`encode_points`].
fn decode_points(blob: &[u8], xs: (u32, u32), zs: (u32, u32)) -> Option<Vec<Vec<Point>>> {
    let mut at = 0;
    let mut byte = || {
        at += 1;
        blob.get(at - 1).copied()
    };
    let mut varint = || {
        let (mut v, mut shift) = (0u32, 0);
        loop {
            let b = byte()?;
            v |= u32::from(b & 127).checked_shl(shift)?;
            shift += 7;
            if b & 128 == 0 {
                return Some(v);
            }
        }
    };
    let n = ((xs.1 - xs.0) * (zs.1 - zs.0)) as usize;
    let counts: Vec<usize> = (0..n)
        .map(|_| varint().map(|c| c as usize))
        .collect::<Option<_>>()?;
    let mut flags = Vec::new();
    let mut cols = Vec::with_capacity(n);
    for &c in &counts {
        let mut col = Vec::with_capacity(c.min(4096));
        for _ in 0..c {
            let e = varint()?;
            flags.push(e & 3);
            col.push(Point {
                id: e >> 2,
                height: 0,
                bottom: 0,
                block_light: 0,
                sky_light: 0,
            });
        }
        cols.push(col);
    }
    for p in cols.iter_mut().flatten() {
        p.height = varint()?;
    }
    let mut previous = 0i32;
    for (p, f) in cols.iter_mut().flatten().zip(&flags) {
        let error = if f & 1 != 0 {
            let v = varint()?;
            ((v >> 1) as i32) ^ -((v & 1) as i32)
        } else {
            0
        };
        previous = previous - p.height as i32 + error;
        p.bottom = previous as u32;
    }
    for (p, f) in cols.iter_mut().flatten().zip(&flags) {
        if f & 2 != 0 {
            let packed = byte()?;
            p.sky_light = packed & 15;
            p.block_light = packed >> 4;
        }
    }
    (at == blob.len()).then_some(cols)
}

/// `mergeInputTwoByTwoDataColumn`: four top-down columns into one, sliced at
/// every datapoint edge and sampled mid-slice. A slice takes the commonest
/// non-air id (the first column's on a tie; id 0 where none has data) and
/// the light averaged over the columns showing that id.
fn merge_columns(columns: [&[Point]; 4], is_air: impl Fn(u32) -> bool) -> Vec<Point> {
    let mut edges: Vec<u32> = columns
        .iter()
        .flat_map(|c| c.iter().flat_map(|p| [p.bottom, p.bottom + p.height]))
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let mut out: Vec<Point> = Vec::new();
    for slice in edges.windows(2) {
        let (bottom, height) = (slice[0], slice[1] - slice[0]);
        let y = bottom + height / 2;
        let hits = columns.map(|c| {
            c.iter()
                .find(|p| p.bottom <= y && y < p.bottom + p.height)
                .map_or((0, 0, 0), |p| (p.id, p.block_light, p.sky_light))
        });
        let ids = hits.map(|h| h.0);
        let mut counts = [0; 4];
        for &id in ids.iter().filter(|&&id| !is_air(id)) {
            // The first equal entry takes the count, as DH's if-chain does.
            counts[ids.iter().position(|&v| v == id).unwrap_or(3)] += 1;
        }
        let best = *counts.iter().max().unwrap_or(&0);
        let id = ids[counts.iter().position(|&c| c == best).unwrap_or(0)];
        let average = |light: fn(&(u32, u8, u8)) -> u8| {
            let matching: Vec<u32> = hits
                .iter()
                .filter(|h| h.0 == id)
                .map(|h| u32::from(light(h)))
                .collect();
            if matching.is_empty() {
                (hits.iter().map(|h| u32::from(light(h))).sum::<u32>() / 4) as u8
            } else {
                (matching.iter().sum::<u32>() / matching.len() as u32) as u8
            }
        };
        let (block_light, sky_light) = (average(|h| h.1), average(|h| h.2));
        match out.last_mut() {
            Some(last)
                if (last.id, last.block_light, last.sky_light) == (id, block_light, sky_light) =>
            {
                last.height += height;
            }
            _ => out.push(Point {
                id,
                height,
                bottom,
                block_light,
                sky_light,
            }),
        }
    }
    // Built bottom-up; DH keeps columns top-down.
    out.reverse();
    out
}

/// `VarintUtil.writeVarint`.
fn varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 128 {
        out.push((v as u8) | 128);
        v >>= 7;
    }
    out.push(v as u8);
}

/// DH does not read `DataChecksum` back; any stable value will do.
fn fnv32(bytes: &[u8]) -> i32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    }) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: u32, height: u32, bottom: u32, block_light: u8, sky_light: u8) -> Point {
        Point {
            id,
            height,
            bottom,
            block_light,
            sky_light,
        }
    }

    #[test]
    fn points_round_trip_through_dhs_reader() {
        let mut points = vec![Vec::new(); WIDTH * WIDTH];
        // Contiguous, then a gap, unlit, block-lit, and a multi-byte id.
        points[65] = vec![
            point(0, 300, 84, 0, 15),
            point(1, 20, 64, 0, 15),
            point(2, 64, 0, 0, 0),
        ];
        points[66] = vec![point(3, 10, 200, 14, 3), point(400, 5, 7, 0, 0)];
        points[WIDTH * 62 + 62] = vec![point(1, 384, 0, 0, 0)];
        let back =
            decode_points(&encode_points(&points, (1, 63), (1, 63)), (1, 63), (1, 63)).unwrap();
        let mut i = 0;
        for x in 1..63 {
            for z in 1..63 {
                assert_eq!(back[i], points[x * WIDTH + z], "column {x},{z}");
                i += 1;
            }
        }
    }

    #[test]
    fn edge_blobs_cover_dhs_adjacent_strips() {
        let mut points = vec![Vec::new(); WIDTH * WIDTH];
        points[5] = vec![point(1, 2, 3, 0, 15)]; // x 0, z 5: west
        points[63 * WIDTH + 63] = vec![point(2, 2, 3, 0, 15)]; // south-east corner
        let strip = |xs, zs| decode_points(&encode_points(&points, xs, zs), xs, zs).unwrap();
        assert_eq!(strip((0, 1), (0, 64))[5], points[5]);
        assert_eq!(strip((0, 64), (63, 64))[63], points[63 * WIDTH + 63]);
        assert_eq!(strip((63, 64), (0, 64))[63], points[63 * WIDTH + 63]);
    }

    #[test]
    fn block_serials_match_dhs_wrapper() {
        let block = |name: &str, props: &[(&str, &str)]| BlockNbt {
            name: name.into(),
            properties: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        assert_eq!(
            block_serial(&block(
                "minecraft:oak_stairs",
                &[("half", "bottom"), ("facing", "north")]
            )),
            "minecraft:oak_stairs_STATE_{facing:north}{half:bottom}"
        );
        assert_eq!(block_serial(&block("stone", &[])), "minecraft:stone_STATE_");
        assert_eq!(block_serial(&block("minecraft:cave_air", &[])), "AIR");
    }

    #[test]
    fn unpack_follows_minecrafts_padding() {
        // 5 bits: 12 per long, the top 4 bits of each long unused.
        let word0 = (0..12u64).fold(0u64, |w, i| w | (i << (i * 5)));
        let got = unpack(Some(&[word0 as i64, 31]), 5, 13);
        assert_eq!(got[11], 11);
        assert_eq!(got[12], 31);
        assert_eq!((bits_for(1), bits_for(2), bits_for(17)), (0, 1, 5));
    }

    /// Sections -4..=0: air at y -64, stone up to -1, grass at 0, open above.
    fn test_chunk(lit: bool) -> Chunk {
        let sections = (-4..=0)
            .map(|sy| {
                let mut blocks = vec![1u16; 4096];
                let mut sky = vec![0u8; 2048];
                if sy == -4 {
                    blocks[..256].fill(0);
                }
                if sy == 0 {
                    blocks.fill(0);
                    blocks[..256].fill(2);
                    sky[128..].fill(0xFF);
                }
                let section = Section {
                    blocks,
                    block_names: vec![
                        AIR.into(),
                        "minecraft:stone_STATE_".into(),
                        "minecraft:grass_block_STATE_{snowy:false}".into(),
                    ],
                    biomes: vec![0; 64],
                    biome_names: vec![DEFAULT_BIOME.into()],
                    block_light: lit.then(|| vec![0; 2048]),
                    sky_light: lit.then_some(sky),
                };
                (sy, section)
            })
            .collect();
        Chunk {
            min_y: -64,
            sections,
        }
    }

    /// The columns DH would build from a chunk, top-down to the world floor.
    #[test]
    fn a_chunk_becomes_dhs_columns() {
        let mut source = Source::new();
        source.add_chunk(&test_chunk(true), 16, 0);
        assert_eq!(source.columns, 256);
        assert_eq!(source.gen_steps[16 * WIDTH], GEN_LIGHT);
        assert_eq!(source.gen_steps[0], GEN_EMPTY);
        let col = &source.points[16 * WIDTH];
        let names: Vec<&str> = col
            .iter()
            .map(|p| source.mapping[p.id as usize].as_str())
            .collect();
        assert_eq!(
            names,
            [
                "minecraft:plains_DH-BSW_AIR",
                "minecraft:plains_DH-BSW_minecraft:grass_block_STATE_{snowy:false}",
                "minecraft:plains_DH-BSW_minecraft:stone_STATE_",
                "minecraft:plains_DH-BSW_AIR",
            ]
        );
        // DH's first air run reaches one past the top, as `createFromChunk`'s does.
        assert_eq!(col[0], point(0, 16, 65, 0, 15));
        assert_eq!(col[1], point(1, 1, 64, 0, 15));
        assert_eq!(col[2], point(2, 63, 1, 0, 0));
        assert_eq!(col[3], point(0, 1, 0, 0, 0));
        for pair in col.windows(2) {
            assert_eq!(pair[0].bottom, pair[1].bottom + pair[1].height);
        }
    }

    /// Traced by hand through `mergeInputTwoByTwoDataColumn`: id 0 is air, a
    /// missing column reads as id 0 with no light, and air never outvotes.
    #[test]
    fn four_columns_merge_as_dh_merges_them() {
        let c0 = [point(0, 10, 20, 0, 15), point(1, 20, 0, 0, 0)];
        let c1 = [point(0, 15, 15, 0, 15), point(2, 15, 0, 0, 0)];
        let merged = merge_columns([&c0, &c1, &[], &c0], |id| id == 0);
        // Slices 0-15 and 15-20 go to stone (1), 20-30 to air with the sky
        // light of three lit columns and one empty one: 45 / 4.
        assert_eq!(merged, [point(0, 10, 20, 0, 11), point(1, 20, 0, 0, 0)]);
        // A tie goes to the first column's id, lit by the columns showing it.
        let (a, b) = ([point(2, 5, 0, 3, 0)], [point(1, 5, 0, 9, 0)]);
        let tie = merge_columns([&a, &b, &b, &[point(2, 5, 0, 6, 0)]], |_| false);
        assert_eq!(tie, [point(2, 5, 0, 4, 0)]);
        // Missing columns vote too, as id 0.
        assert_eq!(merge_columns([&a, &b, &[], &[]], |_| false)[0].id, 0);
    }

    #[test]
    fn a_row_reads_back_as_written() {
        let mut source = Source::new();
        source.add_chunk(&test_chunk(true), 48, 0);
        source.add_chunk(&test_chunk(true), 0, 48);
        let row = source.encode().unwrap();
        let [n, s, e, w] = row.adjacent.clone();
        let back = Source::decode(&[row.data, row.gen_steps, row.mapping, n, s, e, w]).unwrap();
        assert_eq!(back.points, source.points);
        assert_eq!(back.gen_steps, source.gen_steps);
        assert_eq!(back.mapping, source.mapping);
    }

    #[test]
    fn every_coarser_level_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_database(&dir.path().join("dh.sqlite")).unwrap();
        let mut source = Source::new();
        source.add_chunk(&test_chunk(true), 0, 0);
        upsert(&conn, 0, -1, 0, &source.encode().unwrap(), 1).unwrap();
        let conn = Mutex::new(conn);
        assert_eq!(write_coarser_levels(&conn, vec![(-1, 0)], 1).unwrap(), 8);
        let conn = conn.into_inner().unwrap();
        let keys: Vec<(u8, i32, i32)> = conn
            .prepare(
                "SELECT DetailLevel, PosX, PosZ FROM FullData WHERE DetailLevel > 0 ORDER BY 1",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(keys, (1..=8).map(|l| (l, -1, 0)).collect::<Vec<_>>());
        // Section -1 is the east child of -1 at level 1: the chunk's 16x16
        // columns land as 8x8 in that quarter.
        let parent = read_source(&conn, 1, -1, 0).unwrap();
        assert_eq!(parent.gen_steps[32 * WIDTH], GEN_LIGHT);
        assert_eq!(parent.gen_steps[40 * WIDTH], GEN_EMPTY);
        assert_eq!(parent.gen_steps[0], GEN_EMPTY);
        let names: Vec<&str> = parent.points[32 * WIDTH]
            .iter()
            .map(|p| parent.mapping[p.id as usize].as_str())
            .collect();
        assert_eq!(
            names[1],
            "minecraft:plains_DH-BSW_minecraft:grass_block_STATE_{snowy:false}"
        );
        assert_eq!(parent.points[32 * WIDTH], source.points[0]);
        // The top level holds the chunk too.
        assert!(read_source(&conn, 8, -1, 0)
            .unwrap()
            .points
            .iter()
            .any(|c| !c.is_empty()));

        // Its sibling written later, with the first child no longer readable
        // here: the parent keeps that child's quarter.
        conn.execute(
            "UPDATE FullData SET CompressionMode = 3 WHERE DetailLevel = 0",
            [],
        )
        .unwrap();
        upsert(&conn, 0, -2, 0, &source.encode().unwrap(), 2).unwrap();
        let conn = Mutex::new(conn);
        write_coarser_levels(&conn, vec![(-2, 0)], 2).unwrap();
        let parent = read_source(&lock(&conn), 1, -1, 0).unwrap();
        assert_eq!(parent.points[32 * WIDTH], source.points[0]);
        assert_eq!(parent.points[0], source.points[0]);
    }

    #[test]
    fn unlit_chunks_take_sky_from_above() {
        let mut source = Source::new();
        source.add_chunk(&test_chunk(false), 0, 0);
        let col = &source.points[0];
        assert_eq!((col[1].block_light, col[1].sky_light), (0, 15));
        assert_eq!((col[2].block_light, col[2].sky_light), (0, 0));
    }

    #[test]
    fn a_new_database_has_dhs_schema_and_takes_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = database_path(dir.path());
        let conn = open_database(&path).unwrap();
        let count = |conn: &Connection, sql: &str| -> i64 {
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        };
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM Schema"), 12);
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");

        let mut source = Source::new();
        source.add_chunk(&test_chunk(true), 0, 0);
        let row = source.encode().unwrap();
        upsert(&conn, 0, -3, 7, &row, 1).unwrap();
        upsert(&conn, 0, -3, 7, &row, 2).unwrap();
        let (n, parent, created, modified): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(ApplyToParent), MAX(CreatedUnixDateTime),
                    MAX(LastModifiedUnixDateTime)
                 FROM FullData WHERE DetailLevel = 0 AND PosX = -3 AND PosZ = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((n, parent, created, modified), (1, 1, 1, 2));

        let blob: Vec<u8> = conn
            .query_row("SELECT Data FROM FullData", [], |r| r.get(0))
            .unwrap();
        let cols = decode_points(&zstd::decode_all(&blob[..]).unwrap(), (1, 63), (1, 63)).unwrap();
        assert_eq!(cols[0], source.points[WIDTH + 1]);
        drop(conn);

        // Reopening keeps the schema and the row.
        let conn = open_database(&path).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM Schema"), 12);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM FullData"), 1);
    }

    /// A world whose `level.dat` declares `data_version`.
    fn world_at(data_version: i32) -> tempfile::TempDir {
        use fastnbt::Value;
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let data = HashMap::from([("DataVersion".to_string(), Value::Int(data_version))]);
        let root = Value::Compound(HashMap::from([("Data".to_string(), Value::Compound(data))]));
        let mut gz = flate2::write::GzEncoder::new(
            File::create(dir.path().join("level.dat")).unwrap(),
            flate2::Compression::default(),
        );
        gz.write_all(&fastnbt::to_bytes(&root).unwrap()).unwrap();
        gz.finish().unwrap();
        dir
    }

    #[test]
    fn the_database_goes_where_each_minecraft_looks() {
        let in_26 = "dimensions/minecraft/overworld/data/DistantHorizons.sqlite";
        // Arnis's own (1.21) layout: `data/` for DH on 1.21, and the same file
        // where DH on 26.1+ looks once Minecraft has upgraded the world.
        let legacy = world_at(4189);
        fs::create_dir_all(legacy.path().join("region")).unwrap();
        fs::write(legacy.path().join("region/r.-1.2.mca"), b"").unwrap();
        fs::write(legacy.path().join("region/r.3.0.mca"), b"").unwrap();
        assert_eq!(region_extent(legacy.path()), Some([-512, 0, 2047, 1535]));
        let old = legacy.path().join("data/DistantHorizons.sqlite");
        assert_eq!(database_path(legacy.path()), old);
        write_lods(legacy.path(), [0, 0, 511, 511], true).unwrap();
        let new = legacy.path().join(in_26);
        assert_eq!(fs::read(&old).unwrap(), fs::read(&new).unwrap());
        // Again, over the link it left.
        write_lods(legacy.path(), [0, 0, 511, 511], false).unwrap();
        let conn = Connection::open(&new).unwrap();
        let scripts: i64 = conn
            .query_row("SELECT COUNT(*) FROM Schema", [], |r| r.get(0))
            .unwrap();
        assert_eq!(scripts, 12);

        // A world Minecraft 26.1+ has already upgraded: only its own folder.
        let upgraded = world_at(4903);
        fs::create_dir_all(
            upgraded
                .path()
                .join("dimensions/minecraft/overworld/region"),
        )
        .unwrap();
        assert_eq!(database_path(upgraded.path()), upgraded.path().join(in_26));
        write_lods(upgraded.path(), [0, 0, 511, 511], true).unwrap();
        assert!(upgraded.path().join(in_26).exists());
        assert!(!upgraded.path().join("data/DistantHorizons.sqlite").exists());
        assert_eq!(region_extent(upgraded.path()), None);
    }

    #[test]
    fn an_old_dh_database_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = database_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE Schema (ScriptName TEXT); CREATE TABLE FullData (DetailLevel TINYINT);",
            )
            .unwrap();
        let err = open_database(&path).unwrap_err();
        assert!(err.contains("older Distant Horizons"), "{err}");
    }
}
