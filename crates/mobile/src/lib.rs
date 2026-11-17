//! Shared logic for the Android and iOS shells.
//!
//! Both apps use the same `mithu://` documents as the desktop builds. Lite is
//! a system web view (`org.mithu.lite`). The Servo edition is a separate
//! install id (`org.mithu.servo`) so the two can sit side by side. This crate
//! does not link Servo or a web view: the platform projects under `mobile/`
//! load the HTML this crate builds. Signing an APK or IPA needs the Android
//! SDK or Xcode, which this tree does not run.

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
            std::fs::write(path, doc.html).expect("asset");
        }
    }
}
