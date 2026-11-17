//! 1.8's chunks, block changes and chunk unloads as 1.20.2 lays them out.
//!
//! A 1.8 block is a number, `id << 4 | meta`. Which 1.20.2 block state each one is comes from vanilla's own
//! data fixers ([`horizon_world::datafix`], the tables of the player's prepared resources): a made-up 1.8
//! chunk section holding all 4096 numbers, one per position, is upgraded once as a world from 1.8 would be,
//! and each position's state is read back. Nothing of Mojang's is written here.
//!
//! Not done yet: what 1.8 worked out from a block's neighbours rather than storing (fence, pane and wall
//! connections, stair shapes, the two halves of doors and tall plants, snowy grass), block entities (signs,
//! chests, skulls, banners), and chunks sent in part (they come as whole-section updates).
use std::{collections::BTreeMap, sync::OnceLock};

use super::{down, id::clientbound as cb1_8};
use crate::{
    Error, Result, State,
    adapter::ServerVersion,
    buf::{ChunkPos, Codec, Reader, SectionPos, Writer},
    chunk::{Paletted, Strategy, storage_get},
    ids::play::clientbound as play,
    registry::ceil_log2,
};
use horizon_core::nbt::Tag;

/// 1.20.2's block state for each 1.8 block number (`id << 4 | meta`).
pub(super) type Table = [i32; 4096];

const AIR: i32 = 0;
/// 1.20.2's `minecraft:stone`: what every non-air block becomes when there is no table to read.
const STONE: i32 = 1;

/// The table, built on first use. Without the prepared data fixers' tables, or if upgrading the made-up
/// section fails, the world still has its shape: every block is stone.
pub(super) fn table() -> &'static Table {
    static TABLE: OnceLock<Box<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        match std::panic::catch_unwind(build)
            .unwrap_or_else(|_| Err("the data fixers panicked".into()))
        {
            Ok(table) => table,
            Err(e) => {
                log::warn!("Horizon: 1.8 blocks show as stone: {e}");
                let mut table = Box::new([STONE; 4096]);
                table[..16].fill(AIR);
                table
            }
        }
    })
}

fn build() -> std::result::Result<Box<Table>, String> {
    use horizon_world::datafix::{self, dynamic};
    if !datafix::tables_available() {
        return Err("the prepared resources have no data fixer tables".into());
    }
    let registry = horizon_world::level::data::load_registry(&datafix::tables_resources())
        .map_err(|e| format!("no block states: {e}"))?;
    let ids = ServerVersion::V1_20_2.ids().ok_or("no 1.20.2 ids")?;

    // Position i holds block number i: the id in `Blocks`, the meta in the nibble array `Data`.
    let blocks: Vec<u8> = (0..4096usize).map(|i| (i >> 4) as u8).collect();
    let mut data = vec![0u8; 2048];
    for i in 0..4096usize {
        data[i / 2] |= ((i & 15) as u8) << (4 * (i % 2));
    }
    let section = Tag::compound([
        ("Y", Tag::Byte(0)),
        ("Blocks", Tag::ByteArray(blocks)),
        ("Data", Tag::ByteArray(data)),
        ("BlockLight", Tag::ByteArray(vec![0; 2048])),
        ("SkyLight", Tag::ByteArray(vec![0xFF; 2048])),
    ]);
    let level = Tag::compound([
        ("xPos", Tag::Int(0)),
        ("zPos", Tag::Int(0)),
        ("LastUpdate", Tag::Long(0)),
        ("InhabitedTime", Tag::Long(0)),
        ("TerrainPopulated", Tag::Byte(1)),
        ("LightPopulated", Tag::Byte(1)),
        ("Sections", Tag::List(vec![section])),
        ("Entities", Tag::List(Vec::new())),
        ("TileEntities", Tag::List(Vec::new())),
        ("Biomes", Tag::ByteArray(vec![1; 256])),
        ("HeightMap", Tag::IntArray(vec![0; 256])),
    ]);
    let mut chunk = Tag::compound([("Level", level)]);
    datafix::update_chunk(&mut chunk, "minecraft:overworld", None);

    let Some(Tag::List(sections)) = chunk.get("sections") else {
        return Err("the upgraded chunk has no sections".into());
    };
    let section = sections
        .iter()
        .find(|s| dynamic::as_int(s.get("Y"), -1000) == 0)
        .ok_or("the upgraded chunk lost its section")?;
    let states = section.get("block_states").ok_or("no block states")?;
    let Some(Tag::List(palette)) = states.get("palette") else {
        return Err("no palette".into());
    };
    let palette: Vec<i32> = palette
        .iter()
        .map(|entry| {
            registry
                .saved_state_id(entry)
                .and_then(|state| ids.block_state.down(state.0 as i32))
                .unwrap_or(STONE)
        })
        .collect();
    let bits = ceil_log2(palette.len()).max(4);
    let longs = match states.get("data") {
        Some(Tag::LongArray(longs)) => longs.clone(),
        _ => Vec::new(),
    };
    let mut table = Box::new([AIR; 4096]);
    for (i, slot) in table.iter_mut().enumerate() {
        let index = if palette.len() == 1 {
            0
        } else {
            storage_get(&longs, bits, i) as usize
        };
        *slot = *palette.get(index).ok_or("palette index out of range")?;
    }
    Ok(table)
}

