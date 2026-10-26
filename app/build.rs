#[path = "../scripts/platform-cfg.rs"]
mod platform_cfg;

fn main() {
    println!("cargo:rerun-if-changed=../scripts/platform-cfg.rs");
    platform_cfg::configure();
    #[cfg(feature = "tauri")]
    tauri_build::build();
}
