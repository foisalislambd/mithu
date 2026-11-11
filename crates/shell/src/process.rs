//! Browser-process and renderer split.
//!
//! Chromium runs the window, navigation, and network in the browser process
//! (`chrome/browser`, `content/browser`, `net/`) and HTML, CSS, layout, paint,
//! and JavaScript in a renderer process (`content/renderer`, Blink, V8).
//! Mithu uses the same cut. Both sides still run in one process, which is
//! Chromium's `--single-process` mode: a second process would cost more memory
//! than this browser is willing to spend. These messages are the boundary a
//! later sandbox can send across a pipe.

/// The variants are the cut between processes. Only `Commit` and `Committed` are sent today.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BrowserToRenderer {
    /// The browser fetched a document and the renderer should parse it.
    Commit {
        url: String,
        html: String,
    },
    Resize {
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RendererToBrowser {
    /// The renderer finished the document and reports the title it found.
    Committed {
        url: String,
        title: String,
    },
    /// A script, link, or form asked the browser to navigate.
    Navigate {
        url: String,
    },
    Failed {
        url: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_names_the_document_the_renderer_took() {
        let message = RendererToBrowser::Committed {
            url: "https://example.com/".into(),
            title: "Example Domain".into(),
        };
        match message {
            RendererToBrowser::Committed { url, title } => {
                assert_eq!(url, "https://example.com/");
                assert_eq!(title, "Example Domain");
            }
            _ => panic!("expected a commit"),
        }
    }
}
