#[path = "../scripts/platform-cfg.rs"]
mod platform_cfg;

fn main() {
    platform_cfg::configure();
}
