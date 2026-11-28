fn main() {
    println!("cargo:rerun-if-env-changed=KEYGLASS_VERSION");
    let ver = std::env::var("KEYGLASS_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=KEYGLASS_DISPLAY_PKG_VERSION={ver}");
    println!("cargo:rustc-env=KEYGLASS_DISPLAY_VERSION_STR=keyglass-display {ver}");
}
