//! The container reader, the Zen parser and the UI-fix writer against the
//! game's own data, when this machine has it. The expected values were
//! captured from the Python implementation this replaced, on the build of
//! the game installed on 2026-09-02; a game update changes them.
//!
//! `LIS_DE_PAKS` and `LIS_REUNION_PAKS` name the games' `Content/Paks`
//! folders; otherwise the developer machine's paths are tried, and the
//! tests skip if they are absent.
//! `LIS_DE_UI_REFERENCE` names a folder holding a `LiSUltrawideUI_P.utoc`,
//! `.ucas` and `.pak` the Python writer produced for 5120x2160; when set,
//! the Rust writer's output must equal it exactly.

use std::path::PathBuf;

use lis_ultrawide_core::games::Game;
use lis_ultrawide_core::games::double_exposure::DOUBLE_EXPOSURE;
use lis_ultrawide_core::games::reunion::REUNION;
use std::collections::BTreeMap;

use lis_ultrawide_core::iostore::{
    CHUNK_CONTAINER_HEADER, StoreEntry, Toc, build_container_header, load_script_objects, lookup, package_id_of,
    parse_container_header,
};
use lis_ultrawide_core::ui_layout::{build_mod, design_space, slot_payload};
use lis_ultrawide_core::unver::Slot;
use lis_ultrawide_core::zen::{ScriptObjects, Summary, ZenPackage};
use lis_ultrawide_core::{hash, to_hex};

fn game_paks(var: &str, default: &str) -> Option<PathBuf> {
    let p = std::env::var(var).map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(default));
    if p.join("pakchunk0-Windows.utoc").is_file() {
        Some(p)
    } else {
        eprintln!("skipped: no game data at {} (set {var})", p.display());
        None
    }
}

fn paks() -> Option<PathBuf> {
    game_paks("LIS_DE_PAKS", r"D:\SteamLibrary\steamapps\common\LifeIsStrangeDoubleExposure\Chronos\Content\Paks")
}

fn reunion_paks() -> Option<PathBuf> {
    game_paks("LIS_REUNION_PAKS", r"D:\SteamLibrary\steamapps\common\LifeisStrangeReunion\Iris\Content\Paks")
}

struct SlotRef {
    widget: &'static str,
    export: usize,
    base: usize,
    payload_len: usize,
    float_at: usize,
    offsets: [f32; 4],
    anchor_min: (f64, f64),
    anchor_max: (f64, f64),
    align: (f64, f64),
    content: i32,
    parent: i32,
}

struct PkgRef {
    path: &'static str,
    index: usize,
    chunk: &'static str,
    size: usize,
    sha256: &'static str,
    exports: usize,
    name: &'static str,
    entry: (i32, i32, usize),
    slots: &'static [SlotRef],
}

macro_rules! slot {
    ($w:expr, $e:expr, $b:expr, $l:expr, $f:expr, $o:expr, $amin:expr, $amax:expr, $al:expr, $c:expr, $p:expr) => {
        SlotRef { widget: $w, export: $e, base: $b, payload_len: $l, float_at: $f, offsets: $o, anchor_min: $amin, anchor_max: $amax, align: $al, content: $c, parent: $p }
    };
}

