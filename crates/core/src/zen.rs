//! Zen (cooked IoStore) packages: the name batch, import and export maps,
//! and where each export's payload is.
//!
//! Where an export's payload is depends on the engine. UE 5.2 lays them
//! out in export-bundle order, not export-map order:
//! `data_offset = HeaderSize + accumulated sizes in bundle order`. UE 5.3
//! replaced the graph data with dependency bundles and added the imported
//! package names, which pushed the name batch from +44 to +52, and from
//! then on the payloads sit at `HeaderSize + CookedSerialOffset` in
//! export-map order (RESEARCH 13i).

use std::collections::HashMap;

pub type ScriptObjects = HashMap<u64, String>;

fn u32_at(b: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(b.get(p..p + 4).ok_or("truncated package")?.try_into().unwrap()))
}

fn u64_at(b: &[u8], p: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(b.get(p..p + 8).ok_or("truncated package")?.try_into().unwrap()))
}

/// An `FString`: length-prefixed, negative for UTF-16.
pub fn fstring(buf: &[u8], p: usize) -> Result<(String, usize), String> {
    let n = u32_at(buf, p)? as i32;
    let p = p + 4;
    if n == 0 {
        return Ok((String::new(), p));
    }
    if n < 0 {
        let chars = (-n) as usize;
        let bytes = buf.get(p..p + chars * 2).ok_or("truncated string")?;
        let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&units[..units.len().saturating_sub(1)]);
        return Ok((text, p + chars * 2));
    }
    let n = n as usize;
    let bytes = buf.get(p..p + n).ok_or("truncated string")?;
    Ok((String::from_utf8_lossy(&bytes[..n.saturating_sub(1)]).into_owned(), p + n))
}

/// A name batch: count, byte size, hash version, hashes, then headers and
/// the strings themselves.
pub fn load_name_batch(buf: &[u8], mut p: usize) -> Result<(Vec<String>, usize), String> {
    let num = u32_at(buf, p)? as usize;
    p += 8; // count, byte size
    if num == 0 {
        return Ok((Vec::new(), p));
    }
    p += 8; // HashVersion
    p += num * 8; // Hashes
    let mut hdrs = Vec::with_capacity(num);
    for _ in 0..num {
        let h = u16::from_be_bytes(buf.get(p..p + 2).ok_or("truncated name batch")?.try_into().unwrap());
        p += 2;
        hdrs.push((h & 0x8000 != 0, (h & 0x7FFF) as usize));
    }
    let mut names = Vec::with_capacity(num);
    for (utf16, len) in hdrs {
        if utf16 {
            let bytes = buf.get(p..p + len * 2).ok_or("truncated name batch")?;
            let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            names.push(String::from_utf16_lossy(&units));
            p += len * 2;
        } else {
            let bytes = buf.get(p..p + len).ok_or("truncated name batch")?;
            names.push(String::from_utf8_lossy(bytes).into_owned());
            p += len;
        }
    }
    Ok((names, p))
}

/// The script objects of `global.utoc`'s script-object chunk (type 5):
/// global import index -> full object path, `/Script/UMG.CanvasPanelSlot`.
pub fn parse_script_objects(buf: &[u8]) -> Result<ScriptObjects, String> {
    let (names, mut p) = load_name_batch(buf, 0)?;
    let n = u32_at(buf, p)? as usize;
    p += 4;
    let mut ents: HashMap<u64, (String, u64)> = HashMap::with_capacity(n);
    for k in 0..n {
        let at = p + 32 * k;
        let nm = u64_at(buf, at)?;
        let gi = u64_at(buf, at + 8)?;
        let oi = u64_at(buf, at + 16)?;
        let idx = (nm & 0x3FFFFFFF) as usize;
        ents.insert(gi, (names.get(idx).cloned().unwrap_or_else(|| "?".into()), oi));
    }
    fn full(ents: &HashMap<u64, (String, u64)>, gi: u64, depth: u32) -> Option<String> {
        let (nm, oi) = ents.get(&gi)?;
        if depth > 12 {
            return None;
        }
        let parent = if *oi != u64::MAX { full(ents, *oi, depth + 1) } else { None };
        Some(match parent {
            Some(p) => format!("{p}.{nm}"),
            None => nm.clone(),
        })
    }
    Ok(ents.keys().map(|&gi| (gi, full(&ents, gi, 0).unwrap_or_else(|| ents[&gi].0.clone()))).collect())
}

#[derive(Debug, Clone)]
pub struct Export {
    pub index: usize,
    pub cooked_offset: u64,
    pub size: u64,
    pub name: String,
    pub outer: u64,
    pub class: u64,
    pub super_: u64,
    pub template: u64,
}

pub struct ZenPackage<'a> {
    pub buf: &'a [u8],
    pub header_size: usize,
    pub name: String,
    pub names: Vec<String>,
    pub imports: Vec<u64>,
    pub exports: Vec<Export>,
    /// export index -> offset of its payload in the chunk
    layout: HashMap<usize, usize>,
}

