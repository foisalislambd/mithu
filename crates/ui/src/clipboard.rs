//! Text clipboard shared by both editions.
//!
//! The system clipboard is used when it can be opened. A process-local
//! fallback keeps cut, copy, and paste working when the desktop clipboard
//! is unavailable, which is the case in headless tests.

use std::sync::Mutex;

static FALLBACK: Mutex<String> = Mutex::new(String::new());

pub fn set_text(text: &str) {
    if let Ok(mut board) = arboard::Clipboard::new() {
        let _ = board.set_text(text.to_string());
    }
    if let Ok(mut slot) = FALLBACK.lock() {
        *slot = text.to_string();
    }
}

pub fn get_text() -> String {
    if let Ok(mut board) = arboard::Clipboard::new() {
        if let Ok(text) = board.get_text() {
            if !text.is_empty() {
                return text;
            }
        }
    }
    FALLBACK.lock().map(|slot| slot.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_round_trip() {
        set_text("mithu-clipboard");
        let text = FALLBACK.lock().expect("clipboard").clone();
        assert_eq!(text, "mithu-clipboard");
    }
}
