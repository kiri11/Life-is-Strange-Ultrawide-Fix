//! Life is Strange: Double Exposure (Chronos, UE 5.2.1).
//!
//! The camera patch is RESEARCH.md sections 1 and 2, byte for byte: the
//! branch edit and cave A. The cine component's `Super::GetCameraView`
//! call is left alone (2d): the game's only cine cameras are the main-menu
//! and outfit-screen ones, which cave A widens like any other.
//! The UI edits are section 9c-3.

use crate::camera::cave_a;
use crate::plan::{Plan, Site, Write, locate, rel32};
use crate::scan::{Image, find_cave};
use crate::ui_layout::{Edit, Field, NewValue, UiFix};
use crate::zen::Summary;

use super::Game;

pub struct DoubleExposure;

pub static DOUBLE_EXPOSURE: DoubleExposure = DoubleExposure;

const AXIS: Site = Site {
    name: "Hor+ projection branch (CalculateProjectionMatrixGivenViewRectangle)",
    sig: "3B C1 7E 09 80 FA 02 0F 84 ?? ?? ?? ?? 80 FA 01 0F 84 ?? ?? ?? ??",
    expected: 0x440B5C0,
    patched: &["3B C1 7E 09 80 FA FF 0F 84 ?? ?? ?? ?? 80 FA FF 0F 84 ?? ?? ?? ??"],
};

const GATE: Site = Site {
    name: "GetCameraView flag copy (cave A site)",
    sig: "0F B6 83 B4 02 00 00 33 47 4C 83 E0 01",
    expected: 0x441AB4C,
    patched: &[
        "E8 ?? ?? ?? ?? 66 90 33 47 4C 83 E0 01",
        "31 C0 0F 1F 44 00 00 33 47 4C 83 E0 01",
    ],
};

/// RESEARCH.md section 1: the branch edit and cave A.
pub fn plan_double_exposure(img: &Image, upper: [u8; 4]) -> Result<Plan, String> {
    let mut notes = Vec::new();
    let mut writes = Vec::new();

    // 2b: both "cmp dl, <enum>" immediates become 0xFF, which no real enum
    // value equals, so every perspective camera takes the Hor+ path.
    let axis = locate(img, &AXIS, &mut notes)?;
    writes.push(Write {
        va: axis + 6,
        expected: vec![0x02],
        bytes: vec![0xFF],
        what: "MajorAxisFOV compare disabled".into(),
    });
    writes.push(Write {
        va: axis + 15,
        expected: vec![0x01],
        bytes: vec![0xFF],
        what: "MaintainXFOV compare disabled".into(),
    });

    // 2c: the 7-byte movzx becomes "call caveA ; nop2", and cave A gates the
    // constraint on the camera's authored aspect and pins the FOV divisor.
    let gate = locate(img, &GATE, &mut notes)?;
    let blob_a = cave_a(upper);
    let a = find_cave(img, gate, blob_a.len() + 8, &[])
        .ok_or("no int3 padding run large enough for cave A")?;
    let mut site_a = vec![0xE8];
    site_a.extend_from_slice(&rel32(a, gate + 5)?.to_le_bytes());
    site_a.extend([0x66, 0x90]);
    writes.push(Write {
        va: a,
        expected: vec![0xCC; blob_a.len()],
        bytes: blob_a,
        what: format!("cave A: aspect-gated unconstrain, upper bound {:.4}", f32::from_le_bytes(upper)),
    });
    writes.push(Write {
        va: gate,
        expected: img.read(gate, 7).ok_or("cave A site is not readable")?.to_vec(),
        bytes: site_a,
        what: "GetCameraView flag copy -> call cave A".into(),
    });

    notes.push(format!("cave A at rva {a:#x}"));
    Ok(Plan { writes, notes })
}

macro_rules! edit {
    ($pkg:expr, $widget:expr, $field:ident, $old:expr, $new:expr) => {
        Edit { package: $pkg, widget: $widget, field: Field::$field, old: $old, new: $new }
    };
}

/// RESEARCH.md 9c-3: the 15 slot edits. `WindowParent` is the fix itself;
/// the rest repair elements positioned by absolute coordinates on the 3840
/// canvas, which would otherwise shift left. Every edit rewrites an existing
/// float in place, so package sizes never change. Package paths are below
/// `UiFix::ui_prefix`.
static DE_EDITS: &[Edit] = &[
    // --- the fix itself
    edit!("BP/BP_UIWindowManager.uasset", "WindowParent", Right, 3840.0, NewValue::Width),
    // --- HIGH: centred by hardcoding half of 3840
    edit!("BP/Window/BP_PauseWindow.uasset", "Pause", Left, 1920.0, NewValue::HalfWidth),
    // --- FIXW: fixed 3840-wide, not centred; must span the widened parent
    edit!("BP/Window/BP_SettingsWindow.uasset", "Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SaveSelectWindow.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SquareEnixAccountWindow.uasset", "CanvasPanel_Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SquareEnixAccountWindow.uasset", "WidgetSwitcher_CurrentView", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/Settings/BP_UISettings.uasset", "Buttons", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/Choices/BP_ShiftChoiceUI.uasset", "ChoiceButton", Right, 3840.0, NewValue::Width),
    // --- full-bleed 16:9 compositions: re-inset so they keep their authored
    //     framing instead of riding out to the physical screen edges.
    //     Inset shifts a left-anchored element right by (designW-3840)/2;
    //     Outset shifts a right-anchored one left by the same amount.
    edit!("BP/Window/BP_MainMenuWindow.uasset", "MainButtons", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "D9Image", Left, 184.0, NewValue::Inset(184.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "GamerTag", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "InfocastPanel", Left, -220.0, NewValue::Outset(-220.0)),
    edit!("BP/Window/BP_TitleWindow.uasset", "GamerTag", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_TitleWindow.uasset", "PressAnyKey", Left, 220.0, NewValue::Inset(220.0)),
];

static DE_UI: UiFix = UiFix {
    source: "pakchunk0-Windows",
    content_prefix: "Chronos/Content/",
    ui_prefix: "Chronos/Content/UI/",
    mount_point: "../../../Chronos/Content/",
    mod_name: "LiSUltrawideUI_P",
    design: (3840.0, 2160.0),
    edits: DE_EDITS,
    reslots: &[],
    toc_version: 5,
    container_header_version: 2,
    summary: Summary::Ue52,
};

impl Game for DoubleExposure {
    fn id(&self) -> &'static str {
        "double-exposure"
    }
    fn title(&self) -> &'static str {
        "Life is Strange: Double Exposure"
    }
    fn short_title(&self) -> &'static str {
        "Double Exposure"
    }
    fn steam_appid(&self) -> u32 {
        1874000
    }
    fn exe_name(&self) -> &'static str {
        "Chronos-Win64-Shipping.exe"
    }
    fn project(&self) -> &'static str {
        "Chronos"
    }
    fn install_dir(&self) -> &'static str {
        "LifeIsStrangeDoubleExposure"
    }
    fn folder_hint(&self) -> &'static str {
        "doubleexposure"
    }
    fn plan_camera(&self, image: &Image, gate_upper: [u8; 4]) -> Result<Plan, String> {
        plan_double_exposure(image, gate_upper)
    }
    fn ui(&self) -> Option<&'static UiFix> {
        Some(&DE_UI)
    }
    fn ini_markers(&self) -> (&'static str, &'static str) {
        (
            "; ===== BEGIN LiS:DE Ultrawide Fix (managed block - safe to delete) =====",
            "; ===== END LiS:DE Ultrawide Fix =====",
        )
    }
}
