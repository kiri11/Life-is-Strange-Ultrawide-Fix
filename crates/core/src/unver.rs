//! Unversioned-property headers (fragments plus a zero mask), and the one
//! struct the fix decodes with them: `UCanvasPanelSlot`.
//!
//! Property values decode without a `.usmap` only because the schema is
//! hardcoded: unversioned property order is derived-class-first, then base
//! (`TFieldIterator` order), so for `UCanvasPanelSlot` it is
//! `0=LayoutData 1=bAutoSize 2=ZOrder 3=Parent 4=Content`. Object references
//! inside export payloads are 4-byte 1-based `FPackageIndex` values, not
//! the 8-byte `FPackageObjectIndex` used in the import and export maps.

fn u16_at(d: &[u8], p: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(d.get(p..p + 2).ok_or("truncated property header")?.try_into().unwrap()))
}

fn u32_at(d: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(d.get(p..p + 4).ok_or("truncated property header")?.try_into().unwrap()))
}

fn f32_at(d: &[u8], p: usize) -> Result<f32, String> {
    Ok(f32::from_le_bytes(d.get(p..p + 4).ok_or("truncated property value")?.try_into().unwrap()))
}

fn f64_at(d: &[u8], p: usize) -> Result<f64, String> {
    Ok(f64::from_le_bytes(d.get(p..p + 8).ok_or("truncated property value")?.try_into().unwrap()))
}

/// The properties present in a payload: `(schema index, is zero)` in order,
/// and how many bytes the header took.
pub fn parse_header(d: &[u8]) -> Result<(Vec<(usize, bool)>, usize), String> {
    let mut p = 0;
    let mut frags = Vec::new();
    loop {
        let packed = u16_at(d, p)?;
        p += 2;
        let skip = (packed & 0x7F) as usize;
        let has_zero = packed & 0x80 != 0;
        let is_last = packed & 0x100 != 0;
        let vnum = (packed >> 9) as usize;
        frags.push((skip, has_zero, vnum));
        if is_last {
            break;
        }
    }
    let nzero: usize = frags.iter().filter(|f| f.1).map(|f| f.2).sum();
    let mut zbits = Vec::new();
    if nzero > 0 {
        if nzero <= 8 {
            let m = *d.get(p).ok_or("truncated zero mask")?;
            p += 1;
            zbits.extend((0..8).map(|i| (m >> i) & 1 == 1));
        } else if nzero <= 16 {
            let m = u16_at(d, p)?;
            p += 2;
            zbits.extend((0..16).map(|i| (m >> i) & 1 == 1));
        } else {
            for _ in 0..nzero.div_ceil(32) {
                let m = u32_at(d, p)?;
                p += 4;
                zbits.extend((0..32).map(|i| (m >> i) & 1 == 1));
            }
        }
    }
    let mut out = Vec::new();
    let mut idx = 0;
    let mut zi = 0;
    for (skip, has_zero, vnum) in frags {
        idx += skip;
        for _ in 0..vnum {
            let z = if has_zero {
                let z = *zbits.get(zi).ok_or("zero mask too short")?;
                zi += 1;
                z
            } else {
                false
            };
            out.push((idx, z));
            idx += 1;
        }
    }
    Ok((out, p))
}

/// What a `UCanvasPanelSlot` says about its widget.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    /// `FMargin` Left, Top, Right, Bottom.
    pub offsets: [f32; 4],
    pub anchor_min: (f64, f64),
    pub anchor_max: (f64, f64),
    pub alignment: (f64, f64),
    /// 1-based `FPackageIndex`; 0 when absent.
    pub parent: i32,
    pub content: i32,
    /// `bAutoSize` and `ZOrder`, when the payload carries them; they are
    /// kept as they were when a slot is re-encoded.
    pub auto_size: Option<bool>,
    pub z_order: Option<i32>,
}

impl Slot {
    /// The export index the slot's `Content` points at.
    pub fn content_export(&self) -> Option<usize> {
        (self.content > 0).then(|| (self.content - 1) as usize)
    }
}

