#[path = "../scripts/platform-cfg.rs"]
mod platform_cfg;

fn main() {
    println!("cargo:rerun-if-changed=../scripts/platform-cfg.rs");
    platform_cfg::configure();
    #[cfg(feature = "tauri")]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
            println!("cargo:rerun-if-changed=../ios/DownloadDirectory.m");
            cc::Build::new()
                .file("../ios/DownloadDirectory.m")
                .flag("-fobjc-arc")
                .compile("rssh_download_directory");
            println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        }
        tauri_build::build();
    }
}
