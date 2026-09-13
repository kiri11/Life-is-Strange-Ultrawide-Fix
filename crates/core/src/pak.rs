use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{hash, to_hex};

const MAGIC: u32 = 0x5a6f12e1;
const FOOTER: u64 = 222;
const LIMIT: u64 = 128 * 1024 * 1024;

type Result<T> = std::result::Result<T, String>;

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or("pak offset overflow")?;
        let bytes = self.data.get(self.at..end).ok_or("truncated pak index")?;
        self.at = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as i32;
        if n <= 0 {
            return Err("unsupported pak path encoding".into());
        }
        let bytes = self.take(n as usize)?;
        if bytes.last() != Some(&0) {
            return Err("unterminated pak path".into());
        }
        String::from_utf8(bytes[..bytes.len() - 1].to_vec()).map_err(|_| "invalid pak path".into())
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub offset: u64,
    pub size: u64,
    pub raw: u64,
    method: u32,
    digest: [u8; 20],
    blocks: Vec<(u64, u64)>,
    block_size: u32,
}

pub struct Pak {
    file: File,
    pub mount: String,
    pub entries: BTreeMap<String, Entry>,
    pub fingerprint: String,
}

fn read_at(file: &mut File, offset: u64, size: u64) -> Result<Vec<u8>> {
    if size > LIMIT {
        return Err("pak entry exceeds the UI reader's size limit".into());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

impl Pak {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let footer_at = len.checked_sub(FOOTER).ok_or("truncated pak footer")?;
        let footer = read_at(&mut file, footer_at, FOOTER)?;
        let mut r = Reader { data: &footer, at: 0 };
        if r.take(17)?.iter().any(|&b| b != 0) {
            return Err("encrypted pak archives are not supported".into());
        }
        if r.u32()? != MAGIC || r.u32()? != 9 {
            return Err("unsupported pak format (expected version 9)".into());
        }
        let (offset, size) = (r.u64()?, r.u64()?);
        let digest = r.take(20)?;
        if r.take(1)? != [0] {
            return Err("frozen pak indexes are not supported".into());
        }
        let method = r.take(32)?;
        if method.iter().any(|&b| b != 0) && (method[..4] != *b"Zlib" || method[4..].iter().any(|&b| b != 0)) {
            return Err("unsupported pak compression".into());
        }
        if offset.checked_add(size) != Some(footer_at) {
            return Err("invalid pak index bounds".into());
        }
        let index = read_at(&mut file, offset, size)?;
        if hash::sha1(&index) != digest {
            return Err("pak index checksum mismatch".into());
        }
        let fingerprint = to_hex(&hash::sha256(&index));
        let mut r = Reader { data: &index, at: 0 };
        let mount = r.string()?;
        let count = r.u32()?;
        if count as usize > index.len() / 58 {
            return Err("invalid pak entry count".into());
        }
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let name = r.string()?;
            let (entry_offset, size, raw, method) = (r.u64()?, r.u64()?, r.u64()?, r.u32()?);
            let digest = r.take(20)?.try_into().unwrap();
            let mut blocks = Vec::new();
            if method != 0 {
                let count = r.u32()? as usize;
                if count > index.len() / 16 {
                    return Err("invalid pak block count".into());
                }
                for _ in 0..count {
                    blocks.push((r.u64()?, r.u64()?));
                }
            }
            if r.take(1)? != [0] {
                return Err("encrypted or deleted pak entry".into());
            }
            let block_size = r.u32()?;
            let header_size = 53 + if method != 0 { 4 + blocks.len() as u64 * 16 } else { 0 };
            if entry_offset.checked_add(header_size).and_then(|v| v.checked_add(size)).is_none_or(|v| v > offset) {
                return Err("pak entry runs into the index".into());
            }
            if entries.insert(name, Entry { offset: entry_offset, size, raw, method, digest, blocks, block_size }).is_some() {
                return Err("duplicate pak path".into());
            }
        }
        if r.at != index.len() {
            return Err("unexpected pak index trailer".into());
        }
        Ok(Self { file, mount, entries, fingerprint })
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let e = self.entries.get(name).ok_or_else(|| format!("pak entry missing: {name}"))?.clone();
        if e.raw > LIMIT || e.size > LIMIT {
            return Err(format!("{name}: UI asset exceeds size limit"));
        }
        let mut data = Vec::with_capacity(e.raw as usize);
        if e.method == 0 {
            if e.size != e.raw {
                return Err("invalid uncompressed pak size".into());
            }
            data = read_at(&mut self.file, e.offset + 53, e.size)?;
            if hash::sha1(&data) != e.digest {
                return Err(format!("{name}: checksum mismatch"));
            }
        } else {
            if e.method != 1 || e.block_size == 0 {
                return Err("unsupported pak compression method".into());
            }
            let mut previous = 57 + e.blocks.len() as u64 * 16;
            let mut compressed = Vec::new();
            for &(start, end) in &e.blocks {
                if start != previous || end < start || end > 57 + e.blocks.len() as u64 * 16 + e.size {
                    return Err("invalid pak compression block".into());
                }
                let block = read_at(&mut self.file, e.offset + start, end - start)?;
                let want = (e.raw - data.len() as u64).min(e.block_size as u64) as usize;
                let decoded = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&block, want)
                    .map_err(|e| format!("{name}: zlib decompression failed ({e:?})"))?;
                if decoded.len() != want {
                    return Err("pak block size mismatch".into());
                }
                data.extend(decoded);
                compressed.extend(block);
                previous = end;
            }
            if compressed.len() as u64 != e.size || hash::sha1(&compressed) != e.digest {
                return Err(format!("{name}: compressed checksum mismatch"));
            }
        }
        if data.len() as u64 != e.raw {
            return Err(format!("{name}: decoded size mismatch"));
        }
        Ok(data)
    }
}

fn string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&((value.len() + 1) as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

fn entry(out: &mut Vec<u8>, offset: u64, data: &[u8]) {
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&hash::sha1(data));
    out.extend_from_slice(&[0; 5]);
}

pub fn build(mount: &str, files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut index = Vec::new();
    string(&mut index, mount);
    index.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, data) in files {
        let offset = out.len() as u64;
        entry(&mut out, 0, data);
        out.extend_from_slice(data);
        string(&mut index, name);
        entry(&mut index, offset, data);
    }
    let offset = out.len() as u64;
    out.extend_from_slice(&index);
    out.extend_from_slice(&[0; 17]);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&9u32.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&(index.len() as u64).to_le_bytes());
    out.extend_from_slice(&hash::sha1(&index));
    out.extend_from_slice(&[0; 161]);
    out
}