pub fn decode_slot(d: &[u8]) -> Result<Slot, String> {
    decode_slot_len(d).map(|(s, _)| s)
}

/// The slot and how many bytes of the payload its properties took; what
/// follows is the object's own trailer (four zero bytes in this game's
/// packages: no GUID).
pub fn decode_slot_len(d: &[u8]) -> Result<(Slot, usize), String> {
    let mut out = Slot {
        offsets: [0.0, 0.0, 100.0, 100.0],
        anchor_min: (0.0, 0.0),
        anchor_max: (0.0, 0.0),
        alignment: (0.0, 0.0),
        parent: 0,
        content: 0,
        auto_size: None,
        z_order: None,
    };
    let (props, mut p) = parse_header(d)?;
    for (idx, zero) in props {
        match idx {
            0 => {
                // LayoutData (FAnchorData)
                let (sub, used) = parse_header(&d[p..])?;
                p += used;
                for (sidx, szero) in sub {
                    match sidx {
                        0 => {
                            // Offsets (FMargin)
                            let (m, used) = parse_header(&d[p..])?;
                            p += used;
                            let mut vals = [0.0f32, 0.0, 100.0, 100.0];
                            for (mi, mz) in m {
                                if mi > 3 {
                                    return Err("FMargin has four fields".into());
                                }
                                if mz {
                                    vals[mi] = 0.0;
                                } else {
                                    vals[mi] = f32_at(d, p)?;
                                    p += 4;
                                }
                            }
                            out.offsets = vals;
                        }
                        1 => {
                            // Anchors (FAnchors)
                            let (a, used) = parse_header(&d[p..])?;
                            p += used;
                            for (ai, az) in a {
                                let v = if az {
                                    (0.0, 0.0)
                                } else {
                                    let v = (f64_at(d, p)?, f64_at(d, p + 8)?);
                                    p += 16;
                                    v
                                };
                                if ai == 0 {
                                    out.anchor_min = v;
                                } else {
                                    out.anchor_max = v;
                                }
                            }
                        }
                        2 => {
                            // Alignment (FVector2D)
                            if szero {
                                out.alignment = (0.0, 0.0);
                            } else {
                                out.alignment = (f64_at(d, p)?, f64_at(d, p + 8)?);
                                p += 16;
                            }
                        }
                        _ => return Err(format!("unknown FAnchorData field {sidx}")),
                    }
                }
            }
            1 => {
                // bAutoSize
                out.auto_size = Some(if zero {
                    false
                } else {
                    let v = *d.get(p).ok_or("truncated property value")? != 0;
                    p += 1;
                    v
                });
            }
            2 => {
                // ZOrder
                out.z_order = Some(if zero {
                    0
                } else {
                    let v = u32_at(d, p)? as i32;
                    p += 4;
                    v
                });
            }
            3 | 4 => {
                // Parent / Content (FPackageIndex, 1-based)
                let v = if zero {
                    0
                } else {
                    let v = u32_at(d, p)? as i32;
                    p += 4;
                    v
                };
                if idx == 3 {
                    out.parent = v;
                } else {
                    out.content = v;
                }
            }
            _ => return Err(format!("unknown UCanvasPanelSlot field {idx}")),
        }
    }
    Ok((out, p))
}

/// An unversioned header naming exactly `present` (ascending schema
/// indices), every value written out: no zero mask.
pub fn encode_header(present: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut next = 0;
    let mut i = 0;
    while i < present.len() {
        let skip = present[i] - next;
        let mut n = 1;
        while i + n < present.len() && present[i + n] == present[i] + n && n < 127 {
            n += 1;
        }
        i += n;
        next = present[i - 1] + 1;
        let last = i == present.len();
        let packed = (skip as u16) | ((n as u16) << 9) | if last { 0x100 } else { 0 };
        out.extend_from_slice(&packed.to_le_bytes());
    }
    out
}