const PACKAGES: &[PkgRef] = &[
    PkgRef { path: "Chronos/Content/UI/BP/BP_UIWindowManager.uasset", index: 23076, chunk: "61acfe385af60f7500000001", size: 13615, sha256: "13a6a0e2e10354d3855cd5dd21238336d191609f001baca8c38377c37f100a1c", exports: 26, name: "/Game/UI/BP/BP_UIWindowManager", entry: (26, 1, 15),
        slots: &[slot!("WindowParent", 8, 13010, 78, 8, [0.0, 0.0, 3840.0, 2160.0], (0.5, 0.5), (0.5, 0.5), (0.5, 0.5), 16, 4)] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_PauseWindow.uasset", index: 11310, chunk: "44228dddea71f6a700000001", size: 10069, sha256: "13b39827e909063f1e0746a3811327d1d9ab002b142c7578bfff2eecba52e812", exports: 43, name: "/Game/UI/BP/Window/BP_PauseWindow", entry: (43, 1, 6),
        slots: &[slot!("Pause", 3, 8642, 54, 10, [1920.0, 590.0, 300.0, 80.0], (0.0, 0.0), (0.0, 0.0), (0.5, 0.0), 25, 12)] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_SettingsWindow.uasset", index: 10892, chunk: "ee780ccda0ebf22300000001", size: 10235, sha256: "e8e9aad000f28e46e6522ce14a739b74643caf8300ba5026a64514e38fb777e4", exports: 60, name: "/Game/UI/BP/Window/BP_SettingsWindow", entry: (60, 1, 10),
        slots: &[slot!("Background", 19, 8839, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 38, 35)] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_SaveSelectWindow.uasset", index: 9470, chunk: "b84702f3604ed2ca00000001", size: 7668, sha256: "ea30fcf99a07d6fa63e90594ae5863dea5fd9d1abf9478a1add07e5f6ea807f2", exports: 42, name: "/Game/UI/BP/Window/BP_SaveSelectWindow", entry: (42, 1, 5),
        slots: &[slot!("D9Image", 15, 6266, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 27, 25)] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_SquareEnixAccountWindow.uasset", index: 2544, chunk: "d5be7f86b2db437f00000001", size: 38662, sha256: "8881cc62c7430f7ab617a7131228ddccef835f457ed1b201451e7c9c3434650d", exports: 226, name: "/Game/UI/BP/Window/BP_SquareEnixAccountWindow", entry: (226, 1, 10),
        slots: &[
            slot!("CanvasPanel_Background", 45, 32171, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 17, 49),
            slot!("WidgetSwitcher_CurrentView", 46, 32199, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 218, 49),
        ] },
    PkgRef { path: "Chronos/Content/UI/BP/Controls/Settings/BP_UISettings.uasset", index: 53705, chunk: "a6626d04d8585e2300000001", size: 12061, sha256: "432b06aa5de82ae3c2244391c76ba240766acf4518ea0137c148dbb37ec3b747", exports: 48, name: "/Game/UI/BP/Controls/Settings/BP_UISettings", entry: (48, 1, 7),
        slots: &[slot!("Buttons", 14, 7917, 29, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 16, 23)] },
    PkgRef { path: "Chronos/Content/UI/BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI.uasset", index: 33977, chunk: "d598d203f5b3cde700000001", size: 12815, sha256: "93d188a57e0acb93e9e08a48e5a98e4acb1f255515408913f711d31b3cef231f", exports: 82, name: "/Game/UI/BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI", entry: (82, 1, 7),
        slots: &[slot!("D9Image", 28, 11045, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 44, 41)] },
    PkgRef { path: "Chronos/Content/UI/BP/Controls/Choices/BP_ShiftChoiceUI.uasset", index: 26462, chunk: "bd984f3843bd43b700000001", size: 3243, sha256: "c1b34170133414b0d8a8c6623d66279035d384b503dc2f4455410c7587324413", exports: 16, name: "/Game/UI/BP/Controls/Choices/BP_ShiftChoiceUI", entry: (16, 1, 1),
        slots: &[slot!("ChoiceButton", 2, 2553, 79, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 5, 6)] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_MainMenuWindow.uasset", index: 54713, chunk: "26f89c0653e4127600000001", size: 8540, sha256: "6f6737cb0d36751ee0dbfeec8bf50e302347dec9c1be376004d55cc9b26b1ab1", exports: 50, name: "/Game/UI/BP/Window/BP_MainMenuWindow", entry: (50, 1, 6),
        slots: &[
            slot!("MainButtons", 7, 6890, 29, 8, [220.0, 760.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 18, 29),
            slot!("D9Image", 8, 6919, 29, 8, [184.0, 262.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 31, 29),
            slot!("GamerTag", 11, 7073, 84, 9, [220.0, -240.0, 628.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 2, 30),
            slot!("InfocastPanel", 9, 6948, 86, 8, [-220.0, -140.0, 2647.0, 322.0], (1.0, 1.0), (1.0, 1.0), (1.0, 1.0), 28, 30),
        ] },
    PkgRef { path: "Chronos/Content/UI/BP/Window/BP_TitleWindow.uasset", index: 7638, chunk: "5e84e8a96ee85bf200000001", size: 2978, sha256: "3b82981a7626786f85b09ee7ef5af38519b5cf634d59626e844853bc45cee2b4", exports: 12, name: "/Game/UI/BP/Window/BP_TitleWindow", entry: (12, 1, 4),
        slots: &[
            slot!("GamerTag", 5, 2580, 84, 9, [220.0, -100.0, 628.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 1, 9),
            slot!("PressAnyKey", 6, 2664, 80, 9, [220.0, 12.0, 0.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 10, 9),
        ] },
];

#[test]
fn reads_the_games_ui_packages_as_the_python_did() {
    let Some(paks) = paks() else { return };
    let started = std::time::Instant::now();
    let so = load_script_objects(&paks.join("global.utoc")).unwrap();
    assert_eq!(so.len(), 35005);
    assert_eq!(so.get(&5730563978521807506).map(String::as_str), Some("/Script/UMG.CanvasPanelSlot"));
    eprintln!("script objects in {:?}", started.elapsed());

    let mut toc = Toc::open(&paks.join("pakchunk0-Windows.utoc")).unwrap();
    assert_eq!(toc.entries(), 56411);
    assert_eq!(toc.blocks.len(), 675076);
    assert_eq!(toc.block_size, 65536);
    assert_eq!(toc.methods, vec!["None".to_string(), "Oodle".to_string()]);
    assert_eq!(toc.index.len(), 50983);
    assert_eq!(toc.seeds.len(), 28206);
    assert_eq!(toc.unhashed_count, 0);
    assert_eq!(toc.flags, 9);
    assert_eq!(toc.partitions, 1);
    eprintln!("pakchunk0 toc in {:?}", started.elapsed());

    // the perfect hash resolves every chunk the container holds
    for (slot, id) in toc.chunk_ids.iter().enumerate() {
        assert_eq!(lookup(&toc.chunk_ids, &toc.seeds, id), Some(slot), "chunk {slot} does not resolve");
    }

    // the container header rebuilds byte for byte from what was parsed
    let h = toc.find_type(CHUNK_CONTAINER_HEADER).unwrap();
    assert_eq!(h, 3286);
    let header = toc.read(h).unwrap();
    assert_eq!(header.len(), 2038248);
    assert_eq!(to_hex(&hash::sha256(&header)), "4ed57e795622b100524421159b103b48a3f703e453d1a17eeb00841c8fad84de");
    let (cid, entries) = parse_container_header(&header, 2).unwrap();
    assert_eq!(cid, 0x81e4dd3d9595e447);
    assert_eq!(entries.len(), 35054);
    assert_eq!(build_container_header(cid, &entries, 2), header);
    eprintln!("container header round-tripped in {:?}", started.elapsed());

    check_packages(&mut toc, &entries, &so, Summary::Ue52, PACKAGES);
    eprintln!("packages checked in {:?}", started.elapsed());
}

/// Every package reads at the recorded size and hash, parses to the
/// recorded exports, and every slot the edits touch decodes to the
/// recorded values with the float to edit where it was recorded.
fn check_packages(toc: &mut Toc, entries: &BTreeMap<u64, StoreEntry>, so: &ScriptObjects, summary: Summary, packages: &[PkgRef]) {
    for p in packages {
        let idx = toc.index[p.path];
        assert_eq!(idx, p.index, "{}", p.path);
        assert_eq!(to_hex(&toc.chunk_ids[idx]), p.chunk);
        let data = toc.read(idx).unwrap();
        assert_eq!(data.len(), p.size, "{}", p.path);
        assert_eq!(to_hex(&hash::sha256(&data)), p.sha256, "{}", p.path);
        let e = &entries[&package_id_of(&toc.chunk_ids[idx])];
        assert_eq!((e.exports, e.bundles, e.imports.len()), p.entry);
        assert!(e.shader_hashes.is_empty());
        let pkg = ZenPackage::parse(&data, summary).unwrap();
        assert_eq!(pkg.exports.len(), p.exports);
        assert_eq!(pkg.name, p.name);
        for s in p.slots {
            let (export, slot, base, payload) = slot_payload(&pkg, s.widget, so).unwrap_or_else(|| panic!("{}: {}", p.path, s.widget));
            assert_eq!(export, s.export, "{}", s.widget);
            assert_eq!(base, s.base, "{}", s.widget);
            assert_eq!(payload.len(), s.payload_len, "{}", s.widget);
            assert_eq!(slot.offsets, s.offsets, "{}", s.widget);
            assert_eq!(slot.anchor_min, s.anchor_min);
            assert_eq!(slot.anchor_max, s.anchor_max);
            assert_eq!(slot.alignment, s.align);
            assert_eq!(slot.content, s.content);
            assert_eq!(slot.parent, s.parent);
            let old = s.offsets[if s.widget == "WindowParent" || s.offsets[2] == 3840.0 { 2 } else { 0 }];
            let at = (0..payload.len() - 3).find(|&o| f32::from_le_bytes(payload[o..o + 4].try_into().unwrap()) == old).unwrap();
            assert_eq!(at, s.float_at, "{}", s.widget);
        }
    }
}

const REUNION_PACKAGES: &[PkgRef] = &[
    PkgRef { path: "Iris/Content/UI/BP/BP_IrisUIWindowManager.uasset", index: 23182, chunk: "7c7c9cd6d0bac7ed00000001", size: 13028, sha256: "eaff8169e28ebf3a2f37f614a81ccab6f1a45f33207fe2da544e68ca573d0bdd", exports: 26, name: "/Game/UI/BP/BP_IrisUIWindowManager", entry: (0, 0, 4),
        slots: &[slot!("WindowParent", 8, 11699, 78, 8, [0.0, 0.0, 3840.0, 2160.0], (0.5, 0.5), (0.5, 0.5), (0.5, 0.5), 16, 4)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_PauseWindow.uasset", index: 57453, chunk: "44228dddea71f6a700000001", size: 9258, sha256: "8f229e74ae4ef3dab3485095c4849e6a5d6ac70bfbebe1fd35014e508c74c9e6", exports: 40, name: "/Game/UI/BP/Window/BP_PauseWindow", entry: (0, 0, 9),
        slots: &[slot!("Pause", 3, 7002, 54, 10, [1920.0, 590.0, 300.0, 80.0], (0.0, 0.0), (0.0, 0.0), (0.5, 0.0), 25, 12)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_SettingsWindow.uasset", index: 63429, chunk: "ee780ccda0ebf22300000001", size: 13011, sha256: "13f4f74b7e6ba8aa0769e80407da02d2239adf082e7b1316b5b81890fb20508b", exports: 60, name: "/Game/UI/BP/Window/BP_SettingsWindow", entry: (0, 0, 16),
        slots: &[slot!("Background", 19, 10888, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 38, 35)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_SaveSelectWindow.uasset", index: 8853, chunk: "b84702f3604ed2ca00000001", size: 11975, sha256: "ebbaa641f4812201cff22e876672349bdd8e9d04193a361de5018b2f9790205b", exports: 54, name: "/Game/UI/BP/Window/BP_SaveSelectWindow", entry: (0, 0, 10),
        slots: &[slot!("D9Image", 16, 9350, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 32, 29)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_SquareEnixAccountWindow.uasset", index: 43067, chunk: "d5be7f86b2db437f00000001", size: 46449, sha256: "2a54acd02cdf781ca28f133a950d6c01d8e52878eb6390522c3c3e19c2739857", exports: 226, name: "/Game/UI/BP/Window/BP_SquareEnixAccountWindow", entry: (0, 0, 13),
        slots: &[
            slot!("CanvasPanel_Background", 45, 33652, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 17, 49),
            slot!("WidgetSwitcher_CurrentView", 46, 33680, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 218, 49),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_OutfitWindow.uasset", index: 47397, chunk: "39f26d001816402200000001", size: 4151, sha256: "39a3a10dea9d625a0a33b3c6abba5ce58717237a0e4c8dd7c9e70127985d3665", exports: 16, name: "/Game/UI/BP/Window/BP_OutfitWindow", entry: (0, 0, 8),
        slots: &[slot!("Background", 5, 3426, 28, 8, [0.0, 0.0, 3840.0, 2162.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 12, 11)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_MontageWindow.uasset", index: 61022, chunk: "fbf2887be131aac800000001", size: 19588, sha256: "0deb19e914eca7c69a6301ba2905a9d5c271c2389c86fe804dc3d179ac2acf60", exports: 94, name: "/Game/UI/BP/Window/BP_MontageWindow", entry: (0, 0, 19),
        slots: &[slot!("Background", 31, 15598, 29, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 61, 58)] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_FRPosterWindow.uasset", index: 70012, chunk: "bf3410da04cb45fe00000001", size: 8704, sha256: "fbd25fcea37bf4e433b30758a084cb679dafe49d2713b62a85c50f91b70a39b4", exports: 42, name: "/Game/UI/BP/Window/BP_FRPosterWindow", entry: (0, 0, 10),
        slots: &[
            slot!("D9Image", 12, 7400, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 23, 22),
            slot!("UpButton", 15, 7542, 28, 8, [1870.0, 84.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 4, 22),
            slot!("DownButton", 13, 7428, 28, 8, [1870.0, 1576.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 17, 22),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Controls/Settings/BP_UISettings.uasset", index: 43040, chunk: "a6626d04d8585e2300000001", size: 13880, sha256: "7642dffde08773ab0a2cb8886039c8fad3df1200332ef57bd1d06bfa485c2bfe", exports: 48, name: "/Game/UI/BP/Controls/Settings/BP_UISettings", entry: (0, 0, 7),
        slots: &[slot!("Buttons", 14, 9014, 29, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 16, 23)] },
    PkgRef { path: "Iris/Content/UI/BP/Controls/Settings/BP_OutfitSettings.uasset", index: 20010, chunk: "9f9a08c8419418fe00000001", size: 5283, sha256: "a702256f382d272eefe5691fc930f2de6873800625b72e52989081447e5eaa8e", exports: 22, name: "/Game/UI/BP/Controls/Settings/BP_OutfitSettings", entry: (0, 0, 2),
        slots: &[slot!("Buttons", 4, 3960, 29, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 6, 10)] },
    PkgRef { path: "Iris/Content/UI/BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI.uasset", index: 9049, chunk: "d598d203f5b3cde700000001", size: 15429, sha256: "dc0009e1dfb92cb7721710e5661b94e4ef1087b256996b1eedc4cab5d6a9d555", exports: 82, name: "/Game/UI/BP/Controls/PlayerMenu/Collectibles/BP_CollectiblePosterUI", entry: (0, 0, 6),
        slots: &[slot!("D9Image", 28, 12581, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 44, 41)] },
    PkgRef { path: "Iris/Content/UI/BP/Controls/PlayerMenu/Collectibles/BP_ChloePhotoPosterUI.uasset", index: 37806, chunk: "3d4a4bf259cd753d00000001", size: 17838, sha256: "c8cdadc6d1389bf563b7e7fb02107429f3eda04e1ff5beb3545f430d4b1ac0fd", exports: 94, name: "/Game/UI/BP/Controls/PlayerMenu/Collectibles/BP_ChloePhotoPosterUI", entry: (0, 0, 9),
        slots: &[
            slot!("D9Image", 34, 14544, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 55, 52),
            slot!("UpButton", 37, 14674, 28, 8, [1870.0, 242.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 3, 52),
            slot!("DownButton", 35, 14572, 28, 8, [1870.0, 1250.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 39, 52),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Controls/PlayerMenu/Collectibles/BP_IrisPhotoPosterUI.uasset", index: 8435, chunk: "9b481604c122c05900000001", size: 18209, sha256: "cd8b6767872a6fe80ad8c773fab3b58e9d4535d0f64af940535e91cd573f8054", exports: 96, name: "/Game/UI/BP/Controls/PlayerMenu/Collectibles/BP_IrisPhotoPosterUI", entry: (0, 0, 9),
        slots: &[
            slot!("D9Image", 35, 14871, 28, 8, [0.0, 0.0, 3840.0, 2160.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 56, 53),
            slot!("UpButton", 38, 15001, 28, 8, [1870.0, 242.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 3, 53),
            slot!("DownButton", 36, 14899, 28, 8, [1870.0, 1250.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 40, 53),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_ObjectInspectWindow.uasset", index: 49149, chunk: "20ec5c61e967133b00000001", size: 12283, sha256: "1a6ab89bf8d75518b83c3640c93640097f57ae2c721e6eb8c3483d02bbaa8551", exports: 50, name: "/Game/UI/BP/Window/BP_ObjectInspectWindow", entry: (0, 0, 10),
        slots: &[
            slot!("UpButton", 21, 8998, 28, 8, [1870.0, 242.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 4, 34),
            slot!("DownButton", 17, 8801, 28, 8, [1870.0, 1250.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 27, 34),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_MainMenuWindow.uasset", index: 60379, chunk: "26f89c0653e4127600000001", size: 10977, sha256: "29e3a4311b824f4b024466b06216be3664dcca0c2436ebe150d9f90cc483a9f9", exports: 52, name: "/Game/UI/BP/Window/BP_MainMenuWindow", entry: (0, 0, 8),
        slots: &[
            slot!("MainButtons", 7, 8407, 29, 8, [220.0, 760.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 19, 30),
            slot!("D9Image", 9, 8465, 29, 8, [184.0, 262.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 32, 30),
            slot!("D9TextBlock", 8, 8436, 29, 8, [220.0, 400.0, 100.0, 100.0], (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), 38, 30),
            slot!("GamerTag", 12, 8619, 84, 9, [220.0, -240.0, 628.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 2, 31),
            slot!("InfocastPanel", 10, 8494, 86, 8, [-220.0, -140.0, 2647.0, 322.0], (1.0, 1.0), (1.0, 1.0), (1.0, 1.0), 29, 31),
        ] },
    PkgRef { path: "Iris/Content/UI/BP/Window/BP_TitleWindow.uasset", index: 9432, chunk: "5e84e8a96ee85bf200000001", size: 3771, sha256: "c9cc3ed06dd17e3ce617a0a3933d9889c5bc4e351d8e8c21ce840474c5e52c27", exports: 12, name: "/Game/UI/BP/Window/BP_TitleWindow", entry: (0, 0, 8),
        slots: &[
            slot!("GamerTag", 5, 3156, 84, 9, [220.0, -100.0, 628.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 1, 9),
            slot!("PressAnyKey", 6, 3240, 80, 9, [220.0, 12.0, 0.0, 0.0], (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), 10, 9),
        ] },
];

/// Reunion (RESEARCH 13i): TOC version 8, container header version 4, the
/// UE 5.3+ package summary. The values were captured on 2026-09-04 from the
/// build installed then. The mod container is built for 5120x2160, written
/// to a temporary folder, and read back through the container reader: every
/// edited float must come back changed and every other byte of each package
/// unchanged.
#[test]
fn reads_and_rewrites_reunions_ui_packages() {
    let Some(paks) = reunion_paks() else { return };
    let started = std::time::Instant::now();
    let game: &dyn Game = &REUNION;
    let ui = game.ui().unwrap();
    let so = load_script_objects(&paks.join("global.utoc")).unwrap();
    assert_eq!(so.len(), 47561);
    assert_eq!(so.get(&5730563978521807506).map(String::as_str), Some("/Script/UMG.CanvasPanelSlot"));

    let mut toc = Toc::open(&paks.join("pakchunk0-Windows.utoc")).unwrap();
    assert_eq!(toc.version, 8);
    assert_eq!(toc.entries(), 83707);
    assert_eq!(toc.blocks.len(), 867741);
    assert_eq!(toc.methods, vec!["None".to_string(), "Oodle".to_string()]);
    assert_eq!(toc.index.len(), 78882);
    assert_eq!(toc.seeds.len(), 41854);
    assert_eq!(toc.unhashed_count, 0);
    assert_eq!(toc.flags, 9);
    eprintln!("pakchunk0 toc in {:?}", started.elapsed());

    let h = toc.find_type(CHUNK_CONTAINER_HEADER).unwrap();
    assert_eq!(h, 72220);
    let header = toc.read(h).unwrap();
    assert_eq!(header.len(), 2831680);
    assert_eq!(to_hex(&hash::sha256(&header)), "e53808be9d9826556a5826e5a13983df4853436e74c2df95b52b6ff25064dee2");
    let (cid, entries) = parse_container_header(&header, 4).unwrap();
    assert_eq!(cid, 0x81e4dd3d9595e447);
    assert_eq!(entries.len(), 59688);
    assert_eq!(build_container_header(cid, &entries, 4), header);
    eprintln!("container header round-tripped in {:?}", started.elapsed());

    check_packages(&mut toc, &entries, &so, Summary::Ue53, REUNION_PACKAGES);
    eprintln!("packages checked in {:?}", started.elapsed());

    let (dw, _) = design_space(5120, 2160, ui.design);
    let mut lines = Vec::new();
    let built = build_mod(&paks, ui, dw, &so, &mut lines).unwrap();
    for l in &lines {
        eprintln!("{l}");
    }
    assert_eq!((built.applied, built.failed), (16, 0));

    let dir = std::env::temp_dir().join(format!("lis-reunion-ui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join(ui.mod_name);
    std::fs::write(base.with_extension("utoc"), &built.utoc).unwrap();
    std::fs::write(base.with_extension("ucas"), &built.ucas).unwrap();
    let mut mod_toc = Toc::open(&base.with_extension("utoc")).unwrap();
    assert_eq!(mod_toc.version, 8);
    assert_eq!(mod_toc.entries(), 17);
    assert_eq!(mod_toc.index.len(), 16);
    let mh = mod_toc.find_type(CHUNK_CONTAINER_HEADER).unwrap();
    let (_, mod_entries) = parse_container_header(&mod_toc.read(mh).unwrap(), 4).unwrap();
    assert_eq!(mod_entries.len(), 16);
    for p in REUNION_PACKAGES {
        let stock = toc.read(toc.index[p.path]).unwrap();
        let idx = mod_toc.index[p.path];
        let data = mod_toc.read(idx).unwrap();
        assert_eq!(data.len(), stock.len(), "{}", p.path);
        assert_eq!(mod_entries[&package_id_of(&mod_toc.chunk_ids[idx])], entries[&package_id_of(&toc.chunk_ids[toc.index[p.path]])]);
        let pkg = ZenPackage::parse(&data, Summary::Ue53).unwrap();
        let mut expected = stock.clone();
        for s in p.slots {
            let (_, slot, base, _) = slot_payload(&pkg, s.widget, &so).unwrap();
            assert_eq!(base, s.base);
            let edit = ui.edits.iter().find(|e| p.path.ends_with(e.package) && e.widget == s.widget).unwrap();
            let want = edit.new.apply(dw, ui.design) as f32;
            assert_eq!(slot.offsets[edit.field as usize], want, "{}: {}", p.path, s.widget);
            let stock_slot: Slot = slot_payload(&ZenPackage::parse(&stock, Summary::Ue53).unwrap(), s.widget, &so).unwrap().1;
            let mut o = stock_slot.offsets;
            o[edit.field as usize] = want;
            assert_eq!(slot.offsets, o);
            expected[base + s.float_at..base + s.float_at + 4].copy_from_slice(&want.to_le_bytes());
        }
        assert!(data == expected, "{}: bytes other than the edited floats changed", p.path);
    }
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("mod container read back in {:?}", started.elapsed());
}

#[test]
fn builds_the_same_container_as_the_python_writer() {
    let Some(paks) = paks() else { return };
    let Ok(reference) = std::env::var("LIS_DE_UI_REFERENCE") else {
        eprintln!("skipped: LIS_DE_UI_REFERENCE is not set");
        return;
    };
    let reference = PathBuf::from(reference);
    let game: &dyn Game = &DOUBLE_EXPOSURE;
    let ui = game.ui().unwrap();
    let so = load_script_objects(&paks.join("global.utoc")).unwrap();
    let (dw, _) = design_space(5120, 2160, ui.design);
    let mut lines = Vec::new();
    let built = build_mod(&paks, ui, dw, &so, &mut lines).unwrap();
    for l in &lines {
        eprintln!("{l}");
    }
    assert_eq!((built.applied, built.failed), (10, 0));
    let want = |ext: &str| std::fs::read(reference.join(format!("{}.{ext}", ui.mod_name))).unwrap();
    assert!(built.pak == want("pak"), "the stub .pak differs from the Python writer's");
    assert!(built.ucas == want("ucas"), "the .ucas differs from the Python writer's");
    assert!(built.utoc == want("utoc"), "the .utoc differs from the Python writer's");
}

/// What tools/assetdump/verify_kraken.py used to check with a native Oodle
/// library next to it: every block the decoder is asked for decodes, at the
/// size the table says. Without the native library there is nothing to
/// compare the bytes with, so this is the check to run after a game update;
/// it reads a sample of pakchunk0 and takes a while, hence ignored.
///
///     cargo test --release -p lis-ultrawide-core --test ui -- --ignored
#[test]
#[ignore]
fn decodes_every_sampled_block_of_the_games_containers() {
    let Some(paks) = paks() else { return };
    let started = std::time::Instant::now();
    let mut total = 0usize;
    let mut blocks = 0usize;
    for (name, step) in [("global.utoc", 1usize), ("pakchunk0-Windows.utoc", 500), ("pakchunk1-Windows.utoc", 500)] {
        let path = paks.join(name);
        if !path.is_file() {
            continue;
        }
        let mut toc = Toc::open(&path).unwrap();
        for bi in (0..toc.blocks.len()).step_by(step) {
            let want = toc.blocks[bi].uncompressed as usize;
            let out = toc.read_block(bi).unwrap_or_else(|e| panic!("{name} block {bi}: {e}"));
            assert_eq!(out.len(), want, "{name} block {bi}");
            total += out.len();
            blocks += 1;
        }
    }
    eprintln!("{blocks} blocks, {:.1} MB decoded in {:?}", total as f64 / 1e6, started.elapsed());
}
