//! IoStore containers: reading `.utoc`/`.ucas` (header, chunk table,
//! compression blocks, plaintext directory index) and writing a small
//! standalone container (RESEARCH.md sections 12 and 13i): the TOC's
//! perfect hash, `FIoContainerHeader` versions 2 and 4, the directory index
//! and a stub `.pak`.
//!
//! Two pieces had to be recovered from the shipped containers rather than
//! looked up:
//!
//! * the TOC's perfect hash - FNV-1 (multiply, then xor) over the 12 chunk-id
//!   bytes, 64 bits wide, seeded; `chunk_hash(0, id) % SeedCount` picks the
//!   seed, a negative seed *is* the slot, a positive one hashes to it. The
//!   modulo is taken on the full 64-bit value. Confirmed against every chunk
//!   of pakchunk0 and of a third-party mod container.
//! * `FIoContainerHeader` version 2 - package ids, then one 24-byte store
//!   entry each (export count, bundle count, and two `{count, offset-from-here}`
//!   array views), with the array data following the fixed block. Version 3
//!   dropped the two counts (16-byte entries) and version 4 appended a soft
//!   package reference table, empty in the game's own container; TOC
//!   version 8 shrank the per-chunk meta from a 32-byte hash plus flags to a
//!   20-byte hash, flags and padding (24 bytes).
//!
//! Everything is written uncompressed: compression only pays for size, the
//! container is ~120 KB, and it keeps the writer free of Oodle.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::hash;
use crate::kraken::{self, KrakenError};
use crate::zen::{ScriptObjects, fstring, parse_script_objects};

pub const BLOCK_SIZE: usize = 64 * 1024;
pub const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";
pub const HEADER_SIZE: usize = 144;
pub const CONTAINER_HEADER_MAGIC: u32 = 0x496F436E; // 'IoCn'
pub const FLAG_COMPRESSED: u8 = 1;
pub const FLAG_INDEXED: u8 = 8;
const NONE: u32 = 0xFFFF_FFFF;

/// Chunk types in an `FIoChunkId`'s last byte.
pub const CHUNK_PACKAGE_DATA: u8 = 1;
pub const CHUNK_SCRIPT_OBJECTS: u8 = 5;
pub const CHUNK_CONTAINER_HEADER: u8 = 6;

