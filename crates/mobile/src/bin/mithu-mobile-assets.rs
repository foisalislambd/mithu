//! Write the phone new-tab bundle into the directory given on the command line.
//!
//! `tools/install-mobile-assets.sh` runs this before the Android and iOS builds.
//! The HTML is not stored in git.

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: mithu-mobile-assets DIRECTORY");
        std::process::exit(2);
    });
    let version = version();
    mithu_mobile::install_assets(
        std::path::Path::new(&dir),
        mithu_mobile::Edition::Lite,
        &version,
    )
    .unwrap_or_else(|error| {
        eprintln!("mobile assets: {error}");
        std::process::exit(1);
    });
}

fn version() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION.md");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim().to_string()))
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}
