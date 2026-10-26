fn main() {
    println!("cargo:rerun-if-changed=native");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            cc::Build::new()
                .file("native/drag_macos.m")
                .file("native/session_macos.m")
                .flag("-fobjc-arc")
                .flag("-fblocks")
                .flag("-mmacosx-version-min=11.0")
                .compile("immich_drag");
            println!("cargo:rustc-link-lib=framework=AppKit");
            println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
            println!("cargo:rustc-link-lib=framework=CoreGraphics");
        }
        Ok("windows") => {
            cc::Build::new()
                .cpp(true)
                .file("native/drag_windows.cpp")
                .flag_if_supported("/std:c++17")
                .compile("immich_drag");
            for lib in ["ole32", "shell32", "user32", "gdi32", "uuid"] {
                println!("cargo:rustc-link-lib={lib}");
            }
        }
        _ => {}
    }
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "drag_test_file",
            "get_settings",
            "save_settings",
            "open_immich",
            "prepare_asset",
            "drag_asset",
            "drag_assets",
            "cancel_drag",
            "reset_configuration",
        ]),
    ))
    .expect("Tauri build failed")
}