#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    Format(String),
    Decode(KrakenError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::Format(s) => f.write_str(s),
            ReadError::Decode(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

impl From<String> for ReadError {
    fn from(s: String) -> Self {
        ReadError::Format(s)
    }
}

impl From<KrakenError> for ReadError {
    fn from(e: KrakenError) -> Self {
        ReadError::Decode(e)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Block {
    pub offset: u64,
    pub compressed: u32,
    pub uncompressed: u32,
    pub method: u8,
}

/// A `.utoc` and the `.ucas` next to it.
pub struct Toc {
    pub path: PathBuf,
    ucas: File,
    pub version: u8,
    pub flags: u8,
    pub container_id: u64,
    pub block_size: usize,
    pub partitions: u32,
    pub seeds_count: u32,
    pub unhashed_count: u32,
    pub chunk_ids: Vec<[u8; 12]>,
    /// (virtual offset, length) per chunk.
    pub offlen: Vec<(u64, u64)>,
    pub seeds: Vec<i32>,
    pub blocks: Vec<Block>,
    pub methods: Vec<String>,
    pub directory: Vec<u8>,
    /// Path below the mount point (with its `../../../` stripped) -> chunk index.
    pub index: BTreeMap<String, usize>,
}

fn u32_le(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}

fn be_uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64)
}

fn le_uint(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64)
}

impl Toc {
    pub fn open(utoc: &Path) -> Result<Toc, ReadError> {
        let mut f = File::open(utoc)?;
        let mut h = [0u8; HEADER_SIZE];
        f.read_exact(&mut h)?;
        if &h[..16] != TOC_MAGIC {
            return Err(ReadError::Format(format!("{} is not a .utoc", utoc.display())));
        }
        let version = h[0x10];
        let entries = u32_le(&h, 0x18) as usize;
        let cblocks = u32_le(&h, 0x1C) as usize;
        let cmcount = u32_le(&h, 0x24) as usize;
        let cmlen = u32_le(&h, 0x28) as usize;
        let block_size = u32_le(&h, 0x2C) as usize;
        let diridxsize = u32_le(&h, 0x30) as usize;
        let partitions = u32_le(&h, 0x34);
        let container_id = u64::from_le_bytes(h[0x38..0x40].try_into().unwrap());
        let flags = h[0x50];
        let seeds_count = u32_le(&h, 0x54);
        let unhashed_count = u32_le(&h, 0x60);

        let mut ids = vec![0u8; entries * 12];
        f.read_exact(&mut ids)?;
        let chunk_ids = ids.chunks(12).map(|c| c.try_into().unwrap()).collect();
        let mut ol = vec![0u8; entries * 10];
        f.read_exact(&mut ol)?;
        let offlen = ol.chunks(10).map(|c| (be_uint(&c[..5]), be_uint(&c[5..]))).collect();
        let mut sb = vec![0u8; seeds_count as usize * 4];
        f.read_exact(&mut sb)?;
        let seeds = sb.chunks(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
        f.seek(SeekFrom::Current(unhashed_count as i64 * 4))?;
        let mut bb = vec![0u8; cblocks * 12];
        f.read_exact(&mut bb)?;
        let blocks = bb
            .chunks(12)
            .map(|r| Block {
                offset: le_uint(&r[..5]),
                compressed: le_uint(&r[5..8]) as u32,
                uncompressed: le_uint(&r[8..11]) as u32,
                method: r[11],
            })
            .collect();
        let mut mb = vec![0u8; cmcount * cmlen];
        f.read_exact(&mut mb)?;
        let mut methods = vec!["None".to_string()];
        for m in mb.chunks(cmlen) {
            let end = m.iter().position(|&b| b == 0).unwrap_or(m.len());
            methods.push(String::from_utf8_lossy(&m[..end]).into_owned());
        }
        let mut directory = vec![0u8; diridxsize];
        f.read_exact(&mut directory)?;
        let ucas = File::open(utoc.with_extension("ucas"))?;
        let index = if diridxsize > 0 { parse_directory_index(&directory)? } else { BTreeMap::new() };
        Ok(Toc {
            path: utoc.to_path_buf(),
            ucas,
            version,
            flags,
            container_id,
            block_size,
            partitions,
            seeds_count,
            unhashed_count,
            chunk_ids,
            offlen,
            seeds,
            blocks,
            methods,
            directory,
            index,
        })
    }

    pub fn entries(&self) -> usize {
        self.chunk_ids.len()
    }

    pub fn chunk_type(&self, i: usize) -> u8 {
        self.chunk_ids[i][11]
    }

    /// The first chunk of a type, if any.
    pub fn find_type(&self, kind: u8) -> Option<usize> {
        (0..self.entries()).find(|&i| self.chunk_type(i) == kind)
    }

    /// Chunk `i`, decompressed.
    pub fn read(&mut self, i: usize) -> Result<Vec<u8>, ReadError> {
        // the index comes from the directory index, which a foreign container
        // need not keep in step with its chunk table
        let &(off, len) = self.offlen.get(i).ok_or_else(|| ReadError::Format(format!("chunk {i} is past the chunk table")))?;
        if len == 0 {
            return Ok(Vec::new());
        }
        let bs = self.block_size as u64;
        let first = (off / bs) as usize;
        let last = ((off + len - 1) / bs) as usize;
        let mut out = Vec::with_capacity(len as usize + self.block_size);
        for bi in first..=last {
            let b = *self.blocks.get(bi).ok_or_else(|| ReadError::Format(format!("chunk {i} needs block {bi}, past the table")))?;
            self.ucas.seek(SeekFrom::Start(b.offset))?;
            let mut data = vec![0u8; b.compressed as usize];
            self.ucas.read_exact(&mut data)?;
            if b.method == 0 {
                out.extend_from_slice(&data[..(b.uncompressed as usize).min(data.len())]);
            } else {
                let name = self.methods.get(b.method as usize).map(String::as_str).unwrap_or("?");
                if !name.eq_ignore_ascii_case("Oodle") {
                    return Err(ReadError::Format(format!("block {bi} uses {name}, which the fix cannot decode")));
                }
                out.extend(kraken::decompress(&data, b.uncompressed as usize)?);
            }
        }
        let start = (off - first as u64 * bs) as usize;
        let end = (start + len as usize).min(out.len());
        out.drain(..start.min(out.len()));
        out.truncate(end - start.min(end));
        Ok(out)
    }

    /// The compressed blocks of every chunk, decoded one by one: what the
    /// research check runs over the game's own containers.
    pub fn read_block(&mut self, bi: usize) -> Result<Vec<u8>, ReadError> {
        let b = *self.blocks.get(bi).ok_or_else(|| ReadError::Format(format!("block {bi} is past the table")))?;
        self.ucas.seek(SeekFrom::Start(b.offset))?;
        let mut data = vec![0u8; b.compressed as usize];
        self.ucas.read_exact(&mut data)?;
        if b.method == 0 {
            return Ok(data);
        }
        Ok(kraken::decompress(&data, b.uncompressed as usize)?)
    }
}

/// The directory index as `path -> chunk index`, with the mount point's
/// `../../../` removed, so paths read `Chronos/Content/UI/...`.
pub fn parse_directory_index(buf: &[u8]) -> Result<BTreeMap<String, usize>, String> {
    let (mount, mut p) = fstring(buf, 0)?;
    let need = |p: usize, n: usize| -> Result<(), String> {
        if p + n > buf.len() { Err("truncated directory index".into()) } else { Ok(()) }
    };
    need(p, 4)?;
    let nd = u32_le(buf, p) as usize;
    p += 4;
    need(p, nd * 16)?;
    let dirs: Vec<[u32; 4]> = (0..nd)
        .map(|i| {
            let o = p + 16 * i;
            [u32_le(buf, o), u32_le(buf, o + 4), u32_le(buf, o + 8), u32_le(buf, o + 12)]
        })
        .collect();
    p += 16 * nd;
    need(p, 4)?;
    let nf = u32_le(buf, p) as usize;
    p += 4;
    need(p, nf * 12)?;
    let files: Vec<[u32; 3]> = (0..nf)
        .map(|i| {
            let o = p + 12 * i;
            [u32_le(buf, o), u32_le(buf, o + 4), u32_le(buf, o + 8)]
        })
        .collect();
    p += 12 * nf;
    need(p, 4)?;
    let ns = u32_le(buf, p) as usize;
    p += 4;
    let mut strs = Vec::with_capacity(ns);
    for _ in 0..ns {
        let (s, np) = fstring(buf, p)?;
        strs.push(s);
        p = np;
    }
    let mut out = BTreeMap::new();
    fn walk(
        dirs: &[[u32; 4]],
        files: &[[u32; 3]],
        strs: &[String],
        mut di: u32,
        prefix: &str,
        out: &mut BTreeMap<String, usize>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("directory index nests too deep".into());
        }
        while di != NONE {
            let [name, first_child, sibling, first_file] = *dirs.get(di as usize).ok_or("bad directory node")?;
            let path = if name == NONE {
                prefix.to_string()
            } else {
                format!("{prefix}{}/", strs.get(name as usize).ok_or("bad name index")?)
            };
            let mut fi = first_file;
            while fi != NONE {
                let [nm, next, ud] = *files.get(fi as usize).ok_or("bad file node")?;
                out.insert(format!("{path}{}", strs.get(nm as usize).ok_or("bad name index")?), ud as usize);
                fi = next;
            }
            if first_child != NONE {
                walk(dirs, files, strs, first_child, &path, out, depth + 1)?;
            }
            di = sibling;
        }
        Ok(())
    }
    walk(&dirs, &files, &strs, 0, &mount.replace("../../../", ""), &mut out, 0)?;
    Ok(out)
}

/// The script objects from `global.utoc`.
pub fn load_script_objects(global_utoc: &Path) -> Result<ScriptObjects, ReadError> {
    let mut t = Toc::open(global_utoc)?;
    match t.find_type(CHUNK_SCRIPT_OBJECTS) {
        Some(i) => {
            let buf = t.read(i)?;
            Ok(parse_script_objects(&buf)?)
        }
        None => Ok(ScriptObjects::new()),
    }
}

// --------------------------------------------------------------- perfect hash

const FNV1_BASIS: u64 = 0xCBF29CE484222325;
const FNV1_PRIME: u64 = 0x00000100000001B3;

/// The TOC's chunk-id hash: 64-bit FNV-1, `seed` replacing the basis.
pub fn chunk_hash(seed: u64, chunk_id: &[u8; 12]) -> u64 {
    let mut h = if seed != 0 { seed } else { FNV1_BASIS };
    for &b in chunk_id {
        h = h.wrapping_mul(FNV1_PRIME) ^ b as u64;
    }
    h
}

/// The engine's side of it - used to prove a written table resolves.
pub fn lookup(chunk_ids: &[[u8; 12]], seeds: &[i32], chunk_id: &[u8; 12]) -> Option<usize> {
    if seeds.is_empty() {
        return None;
    }
    let seed = seeds[(chunk_hash(0, chunk_id) % seeds.len() as u64) as usize];
    if seed == 0 {
        return None;
    }
    let slot = if seed < 0 {
        (-(seed as i64) - 1) as usize
    } else {
        (chunk_hash(seed as u64, chunk_id) % chunk_ids.len() as u64) as usize
    };
    (slot < chunk_ids.len() && chunk_ids[slot] == *chunk_id).then_some(slot)
}

/// -> (slot order, seeds). `order[slot]` indexes into `chunk_ids`.
///
/// The usual CHD construction: bucket by the unseeded hash, place the crowded
/// buckets first with a seed that spreads them over free slots, then drop the
/// single-chunk buckets into whatever is left and record the slot directly.
pub fn build_perfect_hash(chunk_ids: &[[u8; 12]]) -> Result<(Vec<usize>, Vec<i32>), String> {
    let n = chunk_ids.len();
    for seed_count in (n / 2).max(1)..n * 4 + 2 {
        if let Some((order, seeds)) = try_perfect_hash(chunk_ids, seed_count) {
            let placed: Vec<[u8; 12]> = order.iter().map(|&o| chunk_ids[o]).collect();
            for cid in chunk_ids {
                if lookup(&placed, &seeds, cid).is_none() {
                    return Err(format!("perfect hash does not resolve {}", crate::to_hex(cid)));
                }
            }
            return Ok((order, seeds));
        }
    }
    Err(format!("could not build a perfect hash for {n} chunks"))
}

fn try_perfect_hash(chunk_ids: &[[u8; 12]], seed_count: usize) -> Option<(Vec<usize>, Vec<i32>)> {
    let n = chunk_ids.len();
    // buckets in order of first appearance, as the reference implementation
    // iterated them, so the table it wrote is reproduced exactly
    let mut buckets: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut bucket_of: HashMap<usize, usize> = HashMap::new();
    for (i, cid) in chunk_ids.iter().enumerate() {
        let b = (chunk_hash(0, cid) % seed_count as u64) as usize;
        let k = *bucket_of.entry(b).or_insert_with(|| {
            buckets.push((b, Vec::new()));
            buckets.len() - 1
        });
        buckets[k].1.push(i);
    }
    let mut seeds = vec![0i32; seed_count];
    let mut order = vec![usize::MAX; n];
    let mut free = vec![true; n];
    let mut crowded: Vec<&(usize, Vec<usize>)> = buckets.iter().filter(|b| b.1.len() > 1).collect();
    crowded.sort_by_key(|b| std::cmp::Reverse(b.1.len()));
    for (bucket_index, members) in crowded {
        let mut found = false;
        for seed in 1..100_000u64 {
            let slots: Vec<usize> = members.iter().map(|&i| (chunk_hash(seed, &chunk_ids[i]) % n as u64) as usize).collect();
            let distinct = slots.iter().collect::<std::collections::HashSet<_>>().len() == slots.len();
            if distinct && slots.iter().all(|&s| free[s]) {
                for (&i, &s) in members.iter().zip(&slots) {
                    order[s] = i;
                    free[s] = false;
                }
                seeds[*bucket_index] = seed as i32;
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    for (bucket_index, members) in &buckets {
        if members.len() == 1 {
            let slot = free.iter().position(|&f| f)?;
            order[slot] = members[0];
            free[slot] = false;
            seeds[*bucket_index] = -(slot as i32) - 1;
        }
    }
    Some((order, seeds))
}

// ---------------------------------------------------------- container header

/// `FFilePackageStoreEntry` - what the loader needs to know about a package.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreEntry {
    /// Header version 2 only; zero from version 3 on, where the entry has
    /// no counts.
    pub exports: i32,
    pub bundles: i32,
    /// imported package ids
    pub imports: Vec<u64>,
    /// 20-byte `FSHAHash`es
    pub shader_hashes: Vec<[u8; 20]>,
}

/// Bytes per store entry: the counts went away with version 3.
fn store_entry_size(version: u32) -> usize {
    if version >= 3 { 16 } else { 24 }
}

/// -> (container id, package id -> entry) from a shipped container's header.
/// `version` is 2 (UE 5.2) or 4 (UE 5.4 and 5.5).
pub fn parse_container_header(data: &[u8], version: u32) -> Result<(u64, BTreeMap<u64, StoreEntry>), String> {
    if !matches!(version, 2 | 4) {
        return Err(format!("container header version {version} is not one the fix writes"));
    }
    if data.len() < 20 {
        return Err("container header too short".into());
    }
    let magic = u32_le(data, 0);
    if magic != CONTAINER_HEADER_MAGIC {
        return Err(format!("not a container header (magic {magic:08x})"));
    }
    let got = u32_le(data, 4);
    if got != version {
        return Err(format!("container header version {got}, expected {version}"));
    }
    let container_id = u64::from_le_bytes(data[8..16].try_into().unwrap());
    let mut p = 16;
    let count = u32_le(data, p) as usize;
    p += 4;
    if p + 8 * count + 4 > data.len() {
        return Err("package id table runs past the end of the header".into());
    }
    let package_ids: Vec<u64> = (0..count).map(|k| u64::from_le_bytes(data[p + 8 * k..p + 8 * k + 8].try_into().unwrap())).collect();
    p += 8 * count;
    let size = u32_le(data, p) as usize;
    p += 4;
    let base = p;
    let stride = store_entry_size(version);
    if base + size > data.len() || stride * count > size {
        return Err("store entries run past the end of the header".into());
    }
    let mut entries = BTreeMap::new();
    for (k, pid) in package_ids.iter().enumerate() {
        let mut at = base + stride * k;
        let (mut exports, mut bundles) = (0, 0);
        if stride == 24 {
            exports = u32_le(data, at) as i32;
            bundles = u32_le(data, at + 4) as i32;
            at += 8;
        }
        let imports = read_array(data, at, 8)?.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let shader_hashes = read_array(data, at + 8, 20)?.chunks(20).map(|c| c.try_into().unwrap()).collect();
        entries.insert(*pid, StoreEntry { exports, bundles, imports, shader_hashes });
    }
    Ok((container_id, entries))
}

/// A `TFilePackageStoreEntryCArrayView`: count, then offset from itself.
fn read_array(data: &[u8], at: usize, stride: usize) -> Result<&[u8], String> {
    let count = u32_le(data, at) as i32;
    let offset = u32_le(data, at + 4) as i32;
    if count == 0 {
        return Ok(&[]);
    }
    let start = at as i64 + offset as i64;
    if count < 0 || start < 0 {
        return Err("bad array view".into());
    }
    let start = start as usize;
    data.get(start..start + count as usize * stride).ok_or_else(|| "array view runs past the end of the header".into())
}

/// The chunk bytes for `entries` (package id -> entry), in header `version`
/// 2 or 4.
pub fn build_container_header(container_id: u64, entries: &BTreeMap<u64, StoreEntry>, version: u32) -> Vec<u8> {
    // BTreeMap iterates in sorted order: the loader binary-searches these
    let stride = store_entry_size(version);
    let mut fixed = vec![0u8; stride * entries.len()];
    let mut trailing: Vec<u8> = Vec::new();
    for (k, e) in entries.values().enumerate() {
        let mut at = stride * k;
        if stride == 24 {
            fixed[at..at + 4].copy_from_slice(&e.exports.to_le_bytes());
            fixed[at + 4..at + 8].copy_from_slice(&e.bundles.to_le_bytes());
            at += 8;
        }
        // offset is measured from the field itself, and the data sits after
        // the fixed block - which is what makes it a forward offset here.
        let mut view = |offset_at: usize, count: usize, bytes: Vec<u8>| {
            let data_at = fixed.len() + trailing.len();
            let offset = if count > 0 { (data_at - offset_at) as i32 } else { 0 };
            fixed[offset_at..offset_at + 4].copy_from_slice(&(count as i32).to_le_bytes());
            fixed[offset_at + 4..offset_at + 8].copy_from_slice(&offset.to_le_bytes());
            trailing.extend(bytes);
        };
        view(at, e.imports.len(), e.imports.iter().flat_map(|v| v.to_le_bytes()).collect());
        view(at + 8, e.shader_hashes.len(), e.shader_hashes.iter().flatten().copied().collect());
    }
    let mut out = Vec::new();
    out.extend_from_slice(&CONTAINER_HEADER_MAGIC.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&container_id.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for pid in entries.keys() {
        out.extend_from_slice(&pid.to_le_bytes());
    }
    out.extend_from_slice(&((fixed.len() + trailing.len()) as u32).to_le_bytes());
    out.extend(fixed);
    out.extend(trailing);
    // optional-segment package ids, their store entries, the redirect name map,
    // the localized-package table and the redirects - all empty here, as in
    // a mod container.
    out.extend_from_slice(&[0u8; 20]);
    if version >= 4 {
        // bContainsSoftPackageReferences = false: the game's own header
        // carries none either (RESEARCH 13i).
        out.extend_from_slice(&[0u8; 4]);
    }
    out
}

/// A container needs an id of its own; the name is the only input we have.
pub fn container_id_for(name: &str) -> u64 {
    let digest = hash::sha256(name.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().unwrap()) & 0x7FFF_FFFF_FFFF_FFFF
}

pub fn package_id_of(chunk_id: &[u8; 12]) -> u64 {
    u64::from_le_bytes(chunk_id[..8].try_into().unwrap())
}

pub fn package_data_chunk_id(package_id: u64, index: u16) -> [u8; 12] {
    let mut id = [0u8; 12];
    id[..8].copy_from_slice(&package_id.to_le_bytes());
    id[8..10].copy_from_slice(&index.to_le_bytes());
    id[11] = CHUNK_PACKAGE_DATA;
    id
}

pub fn container_header_chunk_id(container_id: u64) -> [u8; 12] {
    let mut id = [0u8; 12];
    id[..8].copy_from_slice(&container_id.to_le_bytes());
    id[11] = CHUNK_CONTAINER_HEADER;
    id
}

// ---------------------------------------------------------- directory index

fn fstring_bytes(s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    let mut out = (b.len() as i32).to_le_bytes().to_vec();
    out.extend(b);
    out
}

/// `files`: (path below the mount point, chunk slot) -> the index bytes.
pub fn build_directory_index(mount_point: &str, files: &[(String, usize)]) -> Vec<u8> {
    let mut strings: Vec<String> = Vec::new();
    let mut string_index: HashMap<String, u32> = HashMap::new();
    let mut intern = |s: &str| -> u32 {
        if let Some(&i) = string_index.get(s) {
            return i;
        }
        strings.push(s.to_string());
        let i = (strings.len() - 1) as u32;
        string_index.insert(s.to_string(), i);
        i
    };
    // directory nodes: [name, first child, next sibling, first file]
    let mut dirs: Vec<[u32; 4]> = vec![[NONE, NONE, NONE, NONE]];
    let mut children: Vec<HashMap<String, usize>> = vec![HashMap::new()];
    let mut file_nodes: Vec<[u32; 3]> = Vec::new();

    for (path, slot) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let mut node = 0usize;
        for part in &parts[..parts.len() - 1] {
            if !children[node].contains_key(*part) {
                let new = dirs.len();
                dirs.push([intern(part), NONE, dirs[node][1], NONE]);
                dirs[node][1] = new as u32; // newest child first, as UE does
                children[node].insert(part.to_string(), new);
                children.push(HashMap::new());
            }
            node = children[node][*part];
        }
        file_nodes.push([intern(parts[parts.len() - 1]), dirs[node][3], *slot as u32]);
        dirs[node][3] = (file_nodes.len() - 1) as u32;
    }

    let mut out = fstring_bytes(mount_point);
    out.extend_from_slice(&(dirs.len() as u32).to_le_bytes());
    for d in &dirs {
        for v in d {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.extend_from_slice(&(file_nodes.len() as u32).to_le_bytes());
    for f in &file_nodes {
        for v in f {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    for s in &strings {
        out.extend(fstring_bytes(s));
    }
    out
}

// ---------------------------------------------------------------- the writer

/// One chunk to write: its id, its bytes, and its path below the mount
/// point when it belongs in the directory index.
pub struct Chunk {
    pub id: [u8; 12],
    pub data: Vec<u8>,
    pub path: Option<String>,
}

/// The bytes of `<name>.utoc` and `<name>.ucas` for `chunks`, in whatever
/// order; the perfect hash decides where each one lands. `toc_version` is 5
/// (UE 5.2) or 8 (UE 5.5); the two differ only in the per-chunk meta.
pub fn build_container(mount_point: &str, chunks: &[Chunk], container_id: u64, toc_version: u8) -> Result<(Vec<u8>, Vec<u8>), String> {
    let chunk_ids: Vec<[u8; 12]> = chunks.iter().map(|c| c.id).collect();
    let (order, seeds) = build_perfect_hash(&chunk_ids)?;
    let mut slot_of: HashMap<[u8; 12], usize> = HashMap::new();
    for (slot, &c) in order.iter().enumerate() {
        slot_of.insert(chunk_ids[c], slot);
    }

    let mut ucas: Vec<u8> = Vec::new();
    let mut blocks: Vec<(u64, usize, u8)> = Vec::new(); // (offset, size, method)
    let mut entries: Vec<(u64, u64)> = Vec::with_capacity(chunks.len()); // (virtual offset, length)
    let mut virtual_off: u64 = 0;
    for &c in &order {
        let data = &chunks[c].data;
        entries.push((virtual_off, data.len() as u64));
        let mut at = 0;
        loop {
            let piece = &data[at..(at + BLOCK_SIZE).min(data.len())];
            while !ucas.len().is_multiple_of(16) {
                ucas.push(0); // the engine reads aligned
            }
            blocks.push((ucas.len() as u64, piece.len(), 0));
            ucas.extend_from_slice(piece);
            virtual_off += BLOCK_SIZE as u64;
            at += BLOCK_SIZE;
            if at >= data.len() {
                break;
            }
        }
    }

    let mut indexed: Vec<(String, usize)> =
        chunks.iter().filter_map(|c| c.path.as_ref().map(|p| (p.clone(), slot_of[&c.id]))).collect();
    indexed.sort();
    let directory = build_directory_index(mount_point, &indexed);

    let mut header = vec![0u8; HEADER_SIZE];
    header[..16].copy_from_slice(TOC_MAGIC);
    header[0x10] = toc_version;
    let fields: [u32; 9] =
        [HEADER_SIZE as u32, chunks.len() as u32, blocks.len() as u32, 12, 1, 32, BLOCK_SIZE as u32, directory.len() as u32, 1];
    for (i, v) in fields.iter().enumerate() {
        header[0x14 + 4 * i..0x18 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    header[0x38..0x40].copy_from_slice(&container_id.to_le_bytes());
    header[0x50] = FLAG_COMPRESSED | FLAG_INDEXED;
    header[0x54..0x58].copy_from_slice(&(seeds.len() as u32).to_le_bytes());
    header[0x58..0x60].copy_from_slice(&u64::MAX.to_le_bytes()); // one partition
    header[0x60..0x64].copy_from_slice(&0u32.to_le_bytes()); // none unhashed

    let mut out = header;
    for &c in &order {
        out.extend_from_slice(&chunk_ids[c]);
    }
    for (off, len) in &entries {
        out.extend_from_slice(&off.to_be_bytes()[3..]);
        out.extend_from_slice(&len.to_be_bytes()[3..]);
    }
    for s in &seeds {
        out.extend_from_slice(&s.to_le_bytes());
    }
    for (off, size, method) in &blocks {
        out.extend_from_slice(&off.to_le_bytes()[..5]);
        out.extend_from_slice(&(*size as u32).to_le_bytes()[..3]);
        out.extend_from_slice(&(*size as u32).to_le_bytes()[..3]);
        out.push(*method);
    }
    let mut method_name = b"Oodle".to_vec();
    method_name.resize(32, 0);
    out.extend(method_name);
    out.extend(&directory);
    // FIoStoreTocEntryMeta: the chunk hash and a flags byte (0: not
    // compressed). Up to version 7 the hash field is 32 bytes with the
    // 20-byte BLAKE3 in front; version 8 keeps the 20 bytes and pads to 24.
    let pad = if toc_version >= 8 { 3 } else { 12 };
    for &c in &order {
        out.extend(hash::blake3(&chunks[c].data, 20));
        out.extend(vec![0u8; pad + 1]);
    }
    Ok((out, ucas))
}

/// An empty `.pak` next to the container.
///
/// The engine finds IoStore containers through pak mounting, so a mod needs
/// a `.pak` even when every byte it ships lives in the `.ucas`. This is the
/// smallest valid one: an index with no files at all.
pub fn stub_pak() -> Vec<u8> {
    let path_hash_index = [0u8; 8];
    let full_directory_index = [0u8; 4];

    let mut index = fstring_bytes("/");
    index.extend_from_slice(&0i32.to_le_bytes()); // no entries
    index.extend_from_slice(&0u64.to_le_bytes()); // path hash seed
    let head = (index.len() + 4 + 16 + 20 + 4 + 16 + 20 + 4 + 4) as i64; // where sub-indexes go
    index.extend_from_slice(&1i32.to_le_bytes());
    index.extend_from_slice(&head.to_le_bytes());
    index.extend_from_slice(&(path_hash_index.len() as i64).to_le_bytes());
    index.extend_from_slice(&hash::sha1(&path_hash_index));
    index.extend_from_slice(&1i32.to_le_bytes());
    index.extend_from_slice(&(head + path_hash_index.len() as i64).to_le_bytes());
    index.extend_from_slice(&(full_directory_index.len() as i64).to_le_bytes());
    index.extend_from_slice(&hash::sha1(&full_directory_index));
    index.extend_from_slice(&[0u8; 8]); // encoded entries, files

    let mut footer = vec![0u8; 17]; // encryption guid, not encrypted
    footer.extend_from_slice(&0x5A6F12E1u32.to_le_bytes());
    footer.extend_from_slice(&11u32.to_le_bytes());
    footer.extend_from_slice(&0i64.to_le_bytes());
    footer.extend_from_slice(&(index.len() as i64).to_le_bytes());
    footer.extend_from_slice(&hash::sha1(&index));
    footer.extend(vec![0u8; 32 * 5]); // compression method names

    let mut out = index;
    out.extend_from_slice(&path_hash_index);
    out.extend_from_slice(&full_directory_index);
    out.extend(footer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_hash_resolves_everything_it_placed() {
        let ids: Vec<[u8; 12]> = (0..37u8).map(|i| package_data_chunk_id(0x1234_5678_9abc_def0 ^ (i as u64 * 0x9E37_79B9), 0)).collect();
        let (order, seeds) = build_perfect_hash(&ids).unwrap();
        let placed: Vec<[u8; 12]> = order.iter().map(|&o| ids[o]).collect();
        for (i, id) in ids.iter().enumerate() {
            let slot = lookup(&placed, &seeds, id).unwrap();
            assert_eq!(order[slot], i);
        }
        assert!(lookup(&placed, &seeds, &[9u8; 12]).is_none());
        assert_eq!(chunk_hash(0, &[0u8; 12]), {
            let mut h = FNV1_BASIS;
            for _ in 0..12 {
                h = h.wrapping_mul(FNV1_PRIME);
            }
            h
        });
    }

    #[test]
    fn container_header_round_trips() {
        let mut entries = BTreeMap::new();
        entries.insert(7u64, StoreEntry { exports: 3, bundles: 1, imports: vec![1, 2], shader_hashes: vec![] });
        entries.insert(2u64, StoreEntry { exports: 5, bundles: 1, imports: vec![], shader_hashes: vec![[3u8; 20]] });
        let data = build_container_header(0xABCD, &entries, 2);
        let (id, back) = parse_container_header(&data, 2).unwrap();
        assert_eq!(id, 0xABCD);
        assert_eq!(back, entries);
        assert!(parse_container_header(&data, 4).is_err());

        // version 4: no counts, and the empty soft-reference table at the end
        for e in entries.values_mut() {
            e.exports = 0;
            e.bundles = 0;
        }
        let data4 = build_container_header(0xABCD, &entries, 4);
        assert_eq!(data4.len(), data.len() - 2 * 8 + 4);
        assert!(data4.ends_with(&[0u8; 24]));
        let (id, back) = parse_container_header(&data4, 4).unwrap();
        assert_eq!(id, 0xABCD);
        assert_eq!(back, entries);
        assert!(parse_container_header(&data4, 2).is_err());
        assert!(parse_container_header(&data4, 3).is_err());
    }

    #[test]
    fn toc_meta_follows_the_version() {
        let chunks = vec![Chunk { id: package_data_chunk_id(1, 0), data: vec![7; 10], path: None }];
        let (v5, _) = build_container("../../../X/Content/", &chunks, 1, 5).unwrap();
        let (v8, _) = build_container("../../../X/Content/", &chunks, 1, 8).unwrap();
        assert_eq!(v5.len() - v8.len(), 33 - 24);
        assert_eq!(v5[0x10], 5);
        assert_eq!(v5[0x11..v5.len() - 33], v8[0x11..v8.len() - 24]);
        assert_eq!(v5[v5.len() - 33..v5.len() - 13], v8[v8.len() - 24..v8.len() - 4]);
        assert_eq!(v8[0x10], 8);
    }

    #[test]
    fn a_written_container_reads_back() {
        let dir = std::env::temp_dir().join(format!("lis-iostore-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big: Vec<u8> = (0..150_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let chunks = vec![
            Chunk { id: package_data_chunk_id(11, 0), data: b"hello".to_vec(), path: Some("UI/a.uasset".into()) },
            Chunk { id: package_data_chunk_id(12, 0), data: big.clone(), path: Some("UI/Sub/b.uasset".into()) },
            Chunk { id: container_header_chunk_id(99), data: vec![1, 2, 3], path: None },
        ];
        let (utoc, ucas) = build_container("../../../Chronos/Content/", &chunks, 99, 5).unwrap();
        let base = dir.join("Test_P");
        std::fs::write(base.with_extension("utoc"), &utoc).unwrap();
        std::fs::write(base.with_extension("ucas"), &ucas).unwrap();
        let mut toc = Toc::open(&base.with_extension("utoc")).unwrap();
        assert_eq!(toc.entries(), 3);
        assert_eq!(toc.container_id, 99);
        assert_eq!(toc.version, 5);
        assert_eq!(toc.index.len(), 2);
        let i = toc.index["Chronos/Content/UI/Sub/b.uasset"];
        assert_eq!(toc.read(i).unwrap(), big);
        let i = toc.index["Chronos/Content/UI/a.uasset"];
        assert_eq!(toc.read(i).unwrap(), b"hello");
        let h = toc.find_type(CHUNK_CONTAINER_HEADER).unwrap();
        assert_eq!(toc.read(h).unwrap(), vec![1, 2, 3]);
        for (slot, id) in toc.chunk_ids.clone().iter().enumerate() {
            assert_eq!(lookup(&toc.chunk_ids, &toc.seeds, id), Some(slot));
        }
        assert_eq!(stub_pak().len(), 339);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
