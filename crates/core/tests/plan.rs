//! The planner against a made-up image, and against the real executable when
//! it is on this machine: its output has to be the patch the Python installer
//! wrote, captured as a diff of the stock and patched files.

use std::collections::BTreeMap;

use lis_ultrawide_core::games::double_exposure::plan_double_exposure;
use lis_ultrawide_core::plan::Plan;
use lis_ultrawide_core::{hex, hex4};
use lis_ultrawide_core::pe;
use lis_ultrawide_core::scan::{Image, Section};

fn writes(plan: &Plan) -> BTreeMap<u64, Vec<u8>> {
    plan.writes.iter().map(|w| (w.va, w.bytes.clone())).collect()
}

const BASE: u64 = 0x1000;
const AXIS: usize = 0x1000;
const GATE: usize = 0x2000;
const CINE: usize = 0x3000;
const SUPER: usize = 0x3800;
const CAVES: usize = 0x3900;

/// One section holding the three stock sites and one int3 run.
fn synthetic() -> Vec<u8> {
    let mut d = vec![0x90u8; 0x4000];
    let put = |d: &mut Vec<u8>, at: usize, bytes: Vec<u8>| d[at..at + bytes.len()].copy_from_slice(&bytes);
    put(&mut d, AXIS, hex("3BC17E09 80FA02 0F84D2010000 80FA01 0F84C9010000"));
    put(&mut d, GATE, hex("0FB683B4020000 33474C 83E001 31474C"));
    let call = CINE + 14;
    let disp = (SUPER as i64 - (call as i64 + 5)) as i32;
    let mut cine = hex("E811223344 4C8BC7 0F28CE 488BCB E8");
    cine.extend_from_slice(&disp.to_le_bytes());
    put(&mut d, CINE, cine);
    d[CAVES - 1] = 0xC3;
    d[CAVES..CAVES + 0x100].fill(0xCC);
    d
}

fn image(d: &[u8]) -> Image<'_> {
    Image { sections: vec![Section { va: BASE, data: d }] }
}

