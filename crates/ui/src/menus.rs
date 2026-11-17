//! App and context menus. Names stay Mithu's: private window, not another
//! browser's private mode, and no search-by-camera item.

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
    ZoomOut,
    ZoomIn,
    Fullscreen,
    Print,
    Translate,
    Find,
    SaveShare,
    MoreTools,
    Help,
    Settings,
    Exit,
    Back,
    Forward,
    Reload,
    Bookmark,
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
        row(MenuId::ZoomIn, "Zoom", "", false),
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

pub fn page_menu() -> Vec<Row> {
    vec![
        row(MenuId::Back, "Back", "Alt+Left", false),
        row(MenuId::Forward, "Forward", "Alt+Right", false),
        row(MenuId::Reload, "Reload", "Ctrl+R", false),
        sep(),
        row(MenuId::Bookmark, "Bookmark this page", "Ctrl+D", false),
        row(MenuId::Print, "Print", "Ctrl+P", false),
        row(MenuId::Translate, "Translate", "", false),
        sep(),
        row(MenuId::Find, "Find", "Ctrl+F", false),
        row(MenuId::Settings, "Settings", "", false),
    ]
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
        let blob = labels.join(" ").to_ascii_lowercase();
        assert!(!blob.contains("google"));
        assert!(!blob.contains("chrome"));
        assert!(!blob.contains("lens"));
        assert!(!blob.contains("incognito"));
    }
}
