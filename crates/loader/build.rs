// The version resource. Wine 11.6 and later load a native DLL found next to
// the game in preference to their own whenever the DLL names a company other
// than Microsoft, which is what will make the Proton registry override
// unnecessary once Proton picks that Wine up. The installer also looks for
// this resource's strings to tell the fix's DLL from another mod's winhttp.dll.
fn main() {
    println!("cargo:rerun-if-env-changed=LIS_FIX_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The release build passes the release tag (2026.09.02.01); a local build
    // says so instead of pretending to be a release.
    let version = std::env::var("LIS_FIX_VERSION").unwrap_or_else(|_| "0.0.0.0".to_string());
    let numeric = version
        .split('.')
        .map(|p| p.parse::<u16>().unwrap_or(0) as u64)
        .chain(std::iter::repeat(0))
        .take(4)
        .fold(0u64, |n, p| (n << 16) | p);
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", "Kiri11");
    res.set("ProductName", "Life is Strange - Ultrawide Fix");
    res.set("FileDescription", "Ultrawide camera fix loader for Life is Strange: Double Exposure and Reunion");
    res.set("InternalName", "LiSUltrawideCamera");
    res.set("OriginalFilename", "LiSUltrawideCamera.dll");
    res.set("LegalCopyright", "Copyright (C) 2026 Kiri11. GPL-3.0-or-later; see LICENSE.");
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set_version_info(winresource::VersionInfo::FILEVERSION, numeric);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    res.compile().expect("compile the version resource");
}