#[test]
fn plans_the_documented_patch_over_a_synthetic_image() {
    let d = synthetic();
    let plan = plan_double_exposure(&image(&d), hex4("A3011840")).unwrap();
    let got = writes(&plan);

    let axis = BASE + AXIS as u64;
    let gate = BASE + GATE as u64;
    let call = BASE + CINE as u64 + 14;
    let cave_a = BASE + CAVES as u64;
    let cave_b = cave_a + 40; // the rest of the same run, right after cave A
    let super_va = BASE + SUPER as u64;

    let mut want = BTreeMap::new();
    want.insert(axis + 6, vec![0xFF]);
    want.insert(axis + 15, vec![0xFF]);
    want.insert(cave_a, hex("0FB683B4020000 8B8BB0020000 81F90000E03F 7612 81F9 A3011840 730A 83E0FE C74748 398EE33F C3"));
    let mut site_a = vec![0xE8];
    site_a.extend_from_slice(&((cave_a as i64 - (gate as i64 + 5)) as i32).to_le_bytes());
    site_a.extend([0x66, 0x90]);
    want.insert(gate, site_a);
    let mut blob_b = hex("4883EC28 E8");
    blob_b.extend_from_slice(&((super_va as i64 - (cave_b as i64 + 9)) as i32).to_le_bytes());
    blob_b.extend(hex("4883C428 804F4C01 C3"));
    want.insert(cave_b, blob_b);
    want.insert(call + 1, ((cave_b as i64 - (call as i64 + 5)) as i32).to_le_bytes().to_vec());
    assert_eq!(got, want);

    // every expected byte really is there
    let img = image(&d);
    for w in &plan.writes {
        assert_eq!(img.read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
    // the sites were not where the shipped build has them, so they were scanned for
    assert!(plan.notes.iter().any(|n| n.contains("moved")), "{:?}", plan.notes);
}

#[test]
fn refuses_an_already_patched_image() {
    let mut d = synthetic();
    let plan = plan_double_exposure(&image(&d), hex4("A3011840")).unwrap();
    for w in &plan.writes {
        let at = (w.va - BASE) as usize;
        d[at..at + w.bytes.len()].copy_from_slice(&w.bytes);
    }
    let err = plan_double_exposure(&image(&d), hex4("A3011840")).unwrap_err();
    assert!(err.contains("already patched"), "{err}");
}

#[test]
fn refuses_an_ambiguous_site() {
    let mut d = synthetic();
    let copy = d[AXIS..AXIS + 22].to_vec();
    d[0x100..0x100 + 22].copy_from_slice(&copy);
    let err = plan_double_exposure(&image(&d), hex4("A3011840")).unwrap_err();
    assert!(err.contains("2 times"), "{err}");
}

#[test]
fn refuses_an_unknown_build() {
    let mut d = synthetic();
    d[GATE] = 0x00;
    let err = plan_double_exposure(&image(&d), hex4("A3011840")).unwrap_err();
    assert!(err.contains("not one the fix knows"), "{err}");
}

/// The stock executable, when this machine has it: the plan for 5120x2160
/// must be exactly the diff between the stock file and the one the Python
/// installer produced (captured on 2026-09-02), and must be found without
/// scanning.
#[test]
fn matches_the_reference_patch_on_the_real_executable() {
    // LIS_DE_STOCK_EXE names the stock executable; otherwise the game's own
    // (never modified by this fix) is used from where this machine has it.
    const WIN64: &str = r"D:\SteamLibrary\steamapps\common\LifeIsStrangeDoubleExposure\Chronos\Binaries\Win64";
    let candidates = match std::env::var("LIS_DE_STOCK_EXE") {
        Ok(p) => vec![p],
        Err(_) => vec![
            format!(r"{WIN64}\Chronos-Win64-Shipping.exe.original"),
            format!(r"{WIN64}\Chronos-Win64-Shipping.exe"),
        ],
    };
    let Some((path, data)) = candidates.iter().find_map(|p| std::fs::read(p).ok().map(|d| (p, d))) else {
        eprintln!("skipped: no stock executable found (set LIS_DE_STOCK_EXE)");
        return;
    };
    eprintln!("checking against {path}");
    let (_headers, img) = pe::file_image(&data).unwrap();
    let started = std::time::Instant::now();
    let plan = plan_double_exposure(&img, lis_ultrawide_core::camera::gate_upper(5120, 2160)).unwrap();
    eprintln!("planned in {:?}: {:?}", started.elapsed(), plan.notes);

    let want: BTreeMap<u64, Vec<u8>> = [
        (0x6b265b, "4883ec28e85c78d6034883c428804f4c01c3"),
        (0x6b7310, "0fb683b40200008b8bb002000081f90000e03f761281f9a3011840730a83e0fec74748398ee33fc3"),
        (0x4006587, "d0c06afc"),
        (0x440b5c6, "ff"),
        (0x440b5cf, "ff"),
        (0x441ab4c, "e8bfc729fc6690"),
    ]
    .into_iter()
    .map(|(va, bytes)| (va, hex(bytes)))
    .collect();
    assert_eq!(writes(&plan), want);
    for w in &plan.writes {
        assert_eq!(img.read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
    assert!(plan.notes.iter().all(|n| !n.contains("moved")), "{:?}", plan.notes);
}

// ---- Reunion (RESEARCH.md section 13) --------------------------------------

use lis_ultrawide_core::games::reunion::plan_reunion;

/// One section holding Reunion's two stock sites and one int3 run.
fn synthetic_reunion() -> Vec<u8> {
    let mut d = vec![0x90u8; 0x4000];
    let put = |d: &mut Vec<u8>, at: usize, bytes: Vec<u8>| d[at..at + bytes.len()].copy_from_slice(&bytes);
    put(&mut d, AXIS, hex("3BC17E06 4080FE02 742C 4080FE01 7426"));
    put(&mut d, GATE, hex("0FB68B59020000 334F68 83E101 334F68 894F68"));
    d[CAVES - 1] = 0xC3;
    d[CAVES..CAVES + 0x100].fill(0xCC);
    d
}

#[test]
fn plans_the_reunion_patch_over_a_synthetic_image() {
    let d = synthetic_reunion();
    let plan = plan_reunion(&image(&d), hex4("A3011840")).unwrap();
    let got = writes(&plan);

    let axis = BASE + AXIS as u64;
    let gate = BASE + GATE as u64;
    let cave_a = BASE + CAVES as u64;

    let mut want = BTreeMap::new();
    want.insert(axis + 7, vec![0xFF]);
    want.insert(axis + 13, vec![0xFF]);
    want.insert(cave_a, hex("0FB68B59020000 8B8354020000 3D0000E03F 7611 3D A3011840 730A 83E1FE C7475C 398EE33F C3"));
    let mut site_a = vec![0xE8];
    site_a.extend_from_slice(&((cave_a as i64 - (gate as i64 + 5)) as i32).to_le_bytes());
    site_a.extend([0x66, 0x90]);
    want.insert(gate, site_a);
    assert_eq!(got, want);
    // no cave B for this game: its cutscenes are cine cameras (RESEARCH 13e)
    assert_eq!(plan.writes.len(), 4);

    let img = image(&d);
    for w in &plan.writes {
        assert_eq!(img.read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
}

#[test]
fn reunion_caves_stay_in_the_sites_section() {
    // a second executable section full of padding (Denuvo's, in the real
    // game) must never receive a cave
    let d = synthetic_reunion();
    let mut d2 = d.clone();
    d2[CAVES..CAVES + 0x100].fill(0x90); // no room in the code section
    let other = vec![0xCCu8; 0x1000];
    let img = Image { sections: vec![Section { va: BASE, data: &d2 }, Section { va: 0x10000, data: &other }] };
    let err = plan_reunion(&img, hex4("A3011840")).unwrap_err();
    assert!(err.contains("cave A in the code section"), "{err}");
    let img = Image { sections: vec![Section { va: BASE, data: &d }, Section { va: 0x10000, data: &other }] };
    let plan = plan_reunion(&img, hex4("A3011840")).unwrap();
    assert!(plan.writes.iter().all(|w| w.va < 0x10000), "{:?}", plan.notes);
}

/// The shipped Reunion executable, when this machine has it (RESEARCH 13d,
/// 13e): the sites where the analysis put them, found without scanning, the
/// caves in `.sdata`, and the bytes the section documents.
#[test]
fn matches_the_documented_reunion_sites_on_the_real_executable() {
    let path = std::env::var("LIS_REUNION_STOCK_EXE").unwrap_or_else(|_| {
        r"D:\SteamLibrary\steamapps\common\LifeisStrangeReunion\Iris\Binaries\Win64\Iris-Win64-Shipping.exe".into()
    });
    let Ok(data) = std::fs::read(&path) else {
        eprintln!("skipped: no Reunion executable at {path} (set LIS_REUNION_STOCK_EXE)");
        return;
    };
    let (headers, img) = pe::file_image(&data).unwrap();
    let sdata = headers.sections.iter().find(|s| s.name == ".sdata").expect(".sdata");
    let in_sdata = |va: u64| va >= sdata.va && va < sdata.va + sdata.vsize as u64;
    let started = std::time::Instant::now();
    let plan = plan_reunion(&img, lis_ultrawide_core::camera::gate_upper(5120, 2160)).unwrap();
    eprintln!("planned in {:?}: {:?}", started.elapsed(), plan.notes);

    const AXIS_R: u64 = 0x3698BD4;
    const GATE_R: u64 = 0x36A687D;
    const CAVE_A: u64 = 0xC1C005;
    let rel = |to: u64, next: u64| ((to as i64 - next as i64) as i32).to_le_bytes().to_vec();
    let mut want = BTreeMap::new();
    want.insert(AXIS_R + 7, vec![0xFF]);
    want.insert(AXIS_R + 13, vec![0xFF]);
    want.insert(CAVE_A, hex("0FB68B59020000 8B8354020000 3D0000E03F 7611 3D A3011840 730A 83E1FE C7475C 398EE33F C3"));
    let mut site_a = vec![0xE8];
    site_a.extend(rel(CAVE_A, GATE_R + 5));
    site_a.extend([0x66, 0x90]);
    want.insert(GATE_R, site_a);
    assert_eq!(writes(&plan), want);
    for w in &plan.writes {
        assert!(in_sdata(w.va), "{} at {:#x} is outside .sdata", w.what, w.va);
        assert_eq!(img.read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
    assert!(plan.notes.iter().all(|n| !n.contains("moved")), "{:?}", plan.notes);
}
