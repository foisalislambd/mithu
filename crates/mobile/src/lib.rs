//! Shared logic for the Android and iOS shells.
//!
//! Both apps use the same `mithu://` documents as the desktop builds. Lite is
//! a system web view (`org.mithu.lite`). The Servo edition is a separate
//! install id (`org.mithu.servo`) so the two can sit side by side. This crate
//! does not link Servo or a web view: the platform projects under `mobile/`
//! load the HTML this crate builds. Signing an APK or IPA needs the Android
//! SDK or Xcode, which this tree does not run.

use std::fs;
use std::path::{Path, PathBuf};

use mithu_ui::pages::{self, Document, Prefs};

pub const LITE_APP_ID: &str = "org.mithu.lite";
pub const SERVO_APP_ID: &str = "org.mithu.servo";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    Lite,
    Servo,
}

impl Edition {
    pub fn app_id(self) -> &'static str {
        match self {
            Edition::Lite => LITE_APP_ID,
            Edition::Servo => SERVO_APP_ID,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Edition::Lite => "Mithu Lite",
            Edition::Servo => "Mithu",
        }
    }
}

/// The first screen. Both editions open the shared new-tab document.
pub fn start_document(edition: Edition, prefs: &Prefs, version: &str) -> Document {
    let mut document = pages::open("mithu://newtab", prefs, version).expect("new tab");
    document.title = edition.display_name().to_string();
    document
}

pub fn open(spec: &str, prefs: &Prefs, version: &str) -> Option<Document> {
    pages::open(spec, prefs, version)
}

/// Write `newtab.html` plus the logo, mark, and Roboto files it references.
///
/// The HTML is the same document as the desktop new tab, with file URLs
/// instead of base64. Callers pass a directory that becomes the Android
/// asset root or the iOS app bundle.
pub fn install_assets(dir: &Path, edition: Edition, version: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir.join("fonts"))?;
    pages::use_file_assets(true);
    let document = start_document(edition, &Prefs::default(), version);
    pages::use_file_assets(false);
    if document.html.contains("base64,") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mobile page still embeds assets",
        ));
    }
    fs::write(dir.join("newtab.html"), document.html)?;
    let root = repo_root();
    fs::copy(
        root.join("assets/brand/png/logo-512.png"),
        dir.join("logo.png"),
    )?;
    fs::copy(root.join("assets/brand/png/mark-64.png"), dir.join("mark.png"))?;
    for name in ["roboto-regular.woff2", "roboto-medium.woff2", "roboto-bold.woff2"] {
        fs::copy(
            root.join("assets/fonts/roboto").join(name),
            dir.join("fonts").join(name),
        )?;
    }
    Ok(())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_ids_and_the_first_page() {
        assert_eq!(Edition::Lite.app_id(), "org.mithu.lite");
        assert_eq!(Edition::Servo.app_id(), "org.mithu.servo");
        let doc = start_document(Edition::Lite, &Prefs::default(), "0.1.5");
        assert_eq!(doc.url, "mithu://newtab");
        assert!(doc.html.contains("Mithu"));
        assert!(open("mithu://settings", &Prefs::default(), "0.1.5").is_some());
        if let Ok(path) = std::env::var("MITHU_MOBILE_ASSET") {
            let dir = std::path::PathBuf::from(path);
            install_assets(&dir, Edition::Lite, "0.1.7").expect("assets");
            let html = std::fs::read_to_string(dir.join("newtab.html")).expect("html");
            assert!(html.contains("fonts/roboto-regular.woff2"));
            assert!(html.contains("logo.png"));
            assert!(!html.contains("base64,"));
            assert!(dir.join("logo.png").is_file());
            assert!(dir.join("fonts/roboto-bold.woff2").is_file());
        }
    }
}
