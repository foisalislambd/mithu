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
    embed_icon("assets/brand/mithu-lite.ico");
}

fn embed_icon(relative: &str) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let icon = std::path::Path::new(&manifest).join("../..").join(relative);
    println!("cargo:rerun-if-changed={}", icon.display());
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
        let rc_path = std::path::Path::new(&out_dir).join("mithu-icon.rc");
        let res_path = std::path::Path::new(&out_dir).join("mithu-icon.res");
        let icon_rc = icon.display().to_string().replace('\\', "\\\\");
        let _ = std::fs::write(&rc_path, format!("1 ICON \"{icon_rc}\"\n"));
        let compiled = std::process::Command::new("rc.exe")
            .arg("/nologo")
            .arg(format!("/fo{}", res_path.display()))
            .arg(&rc_path)
            .status()
            .ok()
            .is_some_and(|status| status.success());
        if compiled {
            println!("cargo:rustc-link-arg={}", res_path.display());
        } else {
            println!("cargo:warning=rc.exe did not embed {relative}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = icon;
    }
}
