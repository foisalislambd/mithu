//! Paint the desktop frame headlessly and write PNGs for review.
//!
//! `cargo run -p mithu-ui --example chrome_shots -- <out-dir>`

use mithu_ui::chrome::{self, Popup, Secure, State, TabPaint};

fn tab(title: &str, group: u8) -> TabPaint {
    TabPaint { title: title.into(), favicon: None, group, group_name: String::new(), collapsed: false }
}

struct Scene {
    name: &'static str,
    dark: bool,
    scale: f32,
    pointer: (f32, f32),
    omnibox: &'static str,
    secure: Secure,
    focused: bool,
    menu: bool,
    loading: bool,
    bookmarks: bool,
    /// New Tab page overlay, full window height.
    ntp: bool,
    /// Named tab group with its editor open and a permission prompt.
    group: bool,
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "shots".into());
    std::fs::create_dir_all(&out).expect("out dir");
    let fonts = mithu_text::FontBook::load();
    let base = Scene {
        name: "light",
        dark: false,
        scale: 1.0,
        pointer: (-1.0, -1.0),
        omnibox: "https://en.wikipedia.org/wiki/Main_Page",
        secure: Secure::Https,
        focused: false,
        menu: false,
        loading: false,
        bookmarks: true,
        ntp: false,
        group: false,
    };
    let scenes = [
        Scene { ..base },
        Scene { name: "dark", dark: true, ..base },
        Scene { name: "light-hover-tab", pointer: (420.0, 22.0), ..base },
        Scene { name: "dark-hover-tab", dark: true, pointer: (420.0, 22.0), ..base },
        Scene { name: "light-hover-back", pointer: (60.0, 63.0), ..base },
        Scene { name: "light-http", omnibox: "http://neverssl.com/", secure: Secure::Http, ..base },
        Scene { name: "light-newtab", omnibox: "", secure: Secure::Internal, ..base },
        Scene { name: "light-focused", focused: true, ..base },
        Scene { name: "light-menu", menu: true, ..base },
        Scene { name: "dark-menu", dark: true, menu: true, ..base },
        Scene { name: "light-loading", loading: true, ..base },
        Scene { name: "light-2x", scale: 2.0, ..base },
        Scene { name: "light-nobookmarks", bookmarks: false, ..base },
        Scene { name: "dark-newtab-page", dark: true, omnibox: "", secure: Secure::Internal, ntp: true, ..base },
        Scene { name: "light-menu-newtab", omnibox: "", secure: Secure::Internal, menu: true, ntp: true, ..base },
        Scene { name: "light-group-permission", omnibox: "https://example.com/notes", ntp: false, group: true, ..base },
    ];
    for scene in scenes {
        let s = scene.scale;
        let (w, h) = (1280.0 * s, 220.0 * s);
        let mut tabs = vec![
            tab("Wikipedia, the free encyclopedia", 0),
            tab("Rust Programming Language", 0),
            tab("GitHub - foisalislambd/mithu: A Rust browser", 0),
            tab("New Tab", 0),
        ];
        if scene.ntp {
            tabs = vec![tab("New Tab", 0)];
        }
        if scene.group {
            tabs = vec![tab("Notes - example.com", 1), tab("Maps - example.com", 1), tab("Rust Programming Language", 0)];
            tabs[0].group_name = "Work".into();
            tabs[1].group_name = "Work".into();
        }
        let menu_labels: Vec<String> = mithu_ui::menus::labels(&mithu_ui::menus::app_menu(), 100);
        let state = State {
            width: w,
            height: h,
            tabs,
            active: 0,
            omnibox: scene.omnibox.into(),
            omnibox_focused: scene.focused,
            cursor: scene.omnibox.chars().count(),
            secure: scene.secure,
            bookmarked: false,
            bookmarks: vec![
                ("Mithu".into(), "https://github.com/foisalislambd/mithu".into()),
                ("Rust docs".into(), "https://doc.rust-lang.org".into()),
                ("Wikipedia".into(), "https://wikipedia.org".into()),
            ],
            bookmarks_open: scene.bookmarks,
            can_back: true,
            can_forward: false,
            loading: scene.loading,
            progress: 0.4,
            link_status: String::new(),
            pointer: (scene.pointer.0 * s, scene.pointer.1 * s),
            pressed: false,
            ntp: scene.ntp,
            menu: if scene.menu { Some(Popup::App) } else { None },
            menu_labels,
            submenu_at: None,
            submenu_labels: Vec::new(),
            tab_search: false,
            extensions: false,
            profile: false,
            suggestions: Vec::new(),
            suggestion: None,
            find_open: false,
            find_query: String::new(),
            fonts: &fonts,
            scale: s,
            dark: scene.dark,
            show_home: true,
            group_editor: scene.group,
            permission_prompt: if scene.group { "example.com wants to use your camera" } else { "" },
        };
        let full = scene.ntp || scene.group;
        let (pw, ph) = (w as u32, if full { 800.0 * s } else if scene.menu { 720.0 * s } else { h } as u32);
        let state = State { height: ph as f32, ..state };
        let page = if scene.dark { 0x202124 } else { 0xffffff };
        let mut pixels = vec![page; (pw * ph) as usize];
        chrome::paint(&mut pixels, pw, ph, &state);
        let path = format!("{out}/{}.png", scene.name);
        let file = std::fs::File::create(&path).expect("png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), pw, ph);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        let mut bytes = Vec::with_capacity((pw * ph * 3) as usize);
        for px in pixels {
            bytes.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8]);
        }
        writer.write_image_data(&bytes).expect("write");
        println!("{path}");
    }
}
