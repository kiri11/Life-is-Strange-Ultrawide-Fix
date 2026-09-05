//! Life is Strange: Reunion (Iris, UE 5.5.4, Denuvo).
//!
//! Two of Double Exposure's three changes, at the sites RESEARCH.md section
//! 13 documents: the two immediates in the projection function and cave A at
//! the constraint copy in `UCameraComponent::GetCameraView`. There is no
//! cave B: Reunion's cutscenes are cine cameras (`UD9CineCameraComponent`,
//! 13f), and re-asserting the constraint on the cine component's `Super`
//! call boxes every one of them (13h). The registers and offsets differ from
//! Double Exposure (13c, 13e); the executable's layout differs too (13a),
//! which is why the cave goes in the section that holds the sites and
//! nowhere else. The UI edits are RESEARCH.md 13i: the same fixed
//! 3840x2160 `WindowParent` as Double Exposure's, in a UE 5.5 container.

use crate::camera::cave_a_reunion;
use crate::plan::{Plan, Site, Write, locate, rel32};
use crate::scan::{Image, find_cave};
use crate::ui_layout::{Edit, Field, NewValue, UiFix};
use crate::zen::Summary;

use super::Game;

pub struct Reunion;

pub static REUNION: Reunion = Reunion;

const AXIS: Site = Site {
    name: "Hor+ projection branch (CalculateProjectionMatrixGivenViewRectangle)",
    sig: "3B C1 7E 06 40 80 FE 02 74 ?? 40 80 FE 01 74 ??",
    expected: 0x3698BD4,
    patched: &["3B C1 7E 06 40 80 FE FF 74 ?? 40 80 FE FF 74 ??"],
};

const GATE: Site = Site {
    name: "GetCameraView flag copy (cave A site)",
    sig: "0F B6 8B 59 02 00 00 33 4F 68 83 E1 01",
    expected: 0x36A687D,
    patched: &["E8 ?? ?? ?? ?? 66 90 33 4F 68 83 E1 01"],
};

/// RESEARCH.md section 13: the branch edit and cave A.
pub fn plan_reunion(img: &Image, upper: [u8; 4]) -> Result<Plan, String> {
    let mut notes = Vec::new();
    let mut writes = Vec::new();

    // 13d: both "cmp sil, <enum>" immediates become 0xFF.
    let axis = locate(img, &AXIS, &mut notes)?;
    writes.push(Write {
        va: axis + 7,
        expected: vec![0x02],
        bytes: vec![0xFF],
        what: "MajorAxisFOV compare disabled".into(),
    });
    writes.push(Write {
        va: axis + 13,
        expected: vec![0x01],
        bytes: vec![0xFF],
        what: "MaintainXFOV compare disabled".into(),
    });

    // 13e: the 7-byte movzx becomes "call caveA ; nop2".
    let gate = locate(img, &GATE, &mut notes)?;
    let blob_a = cave_a_reunion(upper);
    let a = find_cave(img, gate, blob_a.len() + 8, &[])
        .ok_or("no int3 padding run large enough for cave A in the code section")?;
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

    notes.push(format!("cave A at rva {a:#x}; no cave B for this game (RESEARCH 13e)"));
    Ok(Plan { writes, notes })
}

macro_rules! edit {
    ($pkg:expr, $widget:expr, $field:ident, $old:expr, $new:expr) => {
        Edit { package: $pkg, widget: $widget, field: Field::$field, old: $old, new: $new }
    };
}

