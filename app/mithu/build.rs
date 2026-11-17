fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let path = std::path::Path::new(&manifest).join("../../VERSION.md");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let version = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("0.0.0");
    println!("cargo:rustc-env=MITHU_VERSION={version}");

    // Servo and SpiderMonkey need more than the 1MB Windows main-thread stack.
    // The event loop has to stay on that thread, so the reserve is raised here
    // instead of moving the loop onto a worker (which winit rejects).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/STACK:33554432");
    }
}
