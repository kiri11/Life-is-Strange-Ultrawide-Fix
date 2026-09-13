use crate::pak_ui::{FloatEdit, PackageEdit, PakUiFix};
use crate::plan::{Plan, Site, Write, locate, rel32};
use crate::scan::{Image, find_cave};
use crate::ui_layout::UiFix;

use super::Game;

pub struct TrueColors;

pub static TRUE_COLORS: TrueColors = TrueColors;

const AXIS: Site = Site {
    name: "Hor+ projection branch (CalculateProjectionMatrixGivenViewRectangle)",
    sig: "3B C1 7E 05 80 FA 02 74 ?? 80 FA 01 74 ?? 0F B6 53 34 80 FA 01",
    expected: 0x23EFC3F,
    patched: &["3B C1 7E 05 80 FA FF 74 ?? 80 FA FF 74 ?? 0F B6 53 34 80 FA 01"],
};

const GATE: Site = Site {
    name: "GetCameraView flag copy (cave A site)",
    sig: "0F B6 83 04 02 00 00 33 47 30 83 E0 01",
    expected: 0x23FB3FC,
    patched: &["E8 ?? ?? ?? ?? 66 90 33 47 30 83 E0 01"],
};

pub fn plan_true_colors(img: &Image, upper: [u8; 4]) -> Result<Plan, String> {
    let mut notes = Vec::new();
    let mut writes = Vec::new();

    let axis = locate(img, &AXIS, &mut notes)?;
    writes.push(Write { va: axis + 6, expected: vec![0x02], bytes: vec![0xFF], what: "MajorAxisFOV compare disabled".into() });
    writes.push(Write { va: axis + 11, expected: vec![0x01], bytes: vec![0xFF], what: "MaintainXFOV compare disabled".into() });

    let gate = locate(img, &GATE, &mut notes)?;
    // UE4's linker leaves at most 21 int3 bytes between functions here.
    let mut caves = Vec::new();
    for size in [19, 17, 17, 11] {
        let va = find_cave(img, gate, size, &caves).ok_or("no int3 padding run large enough for the camera gate")?;
        caves.push((va, size));
    }
    let [a, b, c, d] = std::array::from_fn::<_, 4, _>(|i| caves[i].0);
    let jump = |bytes: &mut Vec<u8>, opcode: &[u8], from: u64, to: u64| -> Result<(), String> {
        bytes.extend_from_slice(opcode);
        bytes.extend_from_slice(&rel32(to, from + bytes.len() as u64 + 4)?.to_le_bytes());
        Ok(())
    };
    let mut load = crate::hex("0FB68304020000 8B8B00020000");
    jump(&mut load, &[0xE9], a, b)?;
    load.push(0xC3);
    let mut lower = crate::hex("81F90000E03F");
    jump(&mut lower, &[0x0F, 0x86], b, a + 18)?;
    jump(&mut lower, &[0xE9], b, c)?;
    let mut upper_check = crate::hex("81F9");
    upper_check.extend_from_slice(&upper);
    jump(&mut upper_check, &[0x0F, 0x83], c, a + 18)?;
    jump(&mut upper_check, &[0xE9], c, d)?;
    let apply = crate::hex("83E0FE C7472C398EE33F C3");
    for ((va, size), bytes) in caves.into_iter().zip([load, lower, upper_check, apply]) {
        debug_assert_eq!(size, bytes.len());
        writes.push(Write { va, expected: vec![0xCC; size], bytes, what: "aspect-gated camera helper".into() });
        notes.push(format!("camera helper at rva {va:#x}"));
    }
    let mut call = Vec::new();
    jump(&mut call, &[0xE8], gate, a)?;
    call.extend([0x66, 0x90]);
    writes.push(Write {
        va: gate,
        expected: img.read(gate, 7).ok_or("camera gate is not readable")?.to_vec(),
        bytes: call,
        what: "GetCameraView flag copy -> aspect-gated camera helper".into(),
    });

    Ok(Plan { writes, notes })
}

