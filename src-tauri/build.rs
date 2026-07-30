fn main() {
    if is_windows_msvc() {
        // tauri-build normally links its manifest only into application binaries. The lib unit
        // test harness is a separate executable, so use link.exe to embed the same manifest into
        // every linked artifact while keeping the icon/version resources managed by tauri-build.
        let attributes = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attributes).expect("failed to run Tauri build script");
        embed_windows_manifest();
    } else {
        tauri_build::build();
    }
}

fn is_windows_msvc() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
}

fn embed_windows_manifest() {
    let manifest = std::path::Path::new(
        &std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is unavailable"),
    )
    .join("windows-app-manifest.xml");

    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifest
            .to_str()
            .expect("Windows manifest path is not valid UTF-8")
    );
}