/// The vertical layout of a dimension's chunks in 1.20.2, by 1.8's dimension number: how many sections,
/// and how many of them lie below y = 0 (the vanilla overworld goes down to -64).
fn layout(dimension: i8) -> (usize, usize) {
    if dimension == 0 { (24, 4) } else { (16, 0) }
}

/// Only 1.8's overworld sends sky light.
pub(super) fn has_sky(dimension: i8) -> bool {
    dimension == 0
}

/// One 1.8 column: for each section sent (bottom up), its y, blocks (4096 little-endian shorts), block
/// light and sky light; then its biomes when the column is whole.
struct Column<'a> {
    sections: Vec<(usize, &'a [u8], &'a [u8], Option<&'a [u8]>)>,
    biomes: Option<&'a [u8]>,
}

impl<'a> Column<'a> {
    /// 1.8 sends every section's blocks, then every section's block light, then sky light, then biomes.
    fn read(r: &mut Reader<'a>, mask: u16, sky: bool, whole: bool) -> Result<Self> {
        let ys: Vec<usize> = (0..16).filter(|y| mask & (1 << y) != 0).collect();
        let n = ys.len();
        let blocks = r.bytes(n * 8192)?;
        let block_light = r.bytes(n * 2048)?;
        let sky_light = if sky { Some(r.bytes(n * 2048)?) } else { None };
        let biomes = if whole { Some(r.bytes(256)?) } else { None };
        let sections = ys
            .into_iter()
            .enumerate()
            .map(|(k, y)| {
                (
                    y,
                    &blocks[k * 8192..(k + 1) * 8192],
                    &block_light[k * 2048..(k + 1) * 2048],
                    sky_light.map(|s| &s[k * 2048..(k + 1) * 2048]),
                )
            })
            .collect();
        Ok(Self { sections, biomes })
    }
}

fn states(table: &Table, blocks: &[u8]) -> Vec<i32> {
    blocks
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| table[usize::from(u16::from_le_bytes(b) & 0xFFF)])
        .collect()
}

/// `PalettedContainer.Data.write` before 1.21.5: a VarInt long count before the storage.
fn write_paletted(p: &Paletted, w: &mut Writer) {
    w.u8(p.bits);
    if !p.palette.is_empty() {
        if p.bits != 0 {
            w.varint(p.palette.len() as i32);
        }
        for &entry in &p.palette {
            w.varint(entry);
        }
    }
    w.varint(p.data.len() as i32);
    for &l in &p.data {
        w.i64(l);
    }
}

/// 1.20.2's biome registry index of each 1.8 biome id (the order of [`super::biomes`]).
fn biome_index() -> &'static [i32; 256] {
    static INDEX: OnceLock<[i32; 256]> = OnceLock::new();
    INDEX.get_or_init(|| {
        let biomes = super::biomes();
        std::array::from_fn(|id| {
            let name = super::biome(id as u8);
            biomes.iter().position(|b| *b == name).unwrap_or(0) as i32
        })
    })
}

