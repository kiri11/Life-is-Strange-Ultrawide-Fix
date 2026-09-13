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

/// One section holding the two stock sites, the cine Super call (which
/// must stay untouched), and one int3 run.
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
    let cave_a = BASE + CAVES as u64;

    let mut want = BTreeMap::new();
    want.insert(axis + 6, vec![0xFF]);
    want.insert(axis + 15, vec![0xFF]);
    want.insert(cave_a, hex("0FB683B4020000 8B8BB0020000 81F90000E03F 7612 81F9 A3011840 730A 83E0FE C74748 398EE33F C3"));
    let mut site_a = vec![0xE8];
    site_a.extend_from_slice(&((cave_a as i64 - (gate as i64 + 5)) as i32).to_le_bytes());
    site_a.extend([0x66, 0x90]);
    want.insert(gate, site_a);
    assert_eq!(got, want);
    // the cine Super call stays as it was (RESEARCH 2d)
    assert_eq!(plan.writes.len(), 4);
    assert!(!got.keys().any(|va| (BASE + CINE as u64..BASE + CINE as u64 + 19).contains(va)));

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
/// installer produced (captured on 2026-09-02), less the cine-call reroute
/// that build carried (RESEARCH 2d), and must be found without scanning.
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
        (0x6b7310, "0fb683b40200008b8bb002000081f90000e03f761281f9a3011840730a83e0fec74748398ee33fc3"),
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

// ---- True Colors -----------------------------------------------------------

use lis_ultrawide_core::games::true_colors::plan_true_colors;

fn synthetic_true_colors() -> Vec<u8> {
    let mut d = vec![0x90u8; 0x4000];
    let put = |d: &mut Vec<u8>, at: usize, bytes: Vec<u8>| d[at..at + bytes.len()].copy_from_slice(&bytes);
    put(&mut d, AXIS, hex("3BC17E05 80FA02 742C 80FA01 7426 0FB65334 80FA01"));
    put(&mut d, GATE, hex("0FB68304020000 334730 83E001 314730"));
    d[CAVES - 1] = 0xC3;
    d[CAVES..CAVES + 0x100].fill(0xCC);
    d
}

