fn main() {
    // Application OS names are mutually exclusive. Rust's target_os="linux"
    // also includes OHOS; Tauri's mobile/desktop aliases vary between backends.
    // Read the compilation target, not the host running this build script.
    // `windows` already exists as a built-in Rust cfg.
    for os in ["linux", "macos", "android", "ios", "ohos"] {
        println!("cargo:rustc-check-cfg=cfg({os})");
    }
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").expect("Cargo target environment");
    let os = match (target_os.as_str(), target_env.as_str()) {
        ("linux", "ohos") => "ohos",
        ("linux", _) => "linux",
        ("macos", _) => "macos",
        ("windows", _) => "windows",
        ("android", _) => "android",
        ("ios", _) => "ios",
        _ => panic!("RSSH does not support target OS {target_os} with environment {target_env}"),
    };
    if os != "windows" {
        println!("cargo:rustc-cfg={os}");
    }

    tauri_build::build()
}
