//! The camera patch's building blocks shared by planners: the gate bound
//! for a display and the two code caves (RESEARCH.md section 2), exactly
//! what the Python installer used to write.

use crate::hex;

/// Cave A's upper bound for a display: its aspect plus a little slack so the
/// letterbox ramp's endpoint stays inside the gate (RESEARCH 2c). The same
/// arithmetic the Python installer used, so the bytes match the reference
/// patch exactly.
pub fn gate_upper(width: u32, height: u32) -> [u8; 4] {
    let ratio = width as f64 / height as f64;
    aspect_bytes((ratio.max(1.8) * 1.002 * 10000.0).round() / 10000.0)
}

pub fn aspect_bytes(aspect: f64) -> [u8; 4] {
    (aspect as f32).to_le_bytes()
}

/// 1.7777778f, the aspect the game's cameras are authored for.
const AUTHORED_ASPECT: &str = "398EE33F";

pub fn cave_a(upper: [u8; 4]) -> Vec<u8> {
    let mut v = hex("0FB683B4020000 8B8BB0020000 81F90000E03F 7612 81F9");
    v.extend_from_slice(&upper);
    v.extend(hex("730A 83E0FE C74748"));
    v.extend(hex(AUTHORED_ASPECT));
    v.push(0xC3);
    v
}

/// Cave A for Reunion (RESEARCH 13e): the flags byte travels in `ecx`, the
/// component aspect is compared in `eax`, the view's aspect is at +0x5C.
pub fn cave_a_reunion(upper: [u8; 4]) -> Vec<u8> {
    let mut v = hex("0FB68B59020000 8B8354020000 3D0000E03F 7611 3D");
    v.extend_from_slice(&upper);
    v.extend(hex("730A 83E1FE C7475C"));
    v.extend(hex(AUTHORED_ASPECT));
    v.push(0xC3);
    v
}

pub fn cave_b(rel_to_super: i32) -> Vec<u8> {
    let mut v = hex("4883EC28 E8");
    v.extend_from_slice(&rel_to_super.to_le_bytes());
    v.extend(hex("4883C428 804F4C01 C3"));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex4;

    #[test]
    fn gate_bound_matches_the_python_installer() {
        // 5120x2160 -> 2.3751, the value in the reference patch
        assert_eq!(gate_upper(5120, 2160), hex4("A3011840"));
        // narrower than 1.8 is treated as 1.8, the old default
        assert_eq!(gate_upper(1920, 1080), aspect_bytes(1.8036));
        assert_eq!(gate_upper(3440, 1440), aspect_bytes(2.3937));
    }

    #[test]
    fn cave_bytes_are_the_documented_ones() {
        assert_eq!(
            cave_a(hex4("A3011840")),
            hex("0FB683B4020000 8B8BB0020000 81F90000E03F 7612 81F9 A3011840 730A 83E0FE C74748 398EE33F C3")
        );
        assert_eq!(cave_b(0x03D6785C), hex("4883EC28 E85C78D603 4883C428 804F4C01 C3"));
        assert_eq!(
            cave_a_reunion(hex4("A3011840")),
            hex("0FB68B59020000 8B8354020000 3D0000E03F 7611 3D A3011840 730A 83E1FE C7475C 398EE33F C3")
        );
        // the jumps land on the ret
        let a = cave_a_reunion(hex4("A3011840"));
        assert_eq!(a.len(), 38);
        assert_eq!(a[20 + a[19] as usize], 0xC3); // jbe at 18
        assert_eq!(a[27 + a[26] as usize], 0xC3); // jae at 25
    }
}
