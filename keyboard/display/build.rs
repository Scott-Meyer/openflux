fn main() {
    println!("cargo:rerun-if-env-changed=OPENFLUX_VERSION");
    let ver = std::env::var("OPENFLUX_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=OPENFLUX_DISPLAY_PKG_VERSION={ver}");
    println!("cargo:rustc-env=OPENFLUX_DISPLAY_VERSION_STR=openflux-display {ver}");
}
