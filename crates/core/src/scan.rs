//! Signature search and code-cave placement over an executable image.
//!
//! Pure functions over byte slices, so the same code runs inside the game
//! (over the mapped image) and in the tests (over the file on disk).

/// One section of the executable: where it is mapped and its bytes.
pub struct Section<'a> {
    pub va: u64,
    pub data: &'a [u8],
}

/// The executable sections of an image, addressed by RVA.
pub struct Image<'a> {
    pub sections: Vec<Section<'a>>,
}

impl<'a> Image<'a> {
    /// The `len` bytes at `va`, if they lie inside one section.
    pub fn read(&self, va: u64, len: usize) -> Option<&'a [u8]> {
        self.sections.iter().find_map(|s| {
            let end = s.va + s.data.len() as u64;
            if va >= s.va && va.checked_add(len as u64)? <= end {
                let off = (va - s.va) as usize;
                Some(&s.data[off..off + len])
            } else {
                None
            }
        })
    }
}

/// A byte pattern with `??` wildcards, written the way RESEARCH.md lists them.
pub struct Sig {
    pat: Vec<u8>,
    mask: Vec<bool>,
    anchor: usize,
}

impl Sig {
    pub fn parse(text: &str) -> Sig {
        let mut pat = Vec::new();
        let mut mask = Vec::new();
        for tok in text.split_whitespace() {
            if tok == "??" {
                pat.push(0);
                mask.push(false);
            } else {
                pat.push(u8::from_str_radix(tok, 16).expect("hex byte in signature"));
                mask.push(true);
            }
        }
        let anchor = mask.iter().position(|&m| m).expect("a signature needs one literal byte");
        Sig { pat, mask, anchor }
    }

    pub fn len(&self) -> usize {
        self.pat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pat.is_empty()
    }

    pub fn matches(&self, window: &[u8]) -> bool {
        window.len() >= self.pat.len()
            && self
                .pat
                .iter()
                .zip(&self.mask)
                .zip(window)
                .all(|((p, m), b)| !m || p == b)
    }

    /// Every match in the image in address order, at most `limit` of them.
    pub fn find_all(&self, img: &Image, limit: usize) -> Vec<u64> {
        let mut hits = Vec::new();
        let first = self.pat[self.anchor];
        for s in &img.sections {
            let data = s.data;
            if data.len() < self.pat.len() {
                continue;
            }
            let last_anchor = data.len() - self.pat.len() + self.anchor;
            let mut i = self.anchor;
            while i <= last_anchor {
                let Some(p) = data[i..=last_anchor].iter().position(|&b| b == first) else {
                    break;
                };
                let start = i + p - self.anchor;
                if self.matches(&data[start..start + self.pat.len()]) {
                    hits.push(s.va + start as u64);
                    if hits.len() >= limit {
                        return hits;
                    }
                }
                i += p + 1;
            }
        }
        hits
    }
}

/// The first run of at least `need` int3 (0xCC) padding bytes that begins
/// right after a byte that is not padding, so the cave starts on an
/// instruction boundary, in the section that holds `near` (the site the
/// cave serves) and in no other: Reunion's executable carries a second
/// executable section that is Denuvo's, not the engine's. Bytes inside
/// `taken` count as used, which is how a second cave stays clear of the
/// first before either is written.
pub fn find_cave(img: &Image, near: u64, need: usize, taken: &[(u64, usize)]) -> Option<u64> {
    for s in &img.sections {
        if near < s.va || near >= s.va + s.data.len() as u64 {
            continue;
        }
        let data = s.data;
        let free = |i: usize| {
            data[i] == 0xCC && {
                let a = s.va + i as u64;
                !taken.iter().any(|&(va, len)| a >= va && a < va + len as u64)
            }
        };
        let mut i = 0;
        while i < data.len() {
            if !free(i) {
                i += 1;
                continue;
            }
            let start = i;
            while i < data.len() && free(i) {
                i += 1;
            }
            if i - start >= need {
                return Some(s.va + start as u64);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_and_uniqueness() {
        let data = [0u8, 0x0F, 0xB6, 0x83, 7, 0x0F, 0xB6, 0x84, 9];
        let img = Image { sections: vec![Section { va: 0x1000, data: &data }] };
        let sig = Sig::parse("0F B6 ?? 07");
        assert_eq!(sig.find_all(&img, 4), vec![0x1001]);
        assert_eq!(Sig::parse("0F B6").find_all(&img, 4), vec![0x1001, 0x1005]);
        assert_eq!(Sig::parse("0F B6").find_all(&img, 1), vec![0x1001]);
        assert!(img.read(0x1008, 2).is_none());
        assert_eq!(img.read(0x1008, 1), Some(&[9u8][..]));
    }

    #[test]
    fn caves_start_after_code_and_skip_taken_bytes() {
        let mut data = vec![0x90u8; 64];
        data[10..30].fill(0xCC); // 20 bytes, preceded by code
        data[40..64].fill(0xCC); // 24 bytes to the end
        let other = vec![0xCCu8; 64];
        let img = Image {
            sections: vec![Section { va: 0x1000, data: &data }, Section { va: 0x2000, data: &other }],
        };
        assert_eq!(find_cave(&img, 0x1005, 16, &[]), Some(0x100A));
        assert_eq!(find_cave(&img, 0x1005, 21, &[]), Some(0x1028));
        // the second section has room, but it is not the site's section
        assert_eq!(find_cave(&img, 0x1005, 25, &[]), None);
        assert_eq!(find_cave(&img, 0x2005, 25, &[]), Some(0x2000)); // its own section
        // the first cave used 12 of the 20 bytes: the remaining 8 start on
        // the boundary after it
        assert_eq!(find_cave(&img, 0x1005, 8, &[(0x100A, 12)]), Some(0x1016));
        assert_eq!(find_cave(&img, 0x1005, 9, &[(0x100A, 12)]), Some(0x1028));
    }
}
