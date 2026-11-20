fn main() {
    println!("cargo:rerun-if-env-changed=OPENFLUX_VERSION");
    let ver = std::env::var("OPENFLUX_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=OPENFLUX_BRIDGE_VERSION_STR=openflux-bridge {ver}");
}
