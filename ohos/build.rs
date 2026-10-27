#[path = "../scripts/platform-cfg.rs"]
mod platform_cfg;

fn main() {
    println!("cargo:rerun-if-changed=../scripts/platform-cfg.rs");
    println!("cargo:rerun-if-changed=../app/capabilities");
    platform_cfg::configure();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("ohos") {
        tauri_build::try_build(
            tauri_build::Attributes::new().capabilities_path_pattern("../app/capabilities/*.json"),
        )
        .expect("failed to prepare HarmonyOS application permissions");
    }
}
