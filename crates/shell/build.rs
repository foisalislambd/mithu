fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let patch = std::path::Path::new(&manifest).join("../../tools/patch-servo-processes.py");
    if patch.is_file() {
        let status = std::process::Command::new("python3")
            .arg(&patch)
            .status()
            .unwrap_or_else(|error| panic!("could not run the Servo process patch: {error}"));
        if !status.success() {
            panic!("the Servo process patch failed: {status}");
        }
    }
    let path = std::path::Path::new(&manifest).join("../../VERSION.md");
    println!("cargo:rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let version = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("0.0.0");
    println!("cargo:rustc-env=MITHU_VERSION={version}");
}
