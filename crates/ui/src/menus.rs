//! Desktop menus shared by both editions. Names stay Mithu's. There is no
//! camera-search item and no other browser's private-mode name.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuId {
    NewTab,
    NewWindow,
    PrivateWindow,
    Passwords,
    History,
    Downloads,
    Bookmarks,
    TabGroups,
    Extensions,
    DeleteData,
    Zoom,
    ZoomOut,
    ZoomIn,
    ZoomReset,
    Fullscreen,
    Print,
    Translate,
    Find,
    FindNext,
    SaveShare,
    SavePage,
    CopyLink,
    SendDevice,
    MoreTools,
    CreateShortcut,
    NameWindow,
    Performance,
    TaskManager,
    DevTools,
    Help,
    About,
    Settings,
    Exit,
    Back,
    Forward,
    Reload,
    Bookmark,
    BookmarkManager,
    ShowBookmarksBar,
    PasswordManager,
    Payments,
    Addresses,
    ManageExtensions,
    NewTabGroup,
    OpenLinkTab,
    OpenLinkWindow,
    OpenLinkPrivate,
    CopyLinkAddress,
    SaveLink,
    OpenImageTab,
    SaveImage,
    CopyImage,
    DuplicateTab,
    CloseTab,
    CloseOther,
    ReopenClosed,
    PasteAndGo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Separator,
}

pub struct Row {
    pub id: MenuId,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub submenu: bool,
}

pub fn app_menu() -> Vec<Row> {
    vec![
        row(MenuId::NewTab, "New tab", "Ctrl+T", false),
        row(MenuId::NewWindow, "New window", "Ctrl+N", false),
        row(MenuId::PrivateWindow, "New private window", "Ctrl+Shift+N", false),
        sep(),
        row(MenuId::Passwords, "Passwords and autofill", "", true),
        row(MenuId::History, "History", "", true),
        row(MenuId::Downloads, "Downloads", "Ctrl+J", false),
        row(MenuId::Bookmarks, "Bookmarks and lists", "", true),
        row(MenuId::TabGroups, "Tab groups", "", true),
        row(MenuId::Extensions, "Extensions", "", true),
        sep(),
        row(MenuId::DeleteData, "Delete browsing data", "Ctrl+Shift+Delete", false),
        sep(),
        row(MenuId::Zoom, "Zoom", "", false),
        sep(),
        row(MenuId::Print, "Print", "Ctrl+P", false),
        row(MenuId::Translate, "Translate", "", false),
        row(MenuId::Find, "Find and edit", "", true),
        row(MenuId::SaveShare, "Save and share", "", true),
        sep(),
        row(MenuId::MoreTools, "More tools", "", true),
        sep(),
        row(MenuId::Help, "Help", "", true),
        sep(),
        row(MenuId::Settings, "Settings", "", false),
        row(MenuId::Exit, "Exit", "Alt+F4", false),
    ]
}

pub fn children(id: MenuId) -> Vec<Row> {
    match id {
        MenuId::Passwords => vec![
            row(MenuId::PasswordManager, "Password manager", "", false),
            row(MenuId::Payments, "Payment methods", "", false),
            row(MenuId::Addresses, "Addresses and more", "", false),
        ],
        MenuId::History => vec![
            row(MenuId::History, "History", "Ctrl+H", false),
            row(MenuId::DeleteData, "Delete browsing data", "Ctrl+Shift+Delete", false),
        ],
        MenuId::Bookmarks => vec![
            row(MenuId::BookmarkManager, "Bookmark manager", "Ctrl+Shift+O", false),
            row(MenuId::Bookmark, "Bookmark this tab", "Ctrl+D", false),
            row(MenuId::ShowBookmarksBar, "Show bookmarks bar", "Ctrl+Shift+B", false),
        ],
        MenuId::TabGroups => vec![row(MenuId::NewTabGroup, "New tab group", "", false)],
        MenuId::Extensions => vec![row(MenuId::ManageExtensions, "Manage extensions", "", false)],
        MenuId::Find => vec![
            row(MenuId::Find, "Find", "Ctrl+F", false),
            row(MenuId::FindNext, "Find next", "Ctrl+G", false),
        ],
        MenuId::SaveShare => vec![
            row(MenuId::SavePage, "Save page as", "Ctrl+S", false),
            row(MenuId::CopyLink, "Copy link", "", false),
            row(MenuId::SendDevice, "Send to a device", "", false),
        ],
        MenuId::MoreTools => vec![
            row(MenuId::SavePage, "Save page as", "Ctrl+S", false),
            row(MenuId::CreateShortcut, "Create shortcut", "", false),
            row(MenuId::NameWindow, "Name window", "", false),
            row(MenuId::Performance, "Performance", "", false),
            row(MenuId::TaskManager, "Task manager", "", false),
            row(MenuId::DevTools, "Developer tools", "Ctrl+Shift+I", false),
        ],
        MenuId::Help => vec![
            row(MenuId::About, "About Mithu", "", false),
            row(MenuId::Help, "Help", "", false),
        ],
        _ => Vec::new(),
    }
}