impl Game for TrueColors {
    fn id(&self) -> &'static str {
        "true-colors"
    }
    fn title(&self) -> &'static str {
        "Life is Strange: True Colors"
    }
    fn short_title(&self) -> &'static str {
        "True Colors"
    }
    fn steam_appid(&self) -> u32 {
        936790
    }
    fn exe_name(&self) -> &'static str {
        "Siren-Win64-Shipping.exe"
    }
    fn project(&self) -> &'static str {
        "Siren"
    }
    fn install_dir(&self) -> &'static str {
        "LifeIsStrange3"
    }
    fn folder_hint(&self) -> &'static str {
        "truecolors"
    }
    fn config_dir(&self) -> &'static str {
        "WindowsNoEditor"
    }
    fn plan_camera(&self, image: &Image, gate_upper: [u8; 4]) -> Result<Plan, String> {
        plan_true_colors(image, gate_upper)
    }
    fn pak_ui(&self) -> Option<&'static PakUiFix> {
        Some(&TRUE_COLORS_UI)
    }
    fn ui(&self) -> Option<&'static UiFix> {
        None
    }
    fn ini_markers(&self) -> (&'static str, &'static str) {
        (
            "; ===== BEGIN LiS:TrueColors Ultrawide Fix (managed block - safe to delete) =====",
            "; ===== END LiS:TrueColors Ultrawide Fix =====",
        )
    }
}

static TRUE_COLORS_UI: PakUiFix = PakUiFix {
    source: "pakchunk0-WindowsNoEditor.pak",
    mod_name: "LiSUltrawideUI_P",
    design: (1920.0, 1080.0),
    packages: &[
        PackageEdit {
            path: "Siren/Content/UI/BP/Managers/UIWindowManager_BP",
            header_sha256: "ed79087279d4da7716701dd366c327c858cc26b826d40f1f68e2638c60eb85a0",
            payload_sha256: "fd0344adca2e7f117047f6b9372220aca1c0dea4bc3ef020e81045a2cadf76ca",
            edits: &[
                FloatEdit { label: "WidthOverride", offsets: &[15247, 15439], old: 1920.0, grow: (1.0, 0.0) },
                FloatEdit { label: "HeightOverride", offsets: &[15276, 15468], old: 1080.0, grow: (0.0, 1.0) },
            ],
        },
        PackageEdit {
            path: "Siren/Content/UI/BP/Window/ChoicesWindow_BP",
            header_sha256: "5e9d11663d36a55bd31ed1b893c2560046a82b86f14b92856cc7d0a52f417898",
            payload_sha256: "1678fd365e2772dc445d2bc9ef10e95d06759c25a11511aae67b51cc50c5f20f",
            edits: &[FloatEdit { label: "D9Image_102.Right", offsets: &[27307, 255544], old: 1920.0, grow: (1.0, 0.0) }],
        },
        PackageEdit {
            path: "Siren/Content/UI/BP/Window/MainMenuWindow_BP",
            header_sha256: "984b26855c561cf19e0f63357f7c4e60a654a9f49771fb799ee8527acd2eca70",
            payload_sha256: "0e173ed1aded8742356eeee39bd5c7fc3a7a85de8cf7a45df146ba0129b9920a",
            edits: &[
                FloatEdit { label: "Logo.Left", offsets: &[20243, 33726], old: 442.0, grow: (0.5, 0.0) },
                FloatEdit { label: "ScaleParent.Left", offsets: &[20600, 34083], old: 442.0, grow: (0.5, 0.0) },
            ],
        },
        PackageEdit {
            path: "Siren/Content/UI/BP/Window/TitleWindow_BP",
            header_sha256: "a0c24bdf3ec3fc40b91c9c0d0782cae9f5d1b25cff79870757169a2b0b48ca75",
            payload_sha256: "256db588f38c6616f592ad6d99cbac7ac7d8b5868095e5f55d935a2c45754997",
            edits: &[FloatEdit { label: "Logo.Left", offsets: &[15223, 48313], old: 186.0, grow: (0.5, 0.0) }],
        },
    ],
};