/// A whole 1.8 column as 1.20.2's `level_chunk_with_light`.
fn level_chunk(table: &Table, dimension: i8, x: i32, z: i32, column: &Column) -> Result<Vec<u8>> {
    let (count, below) = layout(dimension);
    let state_bits = ServerVersion::V1_20_2
        .ids()
        .ok_or_else(|| Error::new("no 1.20.2 ids"))?
        .state_bits()
        .0;
    let biome_bits = ceil_log2(super::biomes().len());
    let biomes: Vec<i32> = (0..64)
        .map(|i| {
            let (bx, bz) = (i & 3, (i >> 2) & 3);
            let id = column.biomes.map_or(1, |b| b[(bz * 4) * 16 + bx * 4]);
            biome_index()[usize::from(id)]
        })
        .collect();
    let biomes = Paletted::from_values(&biomes, Strategy::Biomes, biome_bits);
    let air = Paletted::from_values(&[AIR], Strategy::Blocks, state_bits);

    let mut buffer = Writer::new();
    for i in 0..count {
        let sent = i
            .checked_sub(below)
            .and_then(|y| column.sections.iter().find(|s| s.0 == y));
        match sent {
            Some((_, blocks, _, _)) => {
                let values = states(table, blocks);
                buffer.i16(values.iter().filter(|&&s| s != AIR).count() as i16);
                write_paletted(
                    &Paletted::from_values(&values, Strategy::Blocks, state_bits),
                    &mut buffer,
                );
            }
            None => {
                buffer.i16(0);
                write_paletted(&air, &mut buffer);
            }
        }
        write_paletted(&biomes, &mut buffer);
    }

    // Light sections run from one below the bottom section to one above the top.
    let top_sent = column.sections.iter().map(|s| s.0).max();
    let full_sky = [0xFFu8; 2048];
    let (mut sky_mask, mut block_mask) = (0u64, 0u64);
    let (mut sky, mut block): (Vec<&[u8]>, Vec<&[u8]>) = (Vec::new(), Vec::new());
    for light in 0..count + 2 {
        let y = (light as isize) - 1 - below as isize;
        let sent = usize::try_from(y)
            .ok()
            .and_then(|y| column.sections.iter().find(|s| s.0 == y));
        if let Some((_, _, block_light, sky_light)) = sent {
            block_mask |= 1 << light;
            block.push(block_light);
            if let Some(sky_light) = sky_light {
                sky_mask |= 1 << light;
                sky.push(sky_light);
            }
        } else if has_sky(dimension) && top_sent.is_none_or(|top| y > top as isize) {
            // Open sky above the highest section 1.8 sent.
            sky_mask |= 1 << light;
            sky.push(&full_sky);
        }
    }

    let mut w = Writer::new();
    w.i32(x);
    w.i32(z);
    w.nbt(&Tag::compound(Vec::<(&str, Tag)>::new())); // heightmaps: Horizon works them out
    w.byte_array(&buffer.data);
    w.varint(0); // block entities
    for mask in [sky_mask, block_mask, 0, 0] {
        if mask == 0 {
            w.varint(0);
        } else {
            w.varint(1);
            w.i64(mask as i64);
        }
    }
    for layers in [sky, block] {
        w.varint(layers.len() as i32);
        for layer in layers {
            w.byte_array(layer);
        }
    }
    Ok(w.data)
}

fn forget(x: i32, z: i32) -> Result<(i32, Vec<u8>)> {
    let mut w = Writer::new();
    ChunkPos::write(&[x, z], &mut w);
    Ok((down(State::Play, play::FORGET_LEVEL_CHUNK)?, w.data))
}

fn section_update(x: i32, y: i32, z: i32, changes: &[i64]) -> Result<(i32, Vec<u8>)> {
    let mut w = Writer::new();
    SectionPos::write(&[x, y, z], &mut w);
    w.varint(changes.len() as i32);
    for &change in changes {
        w.varlong(change);
    }
    Ok((down(State::Play, play::SECTION_BLOCKS_UPDATE)?, w.data))
}