#[test]
fn plans_true_colors_patch_and_branch_target_over_a_synthetic_image() {
    let d = synthetic_true_colors();
    let plan = plan_true_colors(&image(&d), hex4("A3011840")).unwrap();
    let got = writes(&plan);
    let axis = BASE + AXIS as u64;
    let gate = BASE + GATE as u64;
    assert_eq!(got.get(&(axis + 6)), Some(&vec![0xFF]));
    assert_eq!(got.get(&(axis + 11)), Some(&vec![0xFF]));
    let caves: Vec<_> = got.iter().filter(|&(&va, _)| va != axis + 6 && va != axis + 11 && va != gate).collect();
    assert_eq!(caves.len(), 4);
    let expected_sizes = [19, 17, 17, 11];
    for ((_, bytes), size) in caves.iter().zip(expected_sizes) {
        assert_eq!(bytes.len(), size);
        assert!(bytes.iter().all(|&b| b != 0xCC));
    }
    let addresses: Vec<u64> = caves.iter().map(|&(&va, _)| va).collect();
    let target = |bytes: &[u8], at: u64, offset: usize, width: usize| -> u64 {
        let disp = i32::from_le_bytes(bytes[offset + width - 4..offset + width].try_into().unwrap());
        ((at as i64) + offset as i64 + width as i64 + i64::from(disp)) as u64
    };
    let (a, b, c, d_cave) = (addresses[0], addresses[1], addresses[2], addresses[3]);
    assert_eq!(target(caves[0].1, a, 13, 5), b);
    assert_eq!(target(caves[1].1, b, 6, 6), a + 18);
    assert_eq!(target(caves[1].1, b, 12, 5), c);
    assert_eq!(target(caves[2].1, c, 6, 6), a + 18);
    assert_eq!(target(caves[2].1, c, 12, 5), d_cave);
    assert_eq!(caves[3].1, &hex("83E0FE C7472C398EE33F C3"));
    let call = got.get(&gate).unwrap();
    assert_eq!(call.len(), 7);
    assert_eq!(target(call, gate, 0, 5), a);
    assert_eq!(&call[5..], &[0x66, 0x90]);
    assert_eq!(plan.writes.len(), 7);
    for w in &plan.writes {
        assert_eq!(image(&d).read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
    assert!(plan.notes.iter().any(|n| n.contains("camera helper")), "{:?}", plan.notes);
}

#[test]
fn true_colors_planner_fails_closed_for_bad_or_repeated_sites() {
    let d = synthetic_true_colors();
    let mut patched = d.clone();
    let plan = plan_true_colors(&image(&patched), hex4("A3011840")).unwrap();
    for w in &plan.writes {
        let at = (w.va - BASE) as usize;
        patched[at..at + w.bytes.len()].copy_from_slice(&w.bytes);
    }
    let err = plan_true_colors(&image(&patched), hex4("A3011840")).unwrap_err();
    assert!(err.contains("already patched"), "{err}");

    let mut ambiguous = d.clone();
    let copy = ambiguous[AXIS..AXIS + 28].to_vec();
    ambiguous[0x100..0x100 + 28].copy_from_slice(&copy);
    let err = plan_true_colors(&image(&ambiguous), hex4("A3011840")).unwrap_err();
    assert!(err.contains("2 times"), "{err}");

    let mut unknown = d;
    unknown[GATE] = 0;
    let err = plan_true_colors(&image(&unknown), hex4("A3011840")).unwrap_err();
    assert!(err.contains("not one the fix knows"), "{err}");
}

#[test]
fn matches_true_colors_sites_and_cave_on_the_real_executable() {
    let path = std::env::var("LIS_TRUE_COLORS_STOCK_EXE").unwrap_or_else(|_| {
        r"D:\Games\Life is Strange True Colors\Siren\Binaries\Win64\Siren-Win64-Shipping.exe".into()
    });
    let Ok(data) = std::fs::read(&path) else {
        eprintln!("skipped: no True Colors executable at {path} (set LIS_TRUE_COLORS_STOCK_EXE)");
        return;
    };
    let (headers, img) = pe::file_image(&data).unwrap();
    let started = std::time::Instant::now();
    let plan = plan_true_colors(&img, lis_ultrawide_core::camera::gate_upper(5120, 2160)).unwrap();
    eprintln!("planned in {:?}: {:?}", started.elapsed(), plan.notes);
    let mut got = writes(&plan);
    assert_eq!(got.remove(&(0x23EFC3Fu64 + 6)), Some(vec![0xFF]));
    assert_eq!(got.remove(&(0x23EFC3Fu64 + 11)), Some(vec![0xFF]));
    let cave_writes = &plan.writes[2..6];
    assert_eq!(cave_writes.iter().map(|w| w.bytes.len()).collect::<Vec<_>>(), vec![19, 17, 17, 11]);
    let caves: Vec<u64> = cave_writes.iter().map(|w| w.va).collect();
    let target = |bytes: &[u8], at: u64, offset: usize, width: usize| -> u64 {
        let disp = i32::from_le_bytes(bytes[offset + width - 4..offset + width].try_into().unwrap());
        ((at as i64) + offset as i64 + width as i64 + i64::from(disp)) as u64
    };
    assert_eq!(target(&cave_writes[0].bytes, caves[0], 13, 5), caves[1]);
    assert_eq!(target(&cave_writes[1].bytes, caves[1], 6, 6), caves[0] + 18);
    assert_eq!(target(&cave_writes[1].bytes, caves[1], 12, 5), caves[2]);
    assert_eq!(target(&cave_writes[2].bytes, caves[2], 6, 6), caves[0] + 18);
    assert_eq!(target(&cave_writes[2].bytes, caves[2], 12, 5), caves[3]);
    assert_eq!(&cave_writes[3].bytes[..7], &[0x83, 0xE0, 0xFE, 0xC7, 0x47, 0x2C, 0x39]);
    assert_eq!(cave_writes[3].bytes[7..], hex("8EE33F C3"));
    for cave in &caves { got.remove(cave); }
    let site = got.remove(&0x23FB3FC).expect("gate site");
    assert_eq!(&site[0..1], &[0xE8]);
    assert_eq!(&site[5..], &[0x66, 0x90]);
    let target = 0x23FB3FCi64 + 5 + i64::from(i32::from_le_bytes(site[1..5].try_into().unwrap()));
    assert_eq!(target as u64, caves[0]);
    assert!(got.is_empty(), "unexpected writes: {got:?}");
    for w in &plan.writes {
        assert_eq!(img.read(w.va, w.expected.len()).unwrap(), &w.expected[..], "{}", w.what);
    }
    assert!(plan.notes.iter().all(|n| !n.contains("moved")), "{:?}", plan.notes);
    let _ = headers;
}