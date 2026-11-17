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
    embed_icon("assets/brand/mithu.ico");
}

fn embed_icon(relative: &str) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let icon = std::path::Path::new(&manifest).join("../..").join(relative);
    println!("cargo:rerun-if-changed={}", icon.display());
    // The resource compiler ships with the Windows SDK and is often off PATH.
    // Linux and macOS hosts keep the .ico in the tree; the Windows release
    // job links it into the exe.
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let Some(rc) = rc_compiler() else {
            println!("cargo:warning=rc.exe was not found; {relative} stays next to the tree");
            return;
        };
        let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
        let rc_path = std::path::Path::new(&out_dir).join("mithu-icon.rc");
        let res_path = std::path::Path::new(&out_dir).join("mithu-icon.res");
        let icon_rc = icon.display().to_string().replace('\\', "\\\\");
        let _ = std::fs::write(&rc_path, format!("1 ICON \"{icon_rc}\"\n"));
        let compiled = std::process::Command::new(rc)
            .arg("/nologo")
            .arg(format!("/fo{}", res_path.display()))
            .arg(&rc_path)
            .status()
            .ok()
            .is_some_and(|status| status.success());
        if compiled {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        } else {
            println!("cargo:warning=rc.exe did not embed {relative}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = icon;
    }
}

#[cfg(windows)]
fn rc_compiler() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("RC") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if std::process::Command::new("rc.exe")
        .arg("/?")
        .output()
        .is_ok()
    {
        return Some(std::path::PathBuf::from("rc.exe"));
    }
    let kits = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = std::fs::read_dir(kits).ok()?.flatten().collect();
    versions.sort_by_key(|entry| entry.file_name());
    for entry in versions.into_iter().rev() {
        let candidate = entry.path().join("x64").join("rc.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
