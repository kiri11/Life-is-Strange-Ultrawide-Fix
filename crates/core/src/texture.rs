//! Cooked `UTexture2D` payloads (RESEARCH 13k): where each mip's pixel data
//! sits, so that a mip can be rewritten in place.
//!
//! The layout, the same in the UE 5.2 and 5.5 packages this fix reads:
//! the unversioned properties, then the native part - a strip-flags word,
//! the `PF_*` pixel format name, a 64-bit skip offset, sixteen bytes,
//! `SizeX`, `SizeY`, `PackedData`, the pixel format as an `FString`,
//! `FirstMipToSerialize`, `NumMips`, then each mip: one 32-bit word, its
//! pixel data inline with no other header (the bulk-data meta lives in the
//! package's bulk data map), followed by its `SizeX`, `SizeY`, `SizeZ`; then the
//! virtual flag and the `None` name that ends the platform data list. The
//! parser finds the pixel format string and walks forward from it, and
//! checks every mip's trailer against the size it computed.

use crate::bc::Format;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mip {
    pub width: usize,
    pub height: usize,
    /// Byte range of the pixel data within the payload.
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub format_name: String,
    pub format: Option<Format>,
    pub width: usize,
    pub height: usize,
    pub mips: Vec<Mip>,
}

fn u32_at(d: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(d.get(p..p + 4).ok_or("truncated texture payload")?.try_into().unwrap()))
}

/// Where the mips of a cooked `Texture2D` export are.
pub fn parse_texture(payload: &[u8]) -> Result<Texture, String> {
    // the FString: u32 length (with NUL), "PF_...", NUL
    let at = (4..payload.len().saturating_sub(4))
        .find(|&p| &payload[p..p + 3] == b"PF_" && (5..=40).contains(&(u32_at(payload, p - 4).unwrap_or(0) as usize)))
        .ok_or("no pixel format string")?;
    let n = u32_at(payload, at - 4)? as usize;
    if payload.get(at + n - 1) != Some(&0) {
        return Err("pixel format string is not NUL-terminated".into());
    }
    let format_name = String::from_utf8_lossy(&payload[at..at + n - 1]).into_owned();
    let format = Format::from_name(&format_name);
    let width = u32_at(payload, at - 16)? as usize;
    let height = u32_at(payload, at - 12)? as usize;
    let _packed = u32_at(payload, at - 8)?;
    let mut p = at + n;
    let _first_mip = u32_at(payload, p)?;
    let num_mips = u32_at(payload, p + 4)? as usize;
    p += 8;
    let Some(format) = format else {
        return Ok(Texture { format_name, format, width, height, mips: Vec::new() });
    };
    let (mut w, mut h) = (width, height);
    let mut mips = Vec::with_capacity(num_mips);
    for i in 0..num_mips {
        // one 32-bit word ahead of each mip's data: it reads as 0, -1 or
        // leftovers in the shipped textures, so nothing reads it; left as is
        p += 4;
        let len = format.mip_size(w, h);
        if p + len + 12 > payload.len() {
            return Err(format!("mip {i} ({w}x{h}) runs past the payload"));
        }
        let trailer = (u32_at(payload, p + len)? as usize, u32_at(payload, p + len + 4)? as usize, u32_at(payload, p + len + 8)?);
        if trailer != (w, h, 1) {
            return Err(format!("mip {i}: expected a {w}x{h}x1 trailer, found {trailer:?}"));
        }
        mips.push(Mip { width: w, height: h, offset: p, len });
        p += len + 12;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    // bIsVirtual, then the None name that ends the list of platform data
    if p + 12 != payload.len() || u32_at(payload, p)? != 0 || payload[p + 4..p + 12] != [0; 8] {
        return Err(format!("{} bytes after the last mip, expected the 12 that end the texture", payload.len() - p));
    }
    Ok(Texture { format_name, format: Some(format), width, height, mips })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload in the cooked shape: some property bytes, the skip
    /// offset and the sixteen bytes, the sizes, the format string, the
    /// mip counts, the word ahead of the mip, one 4x4 BC1 mip, its
    /// trailer, the end.
    fn payload(format: &str, mip: &[u8]) -> Vec<u8> {
        let mut d = vec![0x04, 0x03, 0x01, 0x00, 0x00, 0x00];
        d.extend_from_slice(&[0; 24]);
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&((format.len() + 1) as u32).to_le_bytes());
        d.extend_from_slice(format.as_bytes());
        d.push(0);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        d.extend_from_slice(mip);
        for v in [4u32, 4, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0; 12]);
        d
    }

    #[test]
    fn finds_the_mip() {
        let d = payload("PF_DXT1", &[0xAA; 8]);
        let t = parse_texture(&d).unwrap();
        assert_eq!((t.format, t.width, t.height), (Some(Format::Bc1), 4, 4));
        assert_eq!(t.mips, vec![Mip { width: 4, height: 4, offset: 66, len: 8 }]);
        assert_eq!(&d[66..74], &[0xAA; 8]);
    }

    #[test]
    fn rejects_a_wrong_trailer_and_keeps_unknown_formats_mipless() {
        let mut d = payload("PF_DXT1", &[0; 8]);
        d[74] = 5;
        assert!(parse_texture(&d).unwrap_err().contains("trailer"));
        let t = parse_texture(&payload("PF_G8", &[0; 8])).unwrap();
        assert_eq!(t.format, None);
        assert!(t.mips.is_empty());
        assert_eq!(t.format_name, "PF_G8");
    }
}