/// Which `FZenPackageSummary` a package carries. There is no version field
/// to read (`bHasVersioningInfo` is 0 in cooked data), so the game says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summary {
    /// UE 5.2: `GraphDataOffset` at +40, the name batch at +44.
    Ue52,
    /// UE 5.3 to 5.5: `DependencyBundleHeadersOffset` at +40,
    /// `DependencyBundleEntriesOffset` at +44, `ImportedPackageNamesOffset`
    /// at +48, the name batch at +52.
    Ue53,
}

impl Summary {
    /// Where the name batch starts.
    fn names_at(self) -> usize {
        match self {
            Summary::Ue52 => 44,
            Summary::Ue53 => 52,
        }
    }
}

/// What an `FPackageObjectIndex` refers to.
#[derive(Debug, PartialEq)]
pub enum ObjectRef {
    Export(usize),
    ScriptImport(u64),
    PackageImport(u64),
    Null,
}

pub fn object_ref(v: u64) -> ObjectRef {
    let idx = v & ((1u64 << 62) - 1);
    match v >> 62 {
        0 => ObjectRef::Export(idx as usize),
        1 => ObjectRef::ScriptImport(v),
        2 => ObjectRef::PackageImport(idx),
        _ => ObjectRef::Null,
    }
}

impl<'a> ZenPackage<'a> {
    pub fn parse(buf: &'a [u8], summary: Summary) -> Result<Self, String> {
        let header_size = u32_at(buf, 4)? as usize;
        let name_idx = (u32_at(buf, 8)? & 0x3FFFFFFF) as usize;
        let import_off = u32_at(buf, 28)? as usize;
        let export_off = u32_at(buf, 32)? as usize;
        let bundle_off = u32_at(buf, 36)? as usize;
        // what follows the export-bundle entries: the graph data (5.2) or
        // the dependency bundle headers (5.3+); either way their end
        let graph_off = u32_at(buf, 40)? as usize;
        let (names, _) = load_name_batch(buf, summary.names_at())?;
        let name = names.get(name_idx).cloned().ok_or("package name index out of range")?;
        if import_off > export_off || export_off > bundle_off || bundle_off > graph_off || graph_off > buf.len() {
            return Err("package header offsets are out of order".into());
        }
        let imports = (0..(export_off - import_off) / 8).map(|i| u64_at(buf, import_off + 8 * i)).collect::<Result<_, _>>()?;
        let n = (bundle_off - export_off) / 72;
        let mut exports = Vec::with_capacity(n);
        for i in 0..n {
            let o = export_off + 72 * i;
            let objname = (u32_at(buf, o + 16)? & 0x3FFFFFFF) as usize;
            exports.push(Export {
                index: i,
                cooked_offset: u64_at(buf, o)?,
                size: u64_at(buf, o + 8)?,
                name: names.get(objname).cloned().unwrap_or_else(|| "?".into()),
                outer: u64_at(buf, o + 24)?,
                class: u64_at(buf, o + 32)?,
                super_: u64_at(buf, o + 40)?,
                template: u64_at(buf, o + 48)?,
            });
        }
        let mut layout = HashMap::new();
        match summary {
            Summary::Ue52 => {
                // export data is stored in export-bundle (Serialize command) order
                let mut pos = header_size;
                for i in 0..(graph_off - bundle_off) / 8 {
                    let li = u32_at(buf, bundle_off + 8 * i)? as usize;
                    let cmd = u32_at(buf, bundle_off + 8 * i + 4)?;
                    if cmd == 1 {
                        layout.insert(li, pos);
                        pos += exports.get(li).ok_or("bundle entry names a missing export")?.size as usize;
                    }
                }
            }
            Summary::Ue53 => {
                // export data is stored in export-map order, and each entry's
                // cooked offset counts from the end of the header
                for e in &exports {
                    layout.insert(e.index, header_size + e.cooked_offset as usize);
                }
            }
        }
        Ok(ZenPackage { buf, header_size, name, names, imports, exports, layout })
    }

    /// Where an export's payload starts in the chunk.
    pub fn export_offset(&self, index: usize) -> Option<usize> {
        self.layout.get(&index).copied()
    }

    pub fn export_data(&self, e: &Export) -> Option<&'a [u8]> {
        let o = self.export_offset(e.index)?;
        self.buf.get(o..o + e.size as usize)
    }

    /// The full path of a script import (`/Script/UMG.CanvasPanelSlot`),
    /// when the reference is one and the script objects know it.
    pub fn script_class<'s>(&self, v: u64, so: &'s ScriptObjects) -> Option<&'s str> {
        match object_ref(v) {
            ObjectRef::ScriptImport(key) => so.get(&key).map(String::as_str),
            _ => None,
        }
    }
}