/// RESEARCH.md 13i: the slot edits, the same three kinds as Double
/// Exposure's (9c-3). `WindowParent` is the fix itself; the rest repair
/// elements positioned by absolute coordinates on the 3840 canvas, which
/// would otherwise shift left. Package paths are below `UiFix::ui_prefix`.
static REUNION_EDITS: &[Edit] = &[
    // --- the fix itself
    edit!("BP/BP_IrisUIWindowManager.uasset", "WindowParent", Right, 3840.0, NewValue::Width),
    // --- centred by hardcoding half of 3840
    edit!("BP/Window/BP_PauseWindow.uasset", "Pause", Left, 1920.0, NewValue::HalfWidth),
    // --- fixed 3840-wide, not centred; must span the widened parent
    edit!("BP/Window/BP_SettingsWindow.uasset", "Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SaveSelectWindow.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SquareEnixAccountWindow.uasset", "CanvasPanel_Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_SquareEnixAccountWindow.uasset", "WidgetSwitcher_CurrentView", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_OutfitWindow.uasset", "Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_MontageWindow.uasset", "Background", Right, 3840.0, NewValue::Width),
    edit!("BP/Window/BP_FRPosterWindow.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/Settings/BP_UISettings.uasset", "Buttons", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/Settings/BP_OutfitSettings.uasset", "Buttons", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_ChloePhotoPosterUI.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_IrisPhotoPosterUI.uasset", "D9Image", Right, 3840.0, NewValue::Width),
    // --- the scroll buttons of the reading views, centred by an absolute
    //     X on the 3840 canvas (1870 + half their 100 px width = 1920)
    edit!("BP/Window/BP_FRPosterWindow.uasset", "UpButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Window/BP_FRPosterWindow.uasset", "DownButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Window/BP_ObjectInspectWindow.uasset", "UpButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Window/BP_ObjectInspectWindow.uasset", "DownButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_ChloePhotoPosterUI.uasset", "UpButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_ChloePhotoPosterUI.uasset", "DownButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_IrisPhotoPosterUI.uasset", "UpButton", Left, 1870.0, NewValue::Inset(1870.0)),
    edit!("BP/Controls/PlayerMenu/Collectibles/BP_IrisPhotoPosterUI.uasset", "DownButton", Left, 1870.0, NewValue::Inset(1870.0)),
    // --- full-bleed 16:9 compositions: re-inset so they keep their authored
    //     framing instead of riding out to the physical screen edges
    edit!("BP/Window/BP_MainMenuWindow.uasset", "MainButtons", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "D9Image", Left, 184.0, NewValue::Inset(184.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "D9TextBlock", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "GamerTag", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_MainMenuWindow.uasset", "InfocastPanel", Left, -220.0, NewValue::Outset(-220.0)),
    edit!("BP/Window/BP_TitleWindow.uasset", "GamerTag", Left, 220.0, NewValue::Inset(220.0)),
    edit!("BP/Window/BP_TitleWindow.uasset", "PressAnyKey", Left, 220.0, NewValue::Inset(220.0)),
];

static REUNION_UI: UiFix = UiFix {
    source: "pakchunk0-Windows",
    content_prefix: "Iris/Content/",
    ui_prefix: "Iris/Content/UI/",
    mount_point: "../../../Iris/Content/",
    mod_name: "LiSUltrawideUI_P",
    design: (3840.0, 2160.0),
    edits: REUNION_EDITS,
    toc_version: 8,
    container_header_version: 4,
    summary: Summary::Ue53,
};

impl Game for Reunion {
    fn id(&self) -> &'static str {
        "reunion"
    }
    fn title(&self) -> &'static str {
        "Life is Strange: Reunion"
    }
    fn short_title(&self) -> &'static str {
        "Reunion"
    }
    fn steam_appid(&self) -> u32 {
        2624870
    }
    fn exe_name(&self) -> &'static str {
        "Iris-Win64-Shipping.exe"
    }
    fn project(&self) -> &'static str {
        "Iris"
    }
    fn install_dir(&self) -> &'static str {
        "LifeisStrangeReunion"
    }
    fn folder_hint(&self) -> &'static str {
        "reunion"
    }
    fn plan_camera(&self, image: &Image, gate_upper: [u8; 4]) -> Result<Plan, String> {
        plan_reunion(image, gate_upper)
    }
    fn ui(&self) -> Option<&'static UiFix> {
        Some(&REUNION_UI)
    }
    fn ini_markers(&self) -> (&'static str, &'static str) {
        (
            "; ===== BEGIN LiS:Reunion Ultrawide Fix (managed block - safe to delete) =====",
            "; ===== END LiS:Reunion Ultrawide Fix =====",
        )
    }
}