/// The slot as a payload the engine reads back to the same values: every
/// field explicit (the zero mask is only an optimisation), and the four
/// zero trailer bytes of an object without a GUID.
pub fn encode_slot(s: &Slot) -> Vec<u8> {
    let mut top = vec![0];
    if s.auto_size.is_some() {
        top.push(1);
    }
    if s.z_order.is_some() {
        top.push(2);
    }
    top.extend([3, 4]);
    let mut out = encode_header(&top);
    // LayoutData: Offsets, Anchors, Alignment
    out.extend(encode_header(&[0, 1, 2]));
    out.extend(encode_header(&[0, 1, 2, 3]));
    for v in s.offsets {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend(encode_header(&[0, 1]));
    for (x, y) in [s.anchor_min, s.anchor_max, s.alignment] {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
    if let Some(b) = s.auto_size {
        out.push(b as u8);
    }
    if let Some(z) = s.z_order {
        out.extend_from_slice(&z.to_le_bytes());
    }
    out.extend_from_slice(&s.parent.to_le_bytes());
    out.extend_from_slice(&s.content.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BP_VideoWindow`'s video image slot (RESEARCH 13j): Left and Top
    /// absent, Right zero-masked, Bottom -1, only the anchor maximum set.
    const VIDEO_SLOT: [u8; 43] = [
        0x00, 0x02, 0x02, 0x05, 0x00, 0x05, 0x82, 0x05, 0x01, 0x00, 0x00, 0x80, 0xbf, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xf0, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x0a, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn encodes_headers() {
        assert_eq!(encode_header(&[0]), vec![0x00, 0x03]);
        assert_eq!(encode_header(&[0, 1, 2, 3]), vec![0x00, 0x09]);
        assert_eq!(encode_header(&[0, 3, 4]), vec![0x00, 0x02, 0x02, 0x05]);
        assert_eq!(encode_header(&[1]), vec![0x01, 0x03]);
        for present in [&[0usize, 3, 4][..], &[0, 1, 2, 3, 4], &[2, 5], &[0, 1, 2]] {
            let (props, used) = parse_header(&encode_header(present)).unwrap();
            assert_eq!(used, encode_header(present).len());
            assert_eq!(props, present.iter().map(|&i| (i, false)).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_slot_survives_encoding() {
        let (slot, used) = decode_slot_len(&VIDEO_SLOT).unwrap();
        assert_eq!(used, 39);
        assert_eq!(&VIDEO_SLOT[used..], &[0, 0, 0, 0]);
        assert_eq!(slot.offsets, [0.0, 0.0, 0.0, -1.0]);
        assert_eq!(slot.anchor_max, (1.0, 1.0));
        assert_eq!((slot.parent, slot.content, slot.auto_size, slot.z_order), (10, 12, None, None));

        let mut wide = slot.clone();
        wide.offsets = [640.0, 0.0, 640.0, -1.0];
        let bytes = encode_slot(&wide);
        assert_eq!(bytes.len(), 86);
        let (back, used) = decode_slot_len(&bytes).unwrap();
        assert_eq!(back, wide);
        assert_eq!(&bytes[used..], &[0, 0, 0, 0]);

        let full = Slot { auto_size: Some(true), z_order: Some(-3), ..wide };
        assert_eq!(decode_slot(&encode_slot(&full)).unwrap(), full);
    }

    #[test]
    fn header_fragments_and_zero_mask() {
        // one fragment: skip 0, 3 values, has zeros, last; mask 0b010 -> the middle one is zero
        let d = [0x80 | (3 << 9) as u8, ((0x100 | (3 << 9)) >> 8) as u8, 0b010];
        let packed = u16::from_le_bytes([d[0], d[1]]);
        assert_eq!(packed & 0x7F, 0);
        let (props, used) = parse_header(&d).unwrap();
        assert_eq!(used, 3);
        assert_eq!(props, vec![(0, false), (1, true), (2, false)]);
        assert!(parse_header(&[0x00]).is_err());
    }
}