pub fn page_menu() -> Vec<Row> {
    vec![
        row(MenuId::Back, "Back", "Alt+Left", false),
        row(MenuId::Forward, "Forward", "Alt+Right", false),
        row(MenuId::Reload, "Reload", "Ctrl+R", false),
        sep(),
        row(MenuId::SavePage, "Save as", "Ctrl+S", false),
        row(MenuId::Print, "Print", "Ctrl+P", false),
        row(MenuId::Translate, "Translate", "", false),
        sep(),
        row(MenuId::Bookmark, "Bookmark this page", "Ctrl+D", false),
        row(MenuId::Find, "Find", "Ctrl+F", false),
    ]
}

pub fn link_menu() -> Vec<Row> {
    vec![
        row(MenuId::OpenLinkTab, "Open link in new tab", "", false),
        row(MenuId::OpenLinkWindow, "Open link in new window", "", false),
        row(MenuId::OpenLinkPrivate, "Open link in private window", "", false),
        sep(),
        row(MenuId::CopyLinkAddress, "Copy link address", "", false),
        row(MenuId::SaveLink, "Save link as", "", false),
    ]
}

pub fn image_menu() -> Vec<Row> {
    vec![
        row(MenuId::OpenImageTab, "Open image in new tab", "", false),
        row(MenuId::SaveImage, "Save image as", "", false),
        row(MenuId::CopyImage, "Copy image address", "", false),
    ]
}

pub fn tab_menu() -> Vec<Row> {
    vec![
        row(MenuId::NewTab, "New tab to the right", "", false),
        row(MenuId::Reload, "Reload", "Ctrl+R", false),
        row(MenuId::DuplicateTab, "Duplicate", "", false),
        sep(),
        row(MenuId::CloseTab, "Close", "Ctrl+W", false),
        row(MenuId::CloseOther, "Close other tabs", "", false),
        row(MenuId::ReopenClosed, "Reopen closed tab", "Ctrl+Shift+T", false),
    ]
}

pub fn omnibox_menu() -> Vec<Row> {
    vec![
        row(MenuId::PasteAndGo, "Paste and go", "", false),
        sep(),
        row(MenuId::Copy, "Copy", "Ctrl+C", false),
        row(MenuId::Cut, "Cut", "Ctrl+X", false),
        row(MenuId::Paste, "Paste", "Ctrl+V", false),
        row(MenuId::SelectAll, "Select all", "Ctrl+A", false),
    ]
}

pub fn rows_named(name: &str) -> Vec<Row> {
    match name {
        "page" => page_menu(),
        "link" => link_menu(),
        "image" => image_menu(),
        "tab" => tab_menu(),
        "omnibox" => omnibox_menu(),
        _ => app_menu(),
    }
}

pub fn labels(rows: &[Row], zoom_percent: u32) -> Vec<String> {
    rows.iter()
        .map(|row| {
            if row.id == MenuId::Zoom {
                format!("Zoom\t{zoom_percent}%")
            } else {
                paint_label(row)
            }
        })
        .collect()
}

pub fn paint_label(row: &Row) -> String {
    if row.id == MenuId::Separator {
        return String::new();
    }
    let mut text = row.label.to_string();
    if row.submenu {
        text.push('\u{203a}');
    }
    if !row.shortcut.is_empty() {
        text.push('\t');
        text.push_str(row.shortcut);
    }
    text
}

fn row(id: MenuId, label: &'static str, shortcut: &'static str, submenu: bool) -> Row {
    Row { id, label, shortcut, submenu }
}

fn sep() -> Row {
    Row { id: MenuId::Separator, label: "", shortcut: "", submenu: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_menu_follows_the_desktop_order_without_another_brands_services() {
        let labels: Vec<&str> = app_menu().iter().map(|row| row.label).filter(|label| !label.is_empty()).collect();
        assert_eq!(
            labels,
            vec![
                "New tab",
                "New window",
                "New private window",
                "Passwords and autofill",
                "History",
                "Downloads",
                "Bookmarks and lists",
                "Tab groups",
                "Extensions",
                "Delete browsing data",
                "Zoom",
                "Print",
                "Translate",
                "Find and edit",
                "Save and share",
                "More tools",
                "Help",
                "Settings",
                "Exit",
            ]
        );
        let blob = format!("{labels:?} {:?}", children(MenuId::MoreTools).iter().map(|row| row.label).collect::<Vec<_>>()).to_ascii_lowercase();
        assert!(!blob.contains("google"));
        assert!(!blob.contains("chrome"));
        assert!(!blob.contains("lens"));
        assert!(!blob.contains("incognito"));
        assert!(children(MenuId::History).iter().any(|row| row.label == "History"));
        assert_eq!(rows_named("tab")[0].label, "New tab to the right");
        assert_eq!(rows_named("image")[0].label, "Open image in new tab");
        assert_eq!(rows_named("link")[0].label, "Open link in new tab");
        assert_eq!(rows_named("omnibox")[0].label, "Paste and go");
    }
}