fn column_packets(
    table: &Table,
    dimension: i8,
    x: i32,
    z: i32,
    column: &Column,
) -> Result<Vec<(i32, Vec<u8>)>> {
    if column.biomes.is_some() {
        let body = level_chunk(table, dimension, x, z, column)?;
        return Ok(vec![(
            down(State::Play, play::LEVEL_CHUNK_WITH_LIGHT)?,
            body,
        )]);
    }
    // Part of a column: 1.20.2 has no such packet, so each section sent is updated whole.
    let mut out = Vec::new();
    for (y, blocks, _, _) in &column.sections {
        let changes: Vec<i64> = states(table, blocks)
            .into_iter()
            .enumerate()
            .map(|(i, state)| {
                let (bx, bz, by) = (i & 15, (i >> 4) & 15, i >> 8);
                i64::from(state) << 12 | (bx << 8 | bz << 4 | by) as i64
            })
            .collect();
        out.push(section_update(x, *y as i32, z, &changes)?);
    }
    Ok(out)
}

/// The packets a 1.8 chunk packet (`id`) stands for, in the dimension the player is in.
pub(super) fn clientbound(
    table: &Table,
    dimension: i8,
    id: i32,
    payload: &[u8],
) -> Result<Vec<(i32, Vec<u8>)>> {
    let mut r = Reader::new(payload);
    match id {
        cb1_8::CHUNK_DATA => {
            let x = r.i32()?;
            let z = r.i32()?;
            let whole = r.bool()?;
            let mask = r.u16()?;
            let size = r.count(r.remaining())?;
            let mut data = Reader::new(r.bytes(size)?);
            if whole && mask == 0 {
                return Ok(vec![forget(x, z)?]);
            }
            let column = Column::read(&mut data, mask, has_sky(dimension), whole)?;
            column_packets(table, dimension, x, z, &column)
        }
        cb1_8::MAP_CHUNK_BULK => {
            let sky = r.bool()?;
            let n = r.count(r.remaining() / 10)?;
            let mut heads = Vec::with_capacity(n);
            for _ in 0..n {
                heads.push((r.i32()?, r.i32()?, r.u16()?));
            }
            let mut out = Vec::new();
            for (x, z, mask) in heads {
                let column = Column::read(&mut r, mask, sky, true)?;
                out.extend(column_packets(table, dimension, x, z, &column)?);
            }
            Ok(out)
        }
        cb1_8::BLOCK_CHANGE => {
            let pos = super::position(r.i64()?);
            let block = r.varint()?;
            let mut w = Writer::new();
            w.i64(pos);
            w.varint(table[(block & 0xFFF) as usize]);
            Ok(vec![(down(State::Play, play::BLOCK_UPDATE)?, w.data)])
        }
        cb1_8::MULTI_BLOCK_CHANGE => {
            let x = r.i32()?;
            let z = r.i32()?;
            let n = r.count(r.remaining() / 3)?;
            let mut by_section: BTreeMap<i32, Vec<i64>> = BTreeMap::new();
            for _ in 0..n {
                let xz = r.u8()?;
                let y = r.u8()?;
                let block = r.varint()?;
                let state = table[(block & 0xFFF) as usize];
                let local = i64::from(xz >> 4) << 8 | i64::from(xz & 15) << 4 | i64::from(y & 15);
                by_section
                    .entry(i32::from(y >> 4))
                    .or_default()
                    .push(i64::from(state) << 12 | local);
            }
            by_section
                .into_iter()
                .map(|(y, changes)| section_update(x, y, z, &changes))
                .collect()
        }
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::buf::unpack_block_pos;

    /// A made-up table: block number n is state n + 100, air stays air.
    pub(in crate::adapter::v1_8) fn test_table() -> Box<Table> {
        let mut table = Box::new([0; 4096]);
        for (n, slot) in table.iter_mut().enumerate().skip(16) {
            *slot = n as i32 + 100;
        }
        table
    }

    pub(in crate::adapter::v1_8) fn stone_column(x: i32, z: i32) -> Vec<u8> {
        // One section (y 0..16) of stone (1:0), lit 15 by the sky, then plains biomes.
        let mut data = Vec::new();
        for _ in 0..4096 {
            data.extend_from_slice(&(1u16 << 4).to_le_bytes());
        }
        data.extend(std::iter::repeat_n(0u8, 2048));
        data.extend(std::iter::repeat_n(0xFFu8, 2048));
        data.extend(std::iter::repeat_n(1u8, 256));
        let mut w = Writer::new();
        w.i32(x);
        w.i32(z);
        w.bool(true);
        w.u16(1);
        w.byte_array(&data);
        w.data
    }

    #[test]
    fn an_empty_whole_column_unloads_the_chunk() {
        let mut w = Writer::new();
        w.i32(5);
        w.i32(6);
        w.bool(true);
        w.u16(0);
        w.varint(0);
        let out = clientbound(&test_table(), 0, cb1_8::CHUNK_DATA, &w.data).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(ChunkPos::read(&mut Reader::new(&out[0].1)).unwrap(), [5, 6]);
    }

    #[test]
    fn a_bulk_holds_several_columns() {
        let a = stone_column(0, 0);
        let b = stone_column(1, 0);
        // A bulk: the sky flag, the heads, then each column's data without its own head.
        let data = |c: &[u8]| {
            let mut r = Reader::new(c);
            r.bytes(11).unwrap();
            let n = r.count(usize::MAX).unwrap();
            r.bytes(n).unwrap().to_vec()
        };
        let mut w = Writer::new();
        w.bool(true);
        w.varint(2);
        for x in [0, 1] {
            w.i32(x);
            w.i32(0);
            w.u16(1);
        }
        w.bytes(&data(&a));
        w.bytes(&data(&b));
        let out = clientbound(&test_table(), 0, cb1_8::MAP_CHUNK_BULK, &w.data).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_block_change_is_renumbered_and_repacked() {
        let table = test_table();
        let mut w = Writer::new();
        w.i64(((10i64 & 0x3FF_FFFF) << 38) | (64 << 26) | (-3i64 & 0x3FF_FFFF));
        w.varint(4 << 4 | 2); // cobblestone with meta 2
        let out = clientbound(&table, 0, cb1_8::BLOCK_CHANGE, &w.data).unwrap();
        let mut r = Reader::new(&out[0].1);
        assert_eq!(unpack_block_pos(r.i64().unwrap()), [10, 64, -3]);
        assert_eq!(r.varint().unwrap(), (4 << 4 | 2) + 100);
    }

    #[test]
    fn multi_block_changes_split_by_section() {
        let mut w = Writer::new();
        w.i32(2);
        w.i32(-1);
        w.varint(2);
        for y in [5u8, 70] {
            w.u8(0x3A); // x 3, z 10
            w.u8(y);
            w.varint(1 << 4);
        }
        let out = clientbound(&test_table(), 0, cb1_8::MULTI_BLOCK_CHANGE, &w.data).unwrap();
        assert_eq!(out.len(), 2);
        let mut r = Reader::new(&out[1].1);
        let section = SectionPos::read(&mut r).unwrap();
        assert_eq!(section, [2, 4, -1]);
        assert_eq!(r.varint().unwrap(), 1);
        let change = r.varlong().unwrap();
        assert_eq!(change >> 12, (1 << 4) + 100);
        assert_eq!(change & 0xFFF, 3 << 8 | 10 << 4 | (70 & 15));
    }

    #[test]
    fn the_real_table_reads_vanilla_blocks() {
        if !horizon_world::datafix::tables_available() {
            eprintln!("skipping: no prepared resources");
            return;
        }
        let table = table();
        assert_eq!(table[0], AIR);
        assert_ne!(table[1 << 4], STONE + 1000); // stone resolves
        // Each wool colour (35:0..15) is its own block.
        let wool: std::collections::HashSet<i32> = (0..16).map(|m| table[35 << 4 | m]).collect();
        assert_eq!(wool.len(), 16);
    }
}
