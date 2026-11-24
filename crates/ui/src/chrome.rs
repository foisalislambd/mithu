//! Desktop frame painted as a pixel clone of the Chromium desktop browser
//! frame (the "Chrome Refresh 2023" / GM3 design), without another browser's
//! name or mark.
//!
//! Every metric and color below is lifted from the Chromium sources (BSD-3,
//! see THIRD_PARTY_NOTICES.md) and cited next to its use:
//!
//! - `chrome/browser/ui/layout_constants.cc` (heights, paddings, icon sizes)
//! - `chrome/browser/ui/tabs/tab_style.cc` and
//!   `chrome/browser/ui/views/tabs/common/horizontal_tab_style_views.cc`
//!   (tab width, overlap, corner radii, separators, the tab path itself)
//! - `ui/color/ref_color_mixer.cc`, `ui/color/sys_color_mixer.cc`,
//!   `chrome/browser/ui/color/{material_chrome,tab_strip,omnibox}_color_mixer.cc`
//!   (the GM3 baseline palette and the tokens each surface uses)
//!
//! Shapes are rasterized with tiny-skia (anti-aliased paths, the same
//! rasterizer family Chromium uses) and composited into the 0RGB softbuffer.
//! The Mithu mark only appears where the other browser shows its own product
//! mark: the omnibox chip on internal pages.

use mithu_text::{glyph_top, FontBook, Glyph};
use resvg::tiny_skia::{self as sk, FillRule, Mask, Path, PathBuilder, Transform};

use crate::icons::{self, Kind};
use crate::{BOOKMARK_BAR_H, BTN, CAPTION_W, TAB_H, TOOLBAR_H};

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (r << 16) | (g << 8) | b
}

// Layout constants (DIP). Names follow Chromium's LayoutConstant / TabStyle.

/// kTabStripPadding: gap above the tabs.
const TAB_STRIP_PADDING: f32 = 6.0;
/// TabStyle::GetTopCornerRadius.
const TAB_TOP_RADIUS: f32 = 10.0;
/// TabStyle::GetBottomCornerRadius == GetBottomExtensionWidth.
const TAB_EXTENSION: f32 = 12.0;
/// kTabWidth (232) + 2 * bottom extension.
const TAB_STANDARD_W: f32 = 232.0 + 2.0 * TAB_EXTENSION;
/// kSeparatorThickness / kSeparatorHeight / kSeparatorHorizontalMargin.
const SEPARATOR_W: f32 = 2.0;
const SEPARATOR_H: f32 = 16.0;
const SEPARATOR_MARGIN: f32 = 2.0;
/// TabStyle::GetTabOverlap = 2 * extension - (2 * margin + thickness).
const TAB_OVERLAP: f32 = 2.0 * TAB_EXTENSION - (2.0 * SEPARATOR_MARGIN + SEPARATOR_W);
/// kTabHorizontalPadding.
const TAB_H_PADDING: f32 = 8.0;
/// kTabPreTitlePadding / kTabAfterTitlePadding.
const TAB_PRE_TITLE: f32 = 8.0;
const TAB_AFTER_TITLE: f32 = 4.0;
/// gfx::kFaviconSize and kTabCloseButtonSize.
const FAVICON: f32 = 16.0;
const TAB_CLOSE: f32 = 16.0;
/// Height of the hover "squircle": kTabHeight - kTabStripPadding - overlap.
const TAB_HIGHLIGHT_H: f32 = 28.0;
/// Size of the new-tab and tab-search buttons in the strip.
const STRIP_BUTTON: f32 = 28.0;
/// TOOLBAR_INTERIOR_MARGIN (6, 6) and kToolbarIconDefaultMargin.
const TOOLBAR_MARGIN: f32 = 6.0;
const TOOLBAR_GAP: f32 = 2.0;
/// kLocationBarMargin.
const LOCATION_MARGIN: f32 = 9.0;
/// kLocationBarHeight.
const LOCATION_H: f32 = 34.0;
/// kToolbarButtonIconSize (refresh) and kLocationBarIconSize.
const TOOLBAR_ICON: f32 = 20.0;
const LOCATION_ICON: f32 = 16.0;
/// kLocationBarLeadingDecorationEdgePadding and the page-info icon padding.
const LEADING_EDGE: f32 = 5.0;
const PAGE_INFO_BUTTON: f32 = 24.0;
/// kLocationBarTrailingDecorationEdgePadding and the page-action button.
const TRAILING_EDGE: f32 = 12.0;
const PAGE_ACTION_BUTTON: f32 = 24.0;
/// kBookmarkBarButtonHeight, button image/label padding, max button width.
const BOOKMARK_BUTTON_H: f32 = 28.0;
const BOOKMARK_IMAGE_LABEL: f32 = 6.0;
const BOOKMARK_MAX_W: f32 = 150.0;
/// views::MenuConfig corner radius (refresh) and item height.
const MENU_RADIUS: f32 = 8.0;
const MENU_ROW: f32 = 32.0;
const MENU_SEP: f32 = 9.0;
/// Bubble / dialog corner radius (ChromeLayoutProvider, refresh).
const BUBBLE_RADIUS: f32 = 12.0;
/// Font sizes: the platform UI font is 12px; the omnibox is +2.
const UI_FONT: f32 = 12.0;
const OMNIBOX_FONT: f32 = 14.0;
const MENU_FONT: f32 = 13.0;

// Palette: the GM3 baseline tokens, light and dark.

struct Palette {
    /// kColorSysHeader: the frame / tab strip.
    frame: u32,
    /// kColorSysBase: toolbar, active tab, bookmark bar.
    toolbar: u32,
    /// kColorSysOmniboxContainer.
    omnibox: u32,
    /// kColorSysOnSurface: primary text.
    text: u32,
    /// kColorSysOnSurfaceSecondary/Subtle: toolbar icons, inactive titles, dim URL parts.
    text_secondary: u32,
    /// kColorSysStateDisabled over the toolbar.
    disabled: u32,
    /// kColorSysOnHeaderDivider: tab separators.
    tab_divider: u32,
    /// kColorTabBackgroundInactiveHoverFrameActive = 40% active over inactive.
    tab_hover: u32,
    /// kColorSysStateHoverOnSubtle (color, alpha) and the ripple.
    ink: (u32, u8),
    ink_pressed: (u32, u8),
    /// kColorToolbarContentAreaSeparator = kColorSysSurfaceVariant.
    content_separator: u32,
    /// Menus and bubbles.
    popup: u32,
    /// kColorSysDivider: menu separators.
    divider: u32,
    /// kColorSysPrimary.
    primary: u32,
    /// kColorSysInverseSurface / InverseOnSurface: tooltips.
    tooltip: u32,
    tooltip_text: u32,
    /// kColorSysTonalContainer.
    tonal: u32,
}

const LIGHT: Palette = Palette {
    frame: rgb(0xD3, 0xE3, 0xFD),          // ref primary90
    toolbar: rgb(0xFF, 0xFF, 0xFF),        // ref neutral100
    omnibox: rgb(0xED, 0xF2, 0xFA),        // surface4: #6991D6 @ 0x1E over #FFF
    text: rgb(0x1F, 0x1F, 0x1F),           // ref neutral10
    text_secondary: rgb(0x47, 0x47, 0x47), // ref neutral30
    disabled: rgb(0xA6, 0xA6, 0xA6),       // neutral10 @ 0x60 over neutral100
    tab_divider: rgb(0xA8, 0xC7, 0xFA),    // ref primary80
    tab_hover: rgb(0xE5, 0xEE, 0xFE),      // 40% #FFF over #D3E3FD
    ink: (rgb(0x1F, 0x1F, 0x1F), 0x0F),
    ink_pressed: (rgb(0x1F, 0x1F, 0x1F), 0x14),
    content_separator: rgb(0xE1, 0xE3, 0xE1), // ref neutral-variant90
    popup: rgb(0xFF, 0xFF, 0xFF),
    divider: rgb(0xD3, 0xE3, 0xFD),        // ref primary90
    primary: rgb(0x0B, 0x57, 0xD0),        // ref primary40
    tooltip: rgb(0x30, 0x30, 0x30),        // ref neutral20
    tooltip_text: rgb(0xF2, 0xF2, 0xF2),   // ref neutral95
    tonal: rgb(0xD3, 0xE3, 0xFD),
};

const DARK: Palette = Palette {
    frame: rgb(0x1F, 0x20, 0x20),          // ref neutral12
    toolbar: rgb(0x3C, 0x3C, 0x3C),        // ref neutral25
    omnibox: rgb(0x28, 0x28, 0x28),        // ref neutral15
    text: rgb(0xE3, 0xE3, 0xE3),           // ref neutral90
    text_secondary: rgb(0xC7, 0xC7, 0xC7), // ref neutral80
    disabled: rgb(0x8B, 0x8B, 0x8B),       // neutral90 @ 0x60 over neutral25
    tab_divider: rgb(0x3C, 0x3C, 0x3C),    // ref neutral25
    tab_hover: rgb(0x2B, 0x2C, 0x2C),      // 40% #3C3C3C over #1F2020
    ink: (rgb(0xFD, 0xFC, 0xFB), 0x1A),
    ink_pressed: (rgb(0xFD, 0xFC, 0xFB), 0x29),
    content_separator: rgb(0x44, 0x47, 0x46), // ref neutral-variant30
    popup: rgb(0x28, 0x28, 0x28),
    divider: rgb(0x5E, 0x5E, 0x5E),        // ref neutral40
    primary: rgb(0xA8, 0xC7, 0xFA),        // ref primary80
    tooltip: rgb(0xE3, 0xE3, 0xE3),        // ref neutral90
    tooltip_text: rgb(0x1F, 0x1F, 0x1F),   // ref neutral10
    tonal: rgb(0x00, 0x4A, 0x77),          // ref secondary30
};

/// Windows-style caption close hover.
const CAPTION_CLOSE_HOVER: u32 = rgb(0xC4, 0x2B, 0x1C);

/// Tab group colors (GM3 tab-group tokens, frame active, light).
const GROUP_COLOR: [u32; 8] = [
    rgb(0x0B, 0x57, 0xD0),
    rgb(0xB3, 0x26, 0x1E),
    rgb(0xA1, 0x6C, 0x00),
    rgb(0x14, 0x6C, 0x2E),
    rgb(0xB0, 0x1B, 0x7A),
    rgb(0x84, 0x30, 0xD4),
    rgb(0x00, 0x7B, 0x83),
    rgb(0xB0, 0x52, 0x00),
];

thread_local! {
    static PAINT_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static PAINT_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn begin_paint(state: &State<'_>) {
    PAINT_SCALE.set(unit(state.scale));
    PAINT_DARK.set(state.dark);
}

fn paint_scale() -> f32 {
    PAINT_SCALE.get()
}

fn dip(logical: f32) -> f32 {
    snap(logical, paint_scale())
}

fn pal() -> &'static Palette {
    if PAINT_DARK.get() { &DARK } else { &LIGHT }
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Secure {
    Https,
    Http,
    Internal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    App,
    Context,
}

pub struct TabPaint {
    pub title: String,
    pub favicon: Option<(u32, u32, Vec<u8>)>,
    /// 0 is ungrouped. 1–8 are the tab-group colors.
    pub group: u8,
    pub group_name: String,
    /// Hidden member of a collapsed group. The first tab of the group stays visible.
    pub collapsed: bool,
}

pub struct State<'a> {
    pub width: f32,
    pub height: f32,
    pub tabs: Vec<TabPaint>,
    pub active: usize,
    pub omnibox: String,
    pub omnibox_focused: bool,
    pub cursor: usize,
    pub secure: Secure,
    pub bookmarked: bool,
    pub bookmarks: Vec<(String, String)>,
    pub bookmarks_open: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub loading: bool,
    pub progress: f32,
    pub link_status: String,
    pub pointer: (f32, f32),
    pub pressed: bool,
    pub ntp: bool,
    pub menu: Option<Popup>,
    pub menu_labels: Vec<String>,
    pub submenu_at: Option<usize>,
    pub submenu_labels: Vec<String>,
    pub tab_search: bool,
    pub extensions: bool,
    pub profile: bool,
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    pub find_open: bool,
    pub find_query: String,
    pub fonts: &'a FontBook,
    /// Device pixels per logical pixel. 1.0, 1.25, 1.5, and 2.0 are the ones we snap.
    pub scale: f32,
    pub dark: bool,
    pub show_home: bool,
    pub group_editor: bool,
    pub permission_prompt: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Drag,
    Minimize,
    Maximize,
    Close,
    Tab(usize),
    CloseTab(usize),
    NewTab,
    TabSearch,
    Back,
    Forward,
    Reload,
    Home,
    Omnibox,
    Star,
    Extensions,
    Profile,
    Menu,
    Bookmark(usize),
    MenuItem(usize),
    SubmenuItem(usize),
    Zoom(i8),
    Suggestion(usize),
    Panel,
    Content,
    GroupBar(usize),
    GroupCollapse,
    Ungroup,
    CloseGroup,
    GroupColor(u8),
    AllowPermission,
    BlockPermission,
    None,
}

struct TabGeom {
    /// The tab view bounds, including both bottom extensions.
    view: Rect,
    /// Hit-test region: the tab body widened into half of each separator gap.
    rect: Rect,
    close: Rect,
    favicon: Rect,
    title_left: f32,
    title_right: f32,
}

struct Tools {
    back: Rect,
    forward: Rect,
    reload: Rect,
    home: Rect,
    address: Rect,
    star: Rect,
    extensions: Rect,
    profile: Rect,
    menu: Rect,
}

// Paint

pub fn paint(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    begin_paint(state);
    let p = pal();
    let w = state.width;
    let tab_h = dip(TAB_H);
    let bar = content_chrome(state);

    // Frame, toolbar + bookmark bar, and the 1px content-area separator.
    fill_rect(pixels, width, height, 0.0, 0.0, w, tab_h, p.frame);
    fill_rect(pixels, width, height, 0.0, tab_h, w, bar - tab_h, p.toolbar);
    fill_rect(pixels, width, height, 0.0, bar - 1.0, w, 1.0, p.content_separator);

    paint_tab_strip(pixels, width, height, state);
    paint_captions(pixels, width, height, state);

    let tools = toolbar(w, state.scale, state.show_home);
    tool_icon(pixels, width, height, state, tools.back, Kind::Back, state.can_back);
    tool_icon(pixels, width, height, state, tools.forward, Kind::Forward, state.can_forward);
    let reload_kind = if state.loading { Kind::Stop } else { Kind::Reload };
    tool_icon(pixels, width, height, state, tools.reload, reload_kind, true);
    if state.show_home && tools.home.w > 0.0 {
        tool_icon(pixels, width, height, state, tools.home, Kind::Home, true);
    }

    let popup_open = state.omnibox_focused && !state.suggestions.is_empty();
    if popup_open {
        suggestions_card(pixels, width, height, state, tools.address);
    }
    paint_omnibox(pixels, width, height, state, &tools, popup_open);

    tool_icon(pixels, width, height, state, tools.extensions, Kind::Extensions, true);
    tool_icon(pixels, width, height, state, tools.profile, Kind::Avatar, true);
    tool_icon(pixels, width, height, state, tools.menu, Kind::Menu, true);

    if state.bookmarks_open {
        paint_bookmarks(pixels, width, height, state);
    }
    if state.ntp {
        paint_ntp(pixels, width, height, state);
    }
    if state.find_open {
        paint_find(pixels, width, height, state);
    }
    if popup_open {
        paint_suggestions(pixels, width, height, state, tools.address);
    }
    if state.tab_search {
        let (_, _, search) = tab_layout(state.width, &state.tabs, state.scale);
        paint_tab_search(pixels, width, height, state, search);
    }
    if state.extensions {
        paint_extensions(pixels, width, height, state);
    }
    if state.profile {
        paint_profile_card(pixels, width, height, state);
    }
    if let Some(popup) = state.menu {
        paint_menu(pixels, width, height, state, popup);
    }
    paint_status(pixels, width, height, state);
    if state.group_editor {
        paint_group_editor(pixels, width, height, state);
    }
    if !state.permission_prompt.is_empty() {
        paint_permission(pixels, width, height, state);
    }
    paint_tooltips(pixels, width, height, state, &tools);
}

fn paint_tab_strip(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let (tabs, plus, search) = tab_layout(state.width, &state.tabs, state.scale);
    let hovered_tab = tab_under(state.active, state.pointer, &tabs);

    // Tab search sits at the leading edge of the strip (Windows/Linux layout).
    strip_button(pixels, width, height, state, search, Kind::TabSearch, dip(TOOLBAR_ICON));

    // Inactive tab highlights and separators first, then the active tab on top.
    let visible: Vec<usize> = (0..tabs.len()).filter(|index| tabs[*index].rect.w > 0.0).collect();
    for (slot, &index) in visible.iter().enumerate() {
        let geom = &tabs[index];
        let active = index == state.active;
        let hovered = hovered_tab == Some(index);
        if !active && hovered {
            let r = highlight_rect(geom.view);
            let radius = dip(TAB_TOP_RADIUS).min(r.w / 2.0);
            fill_round(pixels, width, height, r.x, r.y, r.w, r.h, radius, p.tab_hover);
        }
        // Trailing separator, hidden next to the active or hovered tab.
        let next = visible.get(slot + 1).copied();
        let next_lit = next.is_some_and(|n| n == state.active || hovered_tab == Some(n));
        if !active && !hovered && !next_lit {
            let x = geom.view.right() - dip(TAB_EXTENSION) + dip(SEPARATOR_MARGIN);
            let sh = dip(SEPARATOR_H);
            let y = (content_center_y() - sh / 2.0).round();
            fill_round(pixels, width, height, x, y, dip(SEPARATOR_W), sh, dip(1.0), p.tab_divider);
        }
    }
    if let Some(geom) = tabs.get(state.active).filter(|geom| geom.rect.w > 0.0) {
        if let Some(path) = active_tab_path(geom.view) {
            fill_path(pixels, width, height, &path, p.toolbar, 255);
        }
    }
    for &index in &visible {
        paint_tab_contents(pixels, width, height, state, &tabs[index], index, hovered_tab == Some(index));
    }

    strip_button(pixels, width, height, state, plus, Kind::Plus, dip(FAVICON));
}

/// The New Tab page carries no favicon in Chrome's tab strip.
fn is_new_tab_page(tab: &TabPaint) -> bool {
    tab.favicon.is_none() && tab.title == "New Tab"
}

fn tab_under(active: usize, (x, y): (f32, f32), tabs: &[TabGeom]) -> Option<usize> {
    if tabs.get(active).is_some_and(|geom| geom.rect.w > 0.0 && geom.rect.contains(x, y)) {
        return Some(active);
    }
    tabs.iter().position(|geom| geom.rect.w > 0.0 && geom.rect.contains(x, y))
}

fn paint_tab_contents(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, geom: &TabGeom, index: usize, hovered: bool) {
    let p = pal();
    let active = index == state.active;
    let tab = state.tabs.get(index);
    let fav = geom.favicon;
    let new_tab_page = tab.is_some_and(is_new_tab_page);
    if active && state.loading && !new_tab_page {
        paint_throbber(pixels, width, height, fav, p.primary);
    } else if let Some((fw, fh, bytes)) = tab.and_then(|tab| tab.favicon.as_ref()) {
        blit_favicon(pixels, width, height, fav.x, fav.y, fav.w, *fw, *fh, bytes);
    } else if !new_tab_page {
        // The default favicon is Chromium's globe.
        icons::blit(pixels, width, height, fav.x, fav.y, fav.w, Kind::Globe, p.text_secondary);
    }

    if let Some(group) = tab.map(|tab| tab.group).filter(|group| *group > 0) {
        let color = GROUP_COLOR[(group as usize - 1) % GROUP_COLOR.len()];
        let body_l = geom.view.x + dip(TAB_EXTENSION);
        let body_r = geom.view.right() - dip(TAB_EXTENSION);
        let y = dip(TAB_H) - dip(3.0);
        fill_round(pixels, width, height, body_l + dip(4.0), y, (body_r - body_l - dip(8.0)).max(dip(4.0)), dip(3.0), dip(1.5), color);
    }

    let show_close = geom.close.w > 0.0 && (active || hovered || geom.view.w >= dip(120.0));
    let title_right = if show_close { geom.title_right } else { geom.view.right() - dip(TAB_EXTENSION + TAB_H_PADDING) };
    if title_right - geom.title_left > dip(8.0) {
        let title = tab.map(|tab| tab.title.as_str()).filter(|title| !title.is_empty()).unwrap_or("New Tab");
        let baseline = text_baseline(content_center_y(), UI_FONT);
        let mut title_left = geom.title_left;
        if let Some(tab) = tab.filter(|tab| tab.group > 0 && !tab.group_name.is_empty() && group_head(&state.tabs, index)) {
            let color = GROUP_COLOR[(tab.group as usize - 1) % GROUP_COLOR.len()];
            let name_w = text_width(state.fonts, &tab.group_name, UI_FONT);
            let chip_w = (name_w + dip(12.0)).min(title_right - title_left);
            let chip_h = dip(18.0);
            fill_round(pixels, width, height, title_left, (content_center_y() - chip_h / 2.0).round(), chip_w, chip_h, dip(6.0), color);
            draw_text_fade(pixels, width, height, state.fonts, title_left + dip(6.0), baseline, UI_FONT, rgb(255, 255, 255), &tab.group_name, title_left, title_left + chip_w - dip(4.0), dip(8.0));
            title_left = (title_left + chip_w + dip(6.0)).min(title_right);
        }
        let color = if active { p.text } else { p.text_secondary };
        draw_text_fade(pixels, width, height, state.fonts, title_left, baseline, UI_FONT, color, title, title_left, title_right, dip(24.0));
    }

    if show_close {
        let over = geom.close.contains(state.pointer.0, state.pointer.1);
        if over {
            let (cx, cy) = geom.close.center();
            let (ink, alpha) = if state.pressed { p.ink_pressed } else { p.ink };
            fill_circle_alpha(pixels, width, height, cx, cy, dip(10.0), ink, alpha.saturating_mul(2));
        }
        icons::blit(pixels, width, height, geom.close.x, geom.close.y, geom.close.w, Kind::CloseTab, if active { p.text } else { p.text_secondary });
    }
}

fn paint_throbber(pixels: &mut [u32], width: u32, height: u32, rect: Rect, color: u32) {
    // Chromium's throbber: a 2 DIP arc that grows and shrinks while it spins.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let t = (millis % 1333) as f32 / 1333.0;
    let start = t * std::f32::consts::TAU * 1.5;
    let sweep = std::f32::consts::PI * (0.35 + 0.9 * (t * std::f32::consts::PI).sin());
    let (cx, cy) = rect.center();
    let stroke = dip(2.0);
    let radius = rect.w / 2.0 - stroke / 2.0;
    let mut builder = PathBuilder::new();
    let steps = 24;
    for step in 0..=steps {
        let a = start + sweep * step as f32 / steps as f32;
        let (x, y) = (cx + radius * a.cos(), cy + radius * a.sin());
        if step == 0 { builder.move_to(x, y) } else { builder.line_to(x, y) }
    }
    stroke_path(pixels, width, height, builder, stroke, color);
}

fn stroke_path(pixels: &mut [u32], width: u32, height: u32, builder: PathBuilder, thickness: f32, color: u32) {
    if let Some(path) = builder.finish() {
        let stroke = sk::Stroke { width: thickness, line_cap: sk::LineCap::Round, line_join: sk::LineJoin::Round, ..sk::Stroke::default() };
        if let Some(outline) = path.stroke(&stroke, 1.0) {
            fill_path(pixels, width, height, &outline, color, 255);
        }
    }
}

fn strip_button(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect, kind: Kind, icon: f32) {
    let p = pal();
    if rect.contains(state.pointer.0, state.pointer.1) {
        let (ink, alpha) = if state.pressed { p.ink_pressed } else { p.ink };
        let (cx, cy) = rect.center();
        fill_circle_alpha(pixels, width, height, cx, cy, rect.w / 2.0, ink, alpha);
    }
    centered_icon(pixels, width, height, rect, kind, icon, p.text);
}

fn paint_omnibox(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, tools: &Tools, popup_open: bool) {
    let p = pal();
    let a = tools.address;
    let radius = a.h / 2.0;
    let hovered = a.contains(state.pointer.0, state.pointer.1) && !state.omnibox_focused;
    if !popup_open {
        if state.omnibox_focused {
            fill_round(pixels, width, height, a.x, a.y, a.w, a.h, radius, p.toolbar);
            stroke_round(pixels, width, height, a.x, a.y, a.w, a.h, radius, dip(2.0), p.primary);
        } else {
            let bg = if hovered { mix(p.ink.0, p.omnibox, p.ink.1) } else { p.omnibox };
            fill_round(pixels, width, height, a.x, a.y, a.w, a.h, radius, bg);
        }
    }

    let cy = a.y + a.h / 2.0;
    let icon = dip(LOCATION_ICON);
    let chip_x = a.x + dip(LEADING_EDGE);
    let chip_h = dip(PAGE_INFO_BUTTON);
    let chip_y = (cy - chip_h / 2.0).round();
    let shown = omnibox_text(&state.omnibox);
    let empty = shown.is_empty() && !state.omnibox_focused;
    let text_left;
    if state.omnibox_focused || empty {
        // Focused or empty: the leading decoration is the search glyph.
        icons::blit(pixels, width, height, (chip_x + (chip_h - icon) / 2.0).round(), (cy - icon / 2.0).round(), icon, Kind::Search, p.text_secondary);
        text_left = chip_x + chip_h + dip(6.0);
    } else {
        let (kind, label): (Option<Kind>, &str) = match state.secure {
            Secure::Https => (Some(Kind::PageInfo), ""),
            Secure::Http => (Some(Kind::NotSecure), "Not secure"),
            Secure::Internal => (None, "Mithu"),
        };
        let label_w = if label.is_empty() { 0.0 } else { text_width(state.fonts, label, MENU_FONT) };
        let chip_w = if label.is_empty() { chip_h } else { (dip(6.0) + icon + dip(4.0) + label_w + dip(8.0)).round() };
        let chip = Rect { x: chip_x, y: chip_y, w: chip_w, h: chip_h };
        if chip.contains(state.pointer.0, state.pointer.1) {
            fill_round_alpha(pixels, width, height, chip.x, chip.y, chip.w, chip.h, chip_h / 2.0, p.ink.0, p.ink.1.saturating_mul(2));
        }
        let icon_x = if label.is_empty() { (chip_x + (chip_h - icon) / 2.0).round() } else { chip_x + dip(6.0) };
        match kind {
            Some(kind) => icons::blit(pixels, width, height, icon_x, (cy - icon / 2.0).round(), icon, kind, p.text_secondary),
            None => crate::brand::blit(pixels, width, height, icon_x, (cy - icon / 2.0).round(), icon, icon, crate::brand::mark()),
        }
        if label.is_empty() {
            text_left = chip.right() + dip(6.0);
        } else {
            let color = if state.secure == Secure::Http { p.text_secondary } else { p.text };
            draw_text(pixels, width, height, state.fonts, icon_x + icon + dip(4.0), text_baseline(cy, MENU_FONT), MENU_FONT, color, label);
            // kLocationBarIconLabelBubbleSeparatorWidth with 8 DIP beside it.
            let sep_x = chip.right() + dip(2.0);
            fill_rect(pixels, width, height, sep_x, (cy - dip(8.0)).round(), dip(1.0).max(1.0), dip(16.0), p.tab_divider);
            text_left = sep_x + dip(9.0);
        }
    }

    let text_right = tools.star.x - dip(4.0);
    let baseline = text_baseline(cy, OMNIBOX_FONT);
    if empty {
        let placeholder = format!("Search {} or type a URL", crate::search_engine_label());
        draw_text_fade(pixels, width, height, state.fonts, text_left, baseline, OMNIBOX_FONT, p.text_secondary, &placeholder, text_left, text_right, dip(18.0));
    } else if state.omnibox_focused {
        draw_text_fade(pixels, width, height, state.fonts, text_left, baseline, OMNIBOX_FONT, p.text, shown, text_left, text_right, dip(18.0));
        let caret = text_left + text_width(state.fonts, &shown.chars().take(state.cursor).collect::<String>(), OMNIBOX_FONT);
        if caret < text_right {
            fill_rect(pixels, width, height, caret.round(), (cy - dip(9.0)).round(), dip(1.0).max(1.0), dip(18.0), p.text);
        }
    } else {
        // Elided display: no scheme, the host emphasized, the rest dimmed.
        let (host, rest) = split_display_url(shown);
        draw_text_fade(pixels, width, height, state.fonts, text_left, baseline, OMNIBOX_FONT, p.text, host, text_left, text_right, dip(18.0));
        let host_w = text_width(state.fonts, host, OMNIBOX_FONT);
        if !rest.is_empty() && text_left + host_w < text_right {
            draw_text_fade(pixels, width, height, state.fonts, text_left + host_w, baseline, OMNIBOX_FONT, p.text_secondary, rest, text_left, text_right, dip(18.0));
        }
    }

    // Bookmark star.
    let star = tools.star;
    if star.contains(state.pointer.0, state.pointer.1) {
        let (cx, cy) = star.center();
        let (ink, alpha) = if state.pressed { p.ink_pressed } else { p.ink };
        fill_circle_alpha(pixels, width, height, cx, cy, star.w / 2.0, ink, alpha);
    }
    let (kind, color) = if state.bookmarked { (Kind::StarFilled, p.primary) } else { (Kind::Star, p.text_secondary) };
    centered_icon(pixels, width, height, star, kind, dip(TOOLBAR_ICON), color);
}

/// Split an elided URL into the emphasized host and the dimmed remainder.
fn split_display_url(url: &str) -> (&str, &str) {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        return (url, "");
    };
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.find('/') {
        Some(at) if &rest[at..] == "/" => (&rest[..at], ""),
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    }
}

fn content_center_y() -> f32 {
    // Tab contents are centered in the hover squircle (kTabStripPadding + 28/2).
    dip(TAB_STRIP_PADDING + TAB_HIGHLIGHT_H / 2.0)
}

/// Baseline that centers a line of `size` px text on `center_y`.
fn text_baseline(center_y: f32, size: f32) -> f32 {
    // Center the cap height (0.714 em for Noto Sans), which is where
    // views::Label's vertical centering lands Latin UI text.
    (center_y + snap(size, paint_scale()) * 0.714 / 2.0).round()
}

fn highlight_rect(view: Rect) -> Rect {
    Rect {
        x: view.x + dip(TAB_EXTENSION),
        y: dip(TAB_STRIP_PADDING),
        w: (view.w - 2.0 * dip(TAB_EXTENSION)).max(1.0),
        h: dip(TAB_HIGHLIGHT_H),
    }
}

/// HorizontalTabStyleViews::GetPath(kFill) for the active tab: a body with
/// 10 DIP top corners and 12 DIP concave flares into the toolbar.
fn active_tab_path(view: Rect) -> Option<Path> {
    const K: f32 = 0.552_284_8;
    let left = view.x;
    let right = view.right();
    let ext = dip(TAB_EXTENSION);
    let top = dip(TAB_STRIP_PADDING);
    let tab_bottom = dip(TAB_H);
    let extended_bottom = tab_bottom + dip(1.0);
    let tab_left = left + ext;
    let tab_right = right - ext;
    let rc = dip(TAB_TOP_RADIUS).min((tab_right - tab_left) / 2.0).max(0.0);
    let re = ext;
    let mut b = PathBuilder::new();
    b.move_to(left, extended_bottom);
    b.line_to(left, tab_bottom);
    // Bottom-left flare (concave), then the ascender and top-left corner.
    let (cx, cy) = (tab_left - re, tab_bottom - re);
    b.cubic_to(cx + K * re, cy + re, cx + re, cy + K * re, cx + re, cy);
    b.line_to(tab_left, top + rc);
    let (cx, cy) = (tab_left + rc, top + rc);
    b.cubic_to(cx - rc, cy - K * rc, cx - K * rc, cy - rc, cx, cy - rc);
    b.line_to(tab_right - rc, top);
    let (cx, cy) = (tab_right - rc, top + rc);
    b.cubic_to(cx + K * rc, cy - rc, cx + rc, cy - K * rc, cx + rc, cy);
    // Descender and the bottom-right flare.
    b.line_to(tab_right, tab_bottom - re);
    let (cx, cy) = (tab_right + re, tab_bottom - re);
    b.cubic_to(cx - re, cy + K * re, cx - K * re, cy + re, cx, cy + re);
    b.line_to(right, extended_bottom);
    b.close();
    b.finish()
}

// Hit testing

pub fn hit(state: &State<'_>, x: f32, y: f32) -> Hit {
    begin_paint(state);
    if !state.permission_prompt.is_empty() {
        let bar = permission_bar(state);
        if bar.contains(x, y) {
            if permission_button(bar, true).contains(x, y) {
                return Hit::AllowPermission;
            }
            if permission_button(bar, false).contains(x, y) {
                return Hit::BlockPermission;
            }
            return Hit::Panel;
        }
    }
    if state.group_editor {
        let card = group_card(state);
        if card.contains(x, y) {
            if group_button(card, 0).contains(x, y) {
                return Hit::GroupCollapse;
            }
            if group_button(card, 1).contains(x, y) {
                return Hit::Ungroup;
            }
            if group_button(card, 2).contains(x, y) {
                return Hit::CloseGroup;
            }
            for (index, dot) in group_dots(card).iter().enumerate() {
                if dot.contains(x, y) {
                    return Hit::GroupColor((index as u8) + 1);
                }
            }
            return Hit::Panel;
        }
    }
    let tools = toolbar(state.width, state.scale, state.show_home);
    if state.menu.is_some() {
        if let Some(hit) = menu_hit(state, x, y) {
            return hit;
        }
    }
    if state.tab_search {
        let (_, _, search) = tab_layout(state.width, &state.tabs, state.scale);
        let rect = tab_search_rect(state, search);
        if rect.contains(x, y) {
            let index = ((y - rect.y - dip(8.0)) / dip(MENU_ROW)).floor() as isize;
            if index >= 0 && (index as usize) < state.tabs.len() {
                return Hit::Tab(index as usize);
            }
            return Hit::Panel;
        }
    }
    if state.extensions && extensions_rect(state).contains(x, y) {
        return Hit::Panel;
    }
    if state.profile && profile_rect(state).contains(x, y) {
        return Hit::Panel;
    }
    if state.omnibox_focused && !state.suggestions.is_empty() {
        let rect = suggestion_rows_rect(state, tools.address);
        if rect.contains(x, y) {
            let index = ((y - rect.y) / dip(MENU_ROW)).floor() as isize;
            if index >= 0 && (index as usize) < state.suggestions.len() {
                return Hit::Suggestion(index as usize);
            }
            return Hit::Panel;
        }
    }
    let tab_h = dip(TAB_H);
    let tool_h = dip(TOOLBAR_H);
    let book_h = dip(BOOKMARK_BAR_H);
    if y < tab_h {
        if let Some(caption) = caption_at(state.width, x, state.scale) {
            return caption;
        }
        let (tabs, plus, search) = tab_layout(state.width, &state.tabs, state.scale);
        if plus.contains(x, y) {
            return Hit::NewTab;
        }
        if search.contains(x, y) {
            return Hit::TabSearch;
        }
        let Some(index) = tab_under(state.active, (x, y), &tabs) else {
            return Hit::Drag;
        };
        let geom = &tabs[index];
        if geom.close.w > 0.0 && geom.close.contains(x, y) {
            return Hit::CloseTab(index);
        }
        if state.tabs.get(index).is_some_and(|tab| tab.group > 0) && y >= geom.rect.bottom() - dip(6.0) {
            return Hit::GroupBar(index);
        }
        return Hit::Tab(index);
    }
    if y < tab_h + tool_h {
        if tools.back.contains(x, y) && state.can_back {
            return Hit::Back;
        }
        if tools.forward.contains(x, y) && state.can_forward {
            return Hit::Forward;
        }
        if tools.reload.contains(x, y) {
            return Hit::Reload;
        }
        if state.show_home && tools.home.w > 0.0 && tools.home.contains(x, y) {
            return Hit::Home;
        }
        if tools.star.contains(x, y) {
            return Hit::Star;
        }
        if tools.extensions.contains(x, y) {
            return Hit::Extensions;
        }
        if tools.profile.contains(x, y) {
            return Hit::Profile;
        }
        if tools.menu.contains(x, y) {
            return Hit::Menu;
        }
        if tools.address.contains(x, y) {
            return Hit::Omnibox;
        }
        return Hit::None;
    }
    if state.bookmarks_open && y < tab_h + tool_h + book_h {
        for (index, rect) in bookmark_rects(state).into_iter().enumerate() {
            if rect.contains(x, y) {
                return Hit::Bookmark(index);
            }
        }
        return Hit::None;
    }
    if y >= content_chrome(state) {
        Hit::Content
    } else {
        Hit::None
    }
}

/// A blank state that keeps the geometry inputs of `state`.
#[cfg(test)]
fn shallow<'a>(state: &State<'a>) -> State<'a> {
    State {
        width: state.width,
        height: state.height,
        tabs: Vec::new(),
        active: state.active,
        omnibox: String::new(),
        omnibox_focused: false,
        cursor: 0,
        secure: state.secure,
        bookmarked: false,
        bookmarks: Vec::new(),
        bookmarks_open: state.bookmarks_open,
        can_back: false,
        can_forward: false,
        loading: false,
        progress: 0.0,
        link_status: String::new(),
        pointer: state.pointer,
        pressed: false,
        ntp: false,
        menu: None,
        menu_labels: Vec::new(),
        submenu_at: None,
        submenu_labels: Vec::new(),
        tab_search: false,
        extensions: false,
        profile: false,
        suggestions: Vec::new(),
        suggestion: None,
        find_open: false,
        find_query: String::new(),
        fonts: state.fonts,
        scale: state.scale,
        dark: state.dark,
        show_home: state.show_home,
        group_editor: false,
        permission_prompt: "",
    }
}

pub fn chrome_height(bookmarks_open: bool, scale: f32) -> f32 {
    let scale = unit(scale);
    let mut top = snap(TAB_H, scale) + snap(TOOLBAR_H, scale);
    if bookmarks_open {
        top += snap(BOOKMARK_BAR_H, scale);
    }
    top
}

pub fn content_chrome(state: &State<'_>) -> f32 {
    chrome_height(state.bookmarks_open, state.scale)
}

fn unit(scale: f32) -> f32 {
    if scale.is_finite() && scale >= 1.0 { scale } else { 1.0 }
}

fn snap(logical: f32, scale: f32) -> f32 {
    (logical * unit(scale)).round()
}

fn box_px(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Rect {
    let x0 = snap(x, scale);
    let y0 = snap(y, scale);
    let x1 = snap(x + w, scale).max(x0 + 1.0);
    let y1 = snap(y + h, scale).max(y0 + 1.0);
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

fn group_head(tabs: &[TabPaint], index: usize) -> bool {
    let Some(tab) = tabs.get(index) else {
        return false;
    };
    if tab.group == 0 {
        return false;
    }
    tabs.iter().take(index).all(|other| other.group != tab.group || other.collapsed)
}

/// Leading x of the first tab view, after the tab-search button.
const TABS_LEFT: f32 = 8.0 + STRIP_BUTTON + 6.0 - TAB_EXTENSION;
/// Space kept after the new-tab button: a drag handle plus the captions.
const STRIP_TRAILING: f32 = 8.0 + STRIP_BUTTON + 24.0 + CAPTION_W * 3.0;

fn tab_layout(width_px: f32, tabs_in: &[TabPaint], scale: f32) -> (Vec<TabGeom>, Rect, Rect) {
    let scale = unit(scale);
    let width = width_px / scale;
    let count = tabs_in.len().max(1);
    let shown = tabs_in.iter().filter(|tab| !tab.collapsed).count().max(1);
    let available = (width - TABS_LEFT - STRIP_TRAILING).max(TAB_OVERLAP + 40.0);
    // n * (w - overlap) + overlap must fit the available width.
    let ideal = (available - TAB_OVERLAP) / shown as f32 + TAB_OVERLAP;
    let tab_w = ideal.clamp(TAB_OVERLAP + 22.0, TAB_STANDARD_W);
    let mut tabs = Vec::with_capacity(count);
    let mut x = TABS_LEFT;
    let source: Vec<(bool, bool)> = if tabs_in.is_empty() {
        vec![(false, false)]
    } else {
        tabs_in.iter().map(|tab| (tab.collapsed, is_new_tab_page(tab))).collect()
    };
    let zero = |x: f32| Rect { x: snap(x, scale), y: 0.0, w: 0.0, h: 0.0 };
    let top = TAB_STRIP_PADDING;
    let center = TAB_STRIP_PADDING + TAB_HIGHLIGHT_H / 2.0;
    let half_gap = (2.0 * SEPARATOR_MARGIN + SEPARATOR_W) / 2.0;
    let mut last_body_right = x + TAB_EXTENSION;
    for (collapsed, no_icon) in source {
        if collapsed {
            tabs.push(TabGeom { view: zero(x), rect: zero(x), close: zero(x), favicon: zero(x), title_left: 0.0, title_right: 0.0 });
            continue;
        }
        let view = box_px(x, 0.0, tab_w, TAB_H, scale);
        let rect = box_px(x + TAB_EXTENSION - half_gap, top, tab_w - 2.0 * TAB_EXTENSION + 2.0 * half_gap, TAB_H - top, scale);
        let inner_l = x + TAB_EXTENSION + TAB_H_PADDING;
        let inner_r = x + tab_w - TAB_EXTENSION - TAB_H_PADDING;
        let narrow = inner_r - inner_l < FAVICON + TAB_PRE_TITLE + 16.0;
        let fav_x = if narrow { (inner_l + inner_r) / 2.0 - FAVICON / 2.0 } else { inner_l };
        let favicon = box_px(fav_x, center - FAVICON / 2.0, FAVICON, FAVICON, scale);
        let close = if inner_r - inner_l >= FAVICON + TAB_CLOSE + 4.0 {
            box_px(inner_r - TAB_CLOSE, center - TAB_CLOSE / 2.0, TAB_CLOSE, TAB_CLOSE, scale)
        } else {
            zero(inner_r)
        };
        // Chrome's New Tab page has no favicon, so its title starts at the icon slot.
        let title_left = if no_icon { snap(inner_l, scale) } else { snap(inner_l + FAVICON + TAB_PRE_TITLE, scale) };
        let title_right = if narrow {
            title_left
        } else if close.w > 0.0 {
            close.x - snap(TAB_AFTER_TITLE, scale)
        } else {
            snap(inner_r, scale)
        };
        tabs.push(TabGeom { view, rect, close, favicon, title_left, title_right });
        last_body_right = x + tab_w - TAB_EXTENSION;
        x += tab_w - TAB_OVERLAP;
    }
    let button_y = TAB_STRIP_PADDING + (TAB_HIGHLIGHT_H - STRIP_BUTTON) / 2.0;
    let plus = box_px(last_body_right + 8.0, button_y, STRIP_BUTTON, STRIP_BUTTON, scale);
    let search = box_px(8.0, button_y, STRIP_BUTTON, STRIP_BUTTON, scale);
    (tabs, plus, search)
}

fn toolbar(width_px: f32, scale: f32, show_home: bool) -> Tools {
    let scale = unit(scale);
    let width = width_px / scale;
    let y = TAB_H + (TOOLBAR_H - BTN) / 2.0;
    let mut x = TOOLBAR_MARGIN;
    let place = |x: &mut f32| {
        let rect = box_px(*x, y, BTN, BTN, scale);
        *x += BTN + TOOLBAR_GAP;
        rect
    };
    let back = place(&mut x);
    let forward = place(&mut x);
    let reload = place(&mut x);
    let home = if show_home { place(&mut x) } else { Rect { x: snap(x, scale), y: snap(y, scale), w: 0.0, h: 0.0 } };
    let left = x - TOOLBAR_GAP + LOCATION_MARGIN;
    let menu_x = width - TOOLBAR_MARGIN - BTN;
    let profile_x = menu_x - TOOLBAR_GAP - BTN;
    let extensions_x = profile_x - TOOLBAR_GAP - BTN;
    let menu = box_px(menu_x, y, BTN, BTN, scale);
    let profile = box_px(profile_x, y, BTN, BTN, scale);
    let extensions = box_px(extensions_x, y, BTN, BTN, scale);
    let address_w = (extensions_x - LOCATION_MARGIN - left).max(80.0);
    let ly = TAB_H + (TOOLBAR_H - LOCATION_H) / 2.0;
    let address = box_px(left, ly, address_w, LOCATION_H, scale);
    // Page-action button: 20 DIP icon, the trailing edge padding is measured to the icon.
    let star_size = PAGE_ACTION_BUTTON + 4.0;
    let star_x = left + address_w - TRAILING_EDGE - TOOLBAR_ICON - (star_size - TOOLBAR_ICON) / 2.0;
    let star = box_px(star_x, ly + (LOCATION_H - star_size) / 2.0, star_size, star_size, scale);
    Tools { back, forward, reload, home, address, star, extensions, profile, menu }
}

fn bookmark_rects(state: &State<'_>) -> Vec<Rect> {
    let y = dip(TAB_H) + dip(TOOLBAR_H);
    let mut x = dip(TOOLBAR_MARGIN + 2.0);
    let mut rects = Vec::new();
    for (title, _) in &state.bookmarks {
        let text = text_width(state.fonts, title, UI_FONT);
        let w = (dip(8.0) + dip(FAVICON) + dip(BOOKMARK_IMAGE_LABEL) + text + dip(8.0)).min(dip(BOOKMARK_MAX_W)).round();
        if x + w > state.width - dip(8.0) {
            break;
        }
        rects.push(Rect { x, y, w, h: dip(BOOKMARK_BUTTON_H) });
        x += w;
    }
    rects
}

fn caption_at(width: f32, x: f32, scale: f32) -> Option<Hit> {
    let caption = snap(CAPTION_W, scale);
    if x >= width - caption {
        Some(Hit::Close)
    } else if x >= width - caption * 2.0 {
        Some(Hit::Maximize)
    } else if x >= width - caption * 3.0 {
        Some(Hit::Minimize)
    } else {
        None
    }
}

fn paint_captions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let caption = dip(CAPTION_W);
    let cap_h = dip(TAB_STRIP_PADDING + TAB_HIGHLIGHT_H);
    let specs = [
        (state.width - caption * 3.0, Kind::Minimize, Hit::Minimize),
        (state.width - caption * 2.0, Kind::Maximize, Hit::Maximize),
        (state.width - caption, Kind::Close, Hit::Close),
    ];
    for (x, kind, which) in specs {
        let rect = Rect { x, y: 0.0, w: caption, h: cap_h };
        let over = rect.contains(state.pointer.0, state.pointer.1);
        let mut color = p.text;
        if over && which == Hit::Close {
            fill_rect(pixels, width, height, x, 0.0, caption, rect.h, CAPTION_CLOSE_HOVER);
            color = rgb(255, 255, 255);
        } else if over {
            fill_rect_alpha(pixels, width, height, x, 0.0, caption, rect.h, p.ink.0, p.ink.1.saturating_mul(2));
        }
        centered_icon(pixels, width, height, rect, kind, dip(16.0), color);
    }
}

fn centered_icon(pixels: &mut [u32], width: u32, height: u32, rect: Rect, kind: Kind, size: f32, color: u32) {
    let icon = size.max(1.0);
    icons::blit(pixels, width, height, (rect.x + (rect.w - icon) / 2.0).round(), (rect.y + (rect.h - icon) / 2.0).round(), icon, kind, color);
}

fn tool_icon(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect, kind: Kind, enabled: bool) {
    let p = pal();
    if enabled && rect.contains(state.pointer.0, state.pointer.1) {
        let (ink, alpha) = if state.pressed { p.ink_pressed } else { p.ink };
        fill_round_alpha(pixels, width, height, rect.x, rect.y, rect.w, rect.h, rect.h / 2.0, ink, alpha);
    }
    centered_icon(pixels, width, height, rect, kind, dip(TOOLBAR_ICON), if enabled { p.text_secondary } else { p.disabled });
}

fn paint_tooltips(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, tools: &Tools) {
    if state.menu.is_some() || state.pressed {
        return;
    }
    let (_, plus, search) = tab_layout(state.width, &state.tabs, state.scale);
    let reload_tip = if state.loading { "Stop loading this page" } else { "Reload this page" };
    let tips = [
        (plus, "New tab", true),
        (search, "Search tabs", true),
        (tools.back, "Click to go back, hold to see history", state.can_back),
        (tools.forward, "Click to go forward, hold to see history", state.can_forward),
        (tools.reload, reload_tip, true),
        (tools.home, "Open the home page", state.show_home && tools.home.w > 0.0),
        (tools.star, "Bookmark this tab", true),
        (tools.extensions, "Extensions", true),
        (tools.profile, "Profile", true),
        (tools.menu, "Customize and control Mithu", true),
    ];
    for (anchor, label, enabled) in tips {
        if enabled {
            tip(pixels, width, height, state, anchor, label);
        }
    }
}

fn paint_bookmarks(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let top = dip(TAB_H) + dip(TOOLBAR_H);
    if state.bookmarks.is_empty() {
        let baseline = text_baseline(top + dip(BOOKMARK_BUTTON_H) / 2.0, UI_FONT);
        draw_text(pixels, width, height, state.fonts, dip(16.0), baseline, UI_FONT, p.text_secondary, "For quick access, place your bookmarks here on the bookmarks bar.");
        return;
    }
    for ((title, _), rect) in state.bookmarks.iter().zip(bookmark_rects(state)) {
        if rect.contains(state.pointer.0, state.pointer.1) {
            let (ink, alpha) = if state.pressed { p.ink_pressed } else { p.ink };
            fill_round_alpha(pixels, width, height, rect.x, rect.y, rect.w, rect.h, rect.h / 2.0, ink, alpha);
        }
        let icon = dip(FAVICON);
        let cy = rect.y + rect.h / 2.0;
        icons::blit(pixels, width, height, rect.x + dip(8.0), (cy - icon / 2.0).round(), icon, Kind::Globe, p.text_secondary);
        let left = rect.x + dip(8.0) + icon + dip(BOOKMARK_IMAGE_LABEL);
        draw_text_fade(pixels, width, height, state.fonts, left, text_baseline(cy, UI_FONT), UI_FONT, p.text_secondary, title, left, rect.right() - dip(8.0), dip(12.0));
    }
}

fn paint_ntp(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let top = content_chrome(state);
    let bg = if state.dark { rgb(0x1F, 0x1F, 0x1F) } else { rgb(0xFF, 0xFF, 0xFF) };
    fill_rect(pixels, width, height, 0.0, top, state.width, state.height - top, bg);
    let size = dip(220.0).min(state.width * 0.46).max(1.0);
    let cx = state.width / 2.0;
    let y = top + dip(48.0);
    crate::brand::blit(pixels, width, height, cx - size / 2.0, y, size, size, crate::brand::logo());
}

fn find_rect(state: &State<'_>) -> Rect {
    let w = dip(372.0);
    Rect { x: state.width - w - dip(16.0), y: content_chrome(state) - dip(1.0), w, h: dip(48.0) }
}

fn paint_find(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let r = find_rect(state);
    elevated(pixels, width, height, r, dip(BUBBLE_RADIUS), p.popup);
    let cy = r.y + r.h / 2.0;
    let text = if state.find_query.is_empty() { "Find in page" } else { state.find_query.as_str() };
    let color = if state.find_query.is_empty() { p.text_secondary } else { p.text };
    let controls = r.right() - dip(12.0) - dip(28.0) * 3.0 - dip(8.0);
    draw_text_fade(pixels, width, height, state.fonts, r.x + dip(16.0), text_baseline(cy, MENU_FONT), MENU_FONT, color, text, r.x + dip(16.0), controls - dip(48.0), dip(12.0));
    if !state.find_query.is_empty() {
        let count = "0/0";
        let cw = text_width(state.fonts, count, UI_FONT);
        draw_text(pixels, width, height, state.fonts, controls - cw - dip(8.0), text_baseline(cy, UI_FONT), UI_FONT, p.text_secondary, count);
    }
    fill_rect(pixels, width, height, controls, (cy - dip(12.0)).round(), dip(1.0).max(1.0), dip(24.0), p.divider);
    let mut x = controls + dip(8.0);
    for glyph in 0..3 {
        let cell = Rect { x, y: (cy - dip(14.0)).round(), w: dip(28.0), h: dip(28.0) };
        if cell.contains(state.pointer.0, state.pointer.1) {
            let (cx, ccy) = cell.center();
            fill_circle_alpha(pixels, width, height, cx, ccy, cell.w / 2.0, p.ink.0, p.ink.1);
        }
        match glyph {
            0 => chevron(pixels, width, height, cell, Dir::Up, p.text_secondary),
            1 => chevron(pixels, width, height, cell, Dir::Down, p.text_secondary),
            _ => centered_icon(pixels, width, height, cell, Kind::CloseTab, dip(16.0), p.text_secondary),
        }
        x += dip(28.0);
    }
}

#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
    Right,
}

fn chevron(pixels: &mut [u32], width: u32, height: u32, cell: Rect, dir: Dir, color: u32) {
    let (cx, cy) = cell.center();
    let s = dip(4.0);
    let mut b = PathBuilder::new();
    match dir {
        Dir::Up => {
            b.move_to(cx - s, cy + s / 2.0);
            b.line_to(cx, cy - s / 2.0);
            b.line_to(cx + s, cy + s / 2.0);
        }
        Dir::Down => {
            b.move_to(cx - s, cy - s / 2.0);
            b.line_to(cx, cy + s / 2.0);
            b.line_to(cx + s, cy - s / 2.0);
        }
        Dir::Right => {
            b.move_to(cx - s / 2.0, cy - s);
            b.line_to(cx + s / 2.0, cy);
            b.line_to(cx - s / 2.0, cy + s);
        }
    }
    stroke_path(pixels, width, height, b, dip(1.5).max(1.0), color);
}

fn suggestion_rows_rect(state: &State<'_>, address: Rect) -> Rect {
    Rect { x: address.x, y: address.bottom() + dip(4.0), w: address.w, h: state.suggestions.len() as f32 * dip(MENU_ROW) }
}

fn suggestions_card(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, address: Rect) {
    let p = pal();
    let rows = suggestion_rows_rect(state, address);
    let card = Rect { x: address.x, y: address.y, w: address.w, h: rows.bottom() + dip(8.0) - address.y };
    elevated(pixels, width, height, card, address.h / 2.0, p.popup);
}

fn paint_suggestions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, address: Rect) {
    let p = pal();
    let rows = suggestion_rows_rect(state, address);
    fill_rect(pixels, width, height, address.x + dip(16.0), address.bottom(), address.w - dip(32.0), dip(1.0).max(1.0), p.divider);
    let row_h = dip(MENU_ROW);
    for (index, suggestion) in state.suggestions.iter().enumerate() {
        let row = Rect { x: rows.x, y: rows.y + index as f32 * row_h, w: rows.w, h: row_h };
        if state.suggestion == Some(index) || row.contains(state.pointer.0, state.pointer.1) {
            fill_rect_alpha(pixels, width, height, row.x, row.y, row.w, row.h, p.ink.0, p.ink.1);
        }
        let cy = row.y + row.h / 2.0;
        let icon = dip(LOCATION_ICON);
        let looks_url = suggestion.contains('.') && !suggestion.contains(' ');
        let kind = if looks_url { Kind::Globe } else { Kind::Search };
        icons::blit(pixels, width, height, (row.x + dip(LEADING_EDGE) + (dip(PAGE_INFO_BUTTON) - icon) / 2.0).round(), (cy - icon / 2.0).round(), icon, kind, p.text_secondary);
        let left = row.x + dip(LEADING_EDGE + PAGE_INFO_BUTTON + 6.0);
        draw_text_fade(pixels, width, height, state.fonts, left, text_baseline(cy, OMNIBOX_FONT), OMNIBOX_FONT, p.text, suggestion, left, row.right() - dip(16.0), dip(16.0));
    }
}

fn tab_search_rect(state: &State<'_>, anchor: Rect) -> Rect {
    let w = dip(320.0);
    let h = dip(16.0) + state.tabs.len().max(1) as f32 * dip(MENU_ROW);
    Rect { x: anchor.x.min(state.width - w - dip(8.0)).max(dip(8.0)), y: anchor.bottom() + dip(6.0), w, h }
}

fn paint_tab_search(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect) {
    let p = pal();
    let rect = tab_search_rect(state, anchor);
    let row_h = dip(MENU_ROW);
    elevated(pixels, width, height, rect, dip(BUBBLE_RADIUS), p.popup);
    for (index, tab) in state.tabs.iter().enumerate() {
        let row = Rect { x: rect.x, y: rect.y + dip(8.0) + index as f32 * row_h, w: rect.w, h: row_h };
        if row.contains(state.pointer.0, state.pointer.1) {
            fill_rect_alpha(pixels, width, height, row.x, row.y, row.w, row.h, p.ink.0, p.ink.1);
        }
        let cy = row.y + row.h / 2.0;
        let icon = dip(FAVICON);
        if let Some((fw, fh, bytes)) = tab.favicon.as_ref() {
            blit_favicon(pixels, width, height, row.x + dip(16.0), (cy - icon / 2.0).round(), icon, *fw, *fh, bytes);
        } else {
            icons::blit(pixels, width, height, row.x + dip(16.0), (cy - icon / 2.0).round(), icon, Kind::Globe, p.text_secondary);
        }
        if index == state.active {
            fill_round(pixels, width, height, row.x, row.y + dip(6.0), dip(3.0), row.h - dip(12.0), dip(1.5), p.primary);
        }
        let left = row.x + dip(16.0) + icon + dip(12.0);
        draw_text_fade(pixels, width, height, state.fonts, left, text_baseline(cy, MENU_FONT), MENU_FONT, p.text, &tab.title, left, row.right() - dip(16.0), dip(16.0));
    }
}

fn extensions_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width, state.scale, state.show_home);
    let w = dip(320.0);
    Rect { x: (tools.extensions.right() - w).min(state.width - w - dip(8.0)).max(dip(8.0)), y: tools.extensions.bottom() + dip(6.0), w, h: dip(100.0) }
}

fn profile_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width, state.scale, state.show_home);
    let w = dip(320.0);
    Rect { x: (tools.profile.right() - w).min(state.width - w - dip(8.0)).max(dip(8.0)), y: tools.profile.bottom() + dip(6.0), w, h: dip(196.0) }
}

fn paint_extensions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let rect = extensions_rect(state);
    elevated(pixels, width, height, rect, dip(BUBBLE_RADIUS), p.popup);
    draw_text(pixels, width, height, state.fonts, rect.x + dip(20.0), rect.y + dip(32.0), 15.0, p.text, "Extensions");
    draw_text(pixels, width, height, state.fonts, rect.x + dip(20.0), rect.y + dip(58.0), MENU_FONT, p.text_secondary, "No extensions installed");
    draw_text(pixels, width, height, state.fonts, rect.x + dip(20.0), rect.y + dip(82.0), MENU_FONT, p.primary, "Manage extensions");
}

fn paint_profile_card(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let rect = profile_rect(state);
    elevated(pixels, width, height, rect, dip(BUBBLE_RADIUS), p.popup);
    // Header: tonal band with the generic signed-out avatar.
    let header = Rect { x: rect.x + dip(8.0), y: rect.y + dip(8.0), w: rect.w - dip(16.0), h: dip(104.0) };
    fill_round(pixels, width, height, header.x, header.y, header.w, header.h, dip(8.0), mix(p.tonal, p.popup, 0x80));
    let disc = dip(56.0);
    let cx = header.x + header.w / 2.0;
    let cy = header.y + dip(12.0) + disc / 2.0;
    fill_circle(pixels, width, height, cx, cy, disc / 2.0, p.popup);
    icons::blit(pixels, width, height, (cx - dip(20.0)).round(), (cy - dip(20.0)).round(), dip(40.0), Kind::Avatar, p.text_secondary);
    let name = "Person 1";
    let nw = text_width(state.fonts, name, 14.0);
    draw_text(pixels, width, height, state.fonts, cx - nw / 2.0, header.y + dip(92.0), 14.0, p.text, name);
    let mut y = header.bottom() + dip(8.0);
    for (kind, label) in [(Kind::Settings, "Customize profile"), (Kind::Avatar, "Manage profiles")] {
        let row = Rect { x: rect.x, y, w: rect.w, h: dip(MENU_ROW) };
        if row.contains(state.pointer.0, state.pointer.1) {
            fill_rect_alpha(pixels, width, height, row.x, row.y, row.w, row.h, p.ink.0, p.ink.1);
        }
        let cy = row.y + row.h / 2.0;
        icons::blit(pixels, width, height, row.x + dip(20.0), (cy - dip(10.0)).round(), dip(20.0), kind, p.text_secondary);
        draw_text(pixels, width, height, state.fonts, row.x + dip(56.0), text_baseline(cy, MENU_FONT), MENU_FONT, p.text, label);
        y += dip(MENU_ROW);
    }
}

fn paint_menu(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, popup: Popup) {
    let p = pal();
    let rect = menu_rect(state, popup);
    elevated(pixels, width, height, rect, dip(MENU_RADIUS), p.popup);
    let mut y = rect.y + dip(8.0);
    for (index, label) in state.menu_labels.iter().enumerate() {
        let row_h = if label.is_empty() { dip(MENU_SEP) } else { dip(MENU_ROW) };
        let row = Rect { x: rect.x, y, w: rect.w, h: row_h };
        if label.is_empty() {
            fill_rect(pixels, width, height, rect.x, y + (row_h / 2.0).floor(), rect.w, dip(1.0).max(1.0), p.divider);
        } else {
            let hot = row.contains(state.pointer.0, state.pointer.1) || state.submenu_at == Some(index);
            if hot {
                fill_rect_alpha(pixels, width, height, row.x, row.y, row.w, row.h, p.ink.0, p.ink.1);
            }
            let (title, shortcut) = label.split_once('\t').unwrap_or((label.as_str(), ""));
            let submenu = title.ends_with('\u{203a}');
            let title = title.trim_end_matches('\u{203a}').trim_end();
            let cy = row.y + row.h / 2.0;
            let icon = dip(TOOLBAR_ICON);
            if let Some(kind) = menu_icon(title) {
                icons::blit(pixels, width, height, row.x + dip(16.0), (cy - icon / 2.0).round(), icon, kind, p.text_secondary);
            }
            draw_text(pixels, width, height, state.fonts, row.x + dip(52.0), text_baseline(cy, MENU_FONT), MENU_FONT, p.text, title);
            if title == "Zoom" {
                paint_zoom_controls(pixels, width, height, state, row, shortcut);
            } else if submenu {
                chevron(pixels, width, height, Rect { x: row.right() - dip(36.0), y: row.y, w: dip(20.0), h: row.h }, Dir::Right, p.text_secondary);
            } else if !shortcut.is_empty() {
                let sw = text_width(state.fonts, shortcut, UI_FONT);
                draw_text(pixels, width, height, state.fonts, row.right() - dip(16.0) - sw, text_baseline(cy, UI_FONT), UI_FONT, p.text_secondary, shortcut);
            }
        }
        y += row_h;
    }
    if let Some(anchor) = state.submenu_at {
        if !state.submenu_labels.is_empty() {
            paint_submenu(pixels, width, height, state, rect, anchor);
        }
    }
}

fn paint_zoom_controls(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, row: Rect, percent: &str) {
    let p = pal();
    let buttons = ["\u{2212}", percent, "+", "\u{26f6}"];
    let mut x = row.right() - dip(164.0);
    let cy = row.y + row.h / 2.0;
    for (index, label) in buttons.iter().enumerate() {
        let w = if index == 1 { dip(52.0) } else { dip(28.0) };
        let cell = Rect { x, y: row.y + dip(2.0), w, h: row.h - dip(4.0) };
        if index != 1 {
            let b = Rect { x: cell.x + dip(2.0), y: cell.y + dip(1.0), w: cell.w - dip(4.0), h: cell.h - dip(2.0) };
            stroke_round(pixels, width, height, b.x, b.y, b.w, b.h, dip(6.0), dip(1.0), p.divider);
            if cell.contains(state.pointer.0, state.pointer.1) {
                fill_round_alpha(pixels, width, height, b.x, b.y, b.w, b.h, dip(6.0), p.ink.0, p.ink.1);
            }
        }
        let tw = text_width(state.fonts, label, UI_FONT);
        draw_text(pixels, width, height, state.fonts, cell.x + (cell.w - tw) / 2.0, text_baseline(cy, UI_FONT), UI_FONT, p.text, label);
        x += w;
    }
}

fn paint_submenu(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, parent: Rect, anchor: usize) {
    let p = pal();
    let sub = submenu_rect(state, parent, anchor);
    elevated(pixels, width, height, sub, dip(MENU_RADIUS), p.popup);
    let mut y = sub.y + dip(8.0);
    for label in &state.submenu_labels {
        let row_h = if label.is_empty() { dip(MENU_SEP) } else { dip(MENU_ROW) };
        let row = Rect { x: sub.x, y, w: sub.w, h: row_h };
        if label.is_empty() {
            fill_rect(pixels, width, height, sub.x, y + (row_h / 2.0).floor(), sub.w, dip(1.0).max(1.0), p.divider);
        } else {
            if row.contains(state.pointer.0, state.pointer.1) {
                fill_rect_alpha(pixels, width, height, row.x, row.y, row.w, row.h, p.ink.0, p.ink.1);
            }
            let (title, shortcut) = label.split_once('\t').unwrap_or((label.as_str(), ""));
            let cy = row.y + row.h / 2.0;
            draw_text(pixels, width, height, state.fonts, row.x + dip(16.0), text_baseline(cy, MENU_FONT), MENU_FONT, p.text, title);
            if !shortcut.is_empty() {
                let sw = text_width(state.fonts, shortcut, UI_FONT);
                draw_text(pixels, width, height, state.fonts, row.right() - dip(16.0) - sw, text_baseline(cy, UI_FONT), UI_FONT, p.text_secondary, shortcut);
            }
        }
        y += row_h;
    }
}

fn submenu_rect(state: &State<'_>, parent: Rect, anchor: usize) -> Rect {
    let w = dip(280.0);
    let h = menu_block_height(&state.submenu_labels);
    let y = parent.y + row_offset(&state.menu_labels, anchor);
    let x = if parent.x > w + dip(8.0) { parent.x - w + dip(4.0) } else { parent.right() - dip(4.0) };
    Rect { x, y, w, h }
}

fn menu_block_height(labels: &[String]) -> f32 {
    let mut h = dip(16.0);
    if labels.is_empty() {
        return h + dip(MENU_ROW);
    }
    for label in labels {
        h += if label.is_empty() { dip(MENU_SEP) } else { dip(MENU_ROW) };
    }
    h
}

fn menu_hit(state: &State<'_>, x: f32, y: f32) -> Option<Hit> {
    let rect = menu_rect(state, state.menu?);
    if let Some(anchor) = state.submenu_at {
        if !state.submenu_labels.is_empty() {
            let sub = submenu_rect(state, rect, anchor);
            if sub.contains(x, y) {
                return menu_index_at(&state.submenu_labels, y - sub.y).map(Hit::SubmenuItem).or(Some(Hit::Panel));
            }
        }
    }
    if !rect.contains(x, y) {
        return None;
    }
    let index = menu_index_at(&state.menu_labels, y - rect.y)?;
    let label = state.menu_labels.get(index).map(String::as_str).unwrap_or("");
    let (title, _) = label.split_once('\t').unwrap_or((label, ""));
    if title == "Zoom" {
        let mut cursor = rect.right() - dip(164.0);
        let widths = [dip(28.0), dip(52.0), dip(28.0), dip(28.0)];
        let kinds = [-1, 0, 1, 2];
        for (width, kind) in widths.into_iter().zip(kinds) {
            if x >= cursor && x < cursor + width {
                return Some(Hit::Zoom(kind));
            }
            cursor += width;
        }
        return Some(Hit::Panel);
    }
    Some(Hit::MenuItem(index))
}

fn row_offset(labels: &[String], index: usize) -> f32 {
    let mut y = 0.0;
    for label in labels.iter().take(index) {
        y += if label.is_empty() { dip(MENU_SEP) } else { dip(MENU_ROW) };
    }
    y
}

fn menu_index_at(labels: &[String], dy: f32) -> Option<usize> {
    let mut y = dip(8.0);
    for (index, label) in labels.iter().enumerate() {
        let row_h = if label.is_empty() { dip(MENU_SEP) } else { dip(MENU_ROW) };
        if dy >= y && dy < y + row_h && !label.is_empty() {
            return Some(index);
        }
        y += row_h;
    }
    None
}

fn menu_icon(label: &str) -> Option<Kind> {
    Some(match label {
        "New tab" => Kind::NewTabPage,
        "History" => Kind::History,
        "Downloads" => Kind::Download,
        "Settings" => Kind::Settings,
        "Bookmarks" | "Bookmarks and lists" | "Bookmark this page" | "Bookmark this tab" => Kind::Star,
        "Extensions" => Kind::Extensions,
        "Find" | "Find\u{2026}" | "Find and edit" => Kind::Search,
        "Back" => Kind::Back,
        "Forward" => Kind::Forward,
        "Reload" => Kind::Reload,
        _ => return None,
    })
}

fn menu_rect(state: &State<'_>, popup: Popup) -> Rect {
    let w = dip(336.0);
    let h = menu_block_height(&state.menu_labels);
    match popup {
        Popup::App => {
            let tools = toolbar(state.width, state.scale, state.show_home);
            Rect { x: (tools.menu.right() - w).max(dip(8.0)), y: tools.menu.bottom() + dip(2.0), w, h }
        }
        Popup::Context => Rect {
            x: state.pointer.0.min(state.width - w - dip(8.0)).max(dip(8.0)),
            y: state.pointer.1.min(state.height - h - dip(8.0)).max(dip(TAB_H)),
            w,
            h,
        },
    }
}

fn paint_status(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let text = if !state.link_status.is_empty() {
        state.link_status.as_str()
    } else if state.loading {
        "Waiting for response\u{2026}"
    } else {
        return;
    };
    // StatusBubbleViews: bottom-left, only the top-right corner rounded.
    let text_w = text_width(state.fonts, text, UI_FONT).min(state.width / 3.0);
    let h = dip(26.0);
    let bubble = Rect { x: 0.0, y: state.height - h, w: (text_w + dip(16.0)).round(), h };
    shadow(pixels, width, height, bubble, dip(4.0));
    if let Some(path) = rrect4(bubble.x, bubble.y, bubble.w, bubble.h, 0.0, dip(4.0), 0.0, 0.0) {
        fill_path(pixels, width, height, &path, p.popup, 255);
    }
    let cy = bubble.y + bubble.h / 2.0;
    draw_text_fade(pixels, width, height, state.fonts, bubble.x + dip(8.0), text_baseline(cy, UI_FONT), UI_FONT, p.text_secondary, text, bubble.x + dip(8.0), bubble.right() - dip(4.0), dip(14.0));
}

fn tip(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect, label: &str) {
    if state.pressed || !anchor.contains(state.pointer.0, state.pointer.1) {
        return;
    }
    let p = pal();
    let text_w = text_width(state.fonts, label, UI_FONT);
    let bubble_w = (text_w + dip(16.0)).round();
    let bubble_h = dip(24.0);
    let x = (anchor.x + anchor.w / 2.0 - bubble_w / 2.0).clamp(dip(4.0), (state.width - bubble_w - dip(4.0)).max(dip(4.0))).round();
    let y = anchor.bottom() + dip(8.0);
    fill_round(pixels, width, height, x, y, bubble_w, bubble_h, dip(4.0), p.tooltip);
    draw_text(pixels, width, height, state.fonts, x + dip(8.0), text_baseline(y + bubble_h / 2.0, UI_FONT), UI_FONT, p.tooltip_text, label);
}

fn omnibox_text(url: &str) -> &str {
    match url {
        "" | "about:blank" | "about:newtab" | "about:mithu" | "about:home" => "",
        other => other,
    }
}

// Overlays: tab group editor and permission prompt.

fn group_card(state: &State<'_>) -> Rect {
    let below_prompt = if state.permission_prompt.is_empty() { 0.0 } else { dip(56.0) };
    Rect { x: dip(16.0), y: content_chrome(state) + dip(8.0) + below_prompt, w: dip(296.0), h: dip(140.0) }
}

fn group_button(card: Rect, index: u8) -> Rect {
    let w = dip(84.0);
    Rect { x: card.x + dip(12.0) + index as f32 * (w + dip(6.0)), y: card.y + card.h - dip(44.0), w, h: dip(32.0) }
}

fn group_dots(card: Rect) -> Vec<Rect> {
    (0..8)
        .map(|index| Rect { x: card.x + dip(16.0) + index as f32 * dip(33.0), y: card.y + dip(44.0), w: dip(24.0), h: dip(24.0) })
        .collect()
}

fn paint_group_editor(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let card = group_card(state);
    elevated(pixels, width, height, card, dip(BUBBLE_RADIUS), p.popup);
    draw_text(pixels, width, height, state.fonts, card.x + dip(16.0), card.y + dip(28.0), 14.0, p.text, "Tab group");
    for (index, dot) in group_dots(card).iter().enumerate() {
        let (cx, cy) = dot.center();
        fill_circle(pixels, width, height, cx, cy, dip(9.0), GROUP_COLOR[index]);
        if dot.contains(state.pointer.0, state.pointer.1) {
            stroke_circle(pixels, width, height, cx, cy, dip(12.0), dip(2.0), GROUP_COLOR[index]);
        }
    }
    for (index, label) in ["Collapse", "Ungroup", "Close group"].iter().enumerate() {
        let b = group_button(card, index as u8);
        stroke_round(pixels, width, height, b.x, b.y, b.w, b.h, b.h / 2.0, dip(1.0), p.divider);
        if b.contains(state.pointer.0, state.pointer.1) {
            fill_round_alpha(pixels, width, height, b.x, b.y, b.w, b.h, b.h / 2.0, p.ink.0, p.ink.1);
        }
        let tw = text_width(state.fonts, label, UI_FONT);
        draw_text(pixels, width, height, state.fonts, b.x + (b.w - tw) / 2.0, text_baseline(b.y + b.h / 2.0, UI_FONT), UI_FONT, p.primary, label);
    }
}

fn permission_bar(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width, state.scale, state.show_home);
    // Wide enough for the prompt, Block and Allow, like Chrome's permission bubble.
    let text = text_width(state.fonts, state.permission_prompt, MENU_FONT);
    let wanted = (dip(16.0) + text + dip(8.0) + dip(72.0) * 2.0 + dip(20.0) + dip(12.0)).ceil();
    let w = wanted.max(dip(420.0)).min(state.width - tools.address.x - dip(8.0)).max(dip(240.0));
    Rect { x: tools.address.x, y: content_chrome(state) + dip(4.0), w, h: dip(48.0) }
}

fn permission_button(bar: Rect, allow: bool) -> Rect {
    let w = dip(72.0);
    Rect {
        x: if allow { bar.right() - dip(12.0) - w } else { bar.right() - dip(20.0) - w * 2.0 },
        y: bar.y + dip(8.0),
        w,
        h: dip(32.0),
    }
}

fn paint_permission(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let p = pal();
    let bar = permission_bar(state);
    elevated(pixels, width, height, bar, dip(BUBBLE_RADIUS), p.popup);
    let cy = bar.y + bar.h / 2.0;
    let right = permission_button(bar, false).x - dip(8.0);
    draw_text_fade(pixels, width, height, state.fonts, bar.x + dip(16.0), text_baseline(cy, MENU_FONT), MENU_FONT, p.text, state.permission_prompt, bar.x + dip(16.0), right, dip(16.0));
    let on_primary = if state.dark { rgb(0x06, 0x2E, 0x6F) } else { rgb(0xFF, 0xFF, 0xFF) };
    for (allow, label) in [(false, "Block"), (true, "Allow")] {
        let b = permission_button(bar, allow);
        if allow {
            fill_round(pixels, width, height, b.x, b.y, b.w, b.h, b.h / 2.0, p.primary);
        } else {
            stroke_round(pixels, width, height, b.x, b.y, b.w, b.h, b.h / 2.0, dip(1.0), p.divider);
        }
        if b.contains(state.pointer.0, state.pointer.1) {
            fill_round_alpha(pixels, width, height, b.x, b.y, b.w, b.h, b.h / 2.0, p.ink.0, p.ink.1);
        }
        let tw = text_width(state.fonts, label, MENU_FONT);
        let color = if allow { on_primary } else { p.primary };
        draw_text(pixels, width, height, state.fonts, b.x + (b.w - tw) / 2.0, text_baseline(b.y + b.h / 2.0, MENU_FONT), MENU_FONT, color, label);
    }
}

// Raster primitives.

/// Composite an anti-aliased path into the 0RGB buffer.
fn fill_path(pixels: &mut [u32], width: u32, height: u32, path: &Path, color: u32, alpha: u8) {
    if alpha == 0 {
        return;
    }
    let b = path.bounds();
    let x0 = b.left().floor().max(0.0) as i32;
    let y0 = b.top().floor().max(0.0) as i32;
    let x1 = (b.right().ceil() as i32).min(width as i32);
    let y1 = (b.bottom().ceil() as i32).min(height as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (mw, mh) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let Some(mut mask) = Mask::new(mw, mh) else { return };
    mask.fill_path(path, FillRule::Winding, true, Transform::from_translate(-x0 as f32, -y0 as f32));
    let data = mask.data();
    for row in 0..mh {
        for col in 0..mw {
            let coverage = data[(row * mw + col) as usize] as u32;
            if coverage == 0 {
                continue;
            }
            let a = coverage * alpha as u32 / 255;
            let (x, y) = (x0 + col as i32, y0 + row as i32);
            if a >= 255 {
                put(pixels, width, height, x, y, color);
            } else {
                blend(pixels, width, height, x, y, color, a as u8);
            }
        }
    }
}

fn rrect4(x: f32, y: f32, w: f32, h: f32, tl: f32, tr: f32, br: f32, bl: f32) -> Option<Path> {
    const K: f32 = 0.552_284_8;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let lim = (w / 2.0).min(h / 2.0);
    let (tl, tr, br, bl) = (tl.clamp(0.0, lim), tr.clamp(0.0, lim), br.clamp(0.0, lim), bl.clamp(0.0, lim));
    let (r, b) = (x + w, y + h);
    let mut p = PathBuilder::new();
    p.move_to(x + tl, y);
    p.line_to(r - tr, y);
    if tr > 0.0 {
        p.cubic_to(r - tr + K * tr, y, r, y + tr - K * tr, r, y + tr);
    }
    p.line_to(r, b - br);
    if br > 0.0 {
        p.cubic_to(r, b - br + K * br, r - br + K * br, b, r - br, b);
    }
    p.line_to(x + bl, b);
    if bl > 0.0 {
        p.cubic_to(x + bl - K * bl, b, x, b - bl + K * bl, x, b - bl);
    }
    p.line_to(x, y + tl);
    if tl > 0.0 {
        p.cubic_to(x, y + tl - K * tl, x + tl - K * tl, y, x + tl, y);
    }
    p.close();
    p.finish()
}

fn fill_round(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
    fill_round_alpha(pixels, width, height, x, y, w, h, radius, color, 255);
}

fn fill_round_alpha(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32, alpha: u8) {
    if let Some(path) = rrect4(x, y, w, h, radius, radius, radius, radius) {
        fill_path(pixels, width, height, &path, color, alpha);
    }
}

/// An inside stroke of `thickness` along a rounded rect.
fn stroke_round(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, thickness: f32, color: u32) {
    let t = thickness.max(1.0);
    let Some(outer) = rrect4(x, y, w, h, radius, radius, radius, radius) else { return };
    let r2 = (radius - t).max(0.0);
    let mut b = PathBuilder::new();
    b.push_path(&outer);
    if let Some(inner) = rrect4(x + t, y + t, w - 2.0 * t, h - 2.0 * t, r2, r2, r2, r2) {
        if let Some(reversed) = reverse_path(&inner) {
            b.push_path(&reversed);
        }
    }
    if let Some(ring) = b.finish() {
        fill_path(pixels, width, height, &ring, color, 255);
    }
}

/// tiny-skia has no Path::reverse; rebuild the contour backwards so the ring
/// fills with the winding rule.
fn reverse_path(path: &Path) -> Option<Path> {
    use sk::PathSegment;
    let mut segments: Vec<(sk::Point, PathSegment)> = Vec::new();
    let mut start = sk::Point::zero();
    let mut current = sk::Point::zero();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                start = p;
                current = p;
            }
            PathSegment::Close => {
                if current != start {
                    segments.push((current, PathSegment::LineTo(start)));
                    current = start;
                }
            }
            PathSegment::LineTo(p) | PathSegment::QuadTo(_, p) | PathSegment::CubicTo(_, _, p) => {
                segments.push((current, segment));
                current = p;
            }
        }
    }
    let first = segments.last().map(|(_, seg)| match seg {
        PathSegment::LineTo(p) | PathSegment::QuadTo(_, p) | PathSegment::CubicTo(_, _, p) | PathSegment::MoveTo(p) => *p,
        PathSegment::Close => start,
    })?;
    let mut b = PathBuilder::new();
    b.move_to(first.x, first.y);
    for (from, segment) in segments.into_iter().rev() {
        match segment {
            PathSegment::LineTo(_) => b.line_to(from.x, from.y),
            PathSegment::QuadTo(c, _) => b.quad_to(c.x, c.y, from.x, from.y),
            PathSegment::CubicTo(c1, c2, _) => b.cubic_to(c2.x, c2.y, c1.x, c1.y, from.x, from.y),
            _ => {}
        }
    }
    b.close();
    b.finish()
}

fn fill_circle(pixels: &mut [u32], width: u32, height: u32, cx: f32, cy: f32, radius: f32, color: u32) {
    fill_circle_alpha(pixels, width, height, cx, cy, radius, color, 255);
}

fn fill_circle_alpha(pixels: &mut [u32], width: u32, height: u32, cx: f32, cy: f32, radius: f32, color: u32, alpha: u8) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, radius.max(0.5)) {
        fill_path(pixels, width, height, &path, color, alpha);
    }
}

fn stroke_circle(pixels: &mut [u32], width: u32, height: u32, cx: f32, cy: f32, radius: f32, thickness: f32, color: u32) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, radius) {
        let stroke = sk::Stroke { width: thickness, ..sk::Stroke::default() };
        if let Some(outline) = path.stroke(&stroke, 1.0) {
            fill_path(pixels, width, height, &outline, color, 255);
        }
    }
}

/// A surface with the views elevation shadow (key + ambient) under it.
fn elevated(pixels: &mut [u32], width: u32, height: u32, rect: Rect, radius: f32, color: u32) {
    shadow(pixels, width, height, rect, radius);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, radius, color);
}

fn shadow(pixels: &mut [u32], width: u32, height: u32, rect: Rect, radius: f32) {
    let dark = PAINT_DARK.get();
    let steps = dip(8.0).max(4.0) as i32;
    let base = if dark { 26.0 } else { 10.0 };
    let y_off = dip(2.0);
    for i in (1..=steps).rev() {
        let spread = i as f32;
        let alpha = (base * (1.0 - spread / (steps as f32 + 1.0))).max(1.0) as u8;
        fill_round_alpha(
            pixels,
            width,
            height,
            rect.x - spread,
            rect.y - spread + y_off,
            rect.w + 2.0 * spread,
            rect.h + 2.0 * spread,
            radius + spread,
            0,
            alpha,
        );
    }
}

fn blit_favicon(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, dest: f32, fw: u32, fh: u32, bytes: &[u8]) {
    if fw == 0 || fh == 0 || bytes.len() < (fw * fh * 4) as usize {
        return;
    }
    let dest = dest.round().max(1.0) as u32;
    let left = x.round() as i32;
    let top = y.round() as i32;
    for row in 0..dest {
        for col in 0..dest {
            // Box-filter the source area that maps onto this destination pixel.
            let sx0 = col * fw / dest;
            let sx1 = ((col + 1) * fw / dest).max(sx0 + 1).min(fw);
            let sy0 = row * fh / dest;
            let sy1 = ((row + 1) * fh / dest).max(sy0 + 1).min(fh);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy * fw + sx) * 4) as usize;
                    let alpha = bytes[i + 3] as u32;
                    r += bytes[i] as u32 * alpha;
                    g += bytes[i + 1] as u32 * alpha;
                    b += bytes[i + 2] as u32 * alpha;
                    a += alpha;
                    n += 1;
                }
            }
            if a == 0 || n == 0 {
                continue;
            }
            let color = ((r / a) << 16) | ((g / a) << 8) | (b / a);
            blend(pixels, width, height, left + col as i32, top + row as i32, color, (a / n) as u8);
        }
    }
}

fn draw_text(pixels: &mut [u32], width: u32, height: u32, fonts: &FontBook, x: f32, baseline: f32, size: f32, color: u32, text: &str) {
    draw_text_fade(pixels, width, height, fonts, x, baseline, size, color, text, f32::MIN, f32::MAX, 0.0);
}

fn draw_text_fade(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    fonts: &FontBook,
    x: f32,
    baseline: f32,
    size: f32,
    color: u32,
    text: &str,
    clip_left: f32,
    clip_right: f32,
    fade: f32,
) {
    let size = snap(size, paint_scale()).max(1.0);
    let glyphs = fonts.shape(text, size, false, false, "");
    let total: f32 = glyphs.iter().map(|glyph| glyph.advance).sum();
    // Fade only when the text overflows, like gfx::RenderText's ELIDE_FADE.
    let fade = if x.round() + total > clip_right + 1.0 { fade } else { 0.0 };
    let lut = text_lut(color);
    let baseline = baseline.round();
    let mut pen = x.round();
    for glyph in glyphs {
        if pen >= clip_right {
            break;
        }
        blit_glyph(pixels, width, height, pen, baseline, &glyph, color, clip_left, clip_right, fade, lut);
        pen += glyph.advance;
    }
}

/// Coverage curve that gives grayscale glyphs the stem weight Skia's text
/// gamma and contrast give them; dark-on-light text gets the stronger boost.
fn text_lut(color: u32) -> &'static [u8; 256] {
    use std::sync::OnceLock;
    static DARK_TEXT: OnceLock<[u8; 256]> = OnceLock::new();
    static LIGHT_TEXT: OnceLock<[u8; 256]> = OnceLock::new();
    let luma = (((color >> 16) & 255) * 299 + ((color >> 8) & 255) * 587 + (color & 255) * 114) / 1000;
    let build = |gamma: f32| {
        let mut table = [0u8; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let c = index as f32 / 255.0;
            *slot = (c.powf(1.0 / gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        table
    };
    if luma < 128 { DARK_TEXT.get_or_init(|| build(1.45)) } else { LIGHT_TEXT.get_or_init(|| build(1.2)) }
}

fn text_width(fonts: &FontBook, text: &str, size: f32) -> f32 {
    let size = snap(size, paint_scale()).max(1.0);
    fonts.shape(text, size, false, false, "").iter().map(|glyph| glyph.advance).sum()
}

fn blit_glyph(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    pen: f32,
    baseline: f32,
    glyph: &Glyph,
    color: u32,
    clip_left: f32,
    clip_right: f32,
    fade: f32,
    lut: &[u8; 256],
) {
    let left = (pen + glyph.x_offset).round() as i32 + glyph.xmin;
    let top = glyph_top(baseline, glyph);
    for row in 0..glyph.height {
        for col in 0..glyph.width {
            let px = left + col as i32;
            let py = top + row as i32;
            let fx = px as f32;
            if fx < clip_left || fx >= clip_right {
                continue;
            }
            let raw = glyph.coverage[row * glyph.width + col];
            if raw == 0 {
                continue;
            }
            let mut coverage = lut[raw as usize] as u32;
            if fade > 0.0 && fx > clip_right - fade {
                coverage = (coverage as f32 * ((clip_right - fx) / fade).clamp(0.0, 1.0)) as u32;
            }
            if coverage > 0 {
                blend(pixels, width, height, px, py, color, coverage.min(255) as u8);
            }
        }
    }
}

fn fill_rect(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, color: u32) {
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let x1 = (x + w).round().clamp(0.0, width as f32) as u32;
    let y1 = (y + h).round().clamp(0.0, height as f32) as u32;
    for row in y0..y1 {
        for col in x0..x1 {
            pixels[(row * width + col) as usize] = color;
        }
    }
}

fn fill_rect_alpha(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, color: u32, alpha: u8) {
    let x0 = x.round().max(0.0) as i32;
    let y0 = y.round().max(0.0) as i32;
    let x1 = (x + w).round().clamp(0.0, width as f32) as i32;
    let y1 = (y + h).round().clamp(0.0, height as f32) as i32;
    for row in y0..y1 {
        for col in x0..x1 {
            blend(pixels, width, height, col, row, color, alpha);
        }
    }
}

/// `fg` over `bg` at `alpha`.
fn mix(fg: u32, bg: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv = 255 - a;
    let ch = |shift: u32| ((((fg >> shift) & 255) * a + ((bg >> shift) & 255) * inv + 127) / 255) << shift;
    ch(16) | ch(8) | ch(0)
}

fn put(pixels: &mut [u32], width: u32, height: u32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    pixels[(y as u32 * width + x as u32) as usize] = color;
}

fn blend(pixels: &mut [u32], width: u32, height: u32, x: i32, y: i32, color: u32, alpha: u8) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let dest = &mut pixels[(y as u32 * width + x as u32) as usize];
    *dest = mix(color, *dest, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_tabs(count: usize) -> Vec<TabPaint> {
        (0..count)
            .map(|_| TabPaint { title: "Tab".into(), favicon: None, group: 0, group_name: String::new(), collapsed: false })
            .collect()
    }

    fn test_state(fonts: &FontBook, width: f32, height: f32, dark: bool) -> State<'_> {
        State {
            tabs: plain_tabs(3),
            omnibox: "https://example.com/".into(),
            can_back: true,
            dark,
            ..shallow(&State {
                width,
                height,
                tabs: Vec::new(),
                active: 0,
                omnibox: String::new(),
                omnibox_focused: false,
                cursor: 0,
                secure: Secure::Https,
                bookmarked: false,
                bookmarks: Vec::new(),
                bookmarks_open: false,
                can_back: false,
                can_forward: false,
                loading: false,
                progress: 0.0,
                link_status: String::new(),
                pointer: (-1.0, -1.0),
                pressed: false,
                ntp: false,
                menu: None,
                menu_labels: Vec::new(),
                submenu_at: None,
                submenu_labels: Vec::new(),
                tab_search: false,
                extensions: false,
                profile: false,
                suggestions: Vec::new(),
                suggestion: None,
                find_open: false,
                find_query: String::new(),
                fonts,
                scale: 1.0,
                dark,
                show_home: true,
                group_editor: false,
                permission_prompt: "",
            })
        }
    }

    #[test]
    fn home_sits_between_reload_and_the_omnibox() {
        let tools = toolbar(1280.0, 1.0, true);
        assert!(tools.home.x > tools.reload.x);
        assert!(tools.address.x > tools.home.x);
        assert!(tools.star.x > tools.address.x);
        assert!(tools.star.right() <= tools.address.right() + 1.0);
        assert!(tools.extensions.x >= tools.address.right() - 1.0);
        assert!(tools.menu.right() <= 1280.0);
    }

    #[test]
    fn toolbar_matches_chromium_metrics() {
        let tools = toolbar(1280.0, 1.0, true);
        // TOOLBAR_INTERIOR_MARGIN 6, kToolbarButtonHeight 34, 2 DIP gaps.
        assert_eq!(tools.back.x, 6.0);
        assert_eq!(tools.back.w, 34.0);
        assert_eq!(tools.forward.x - tools.back.right(), 2.0);
        assert_eq!(tools.back.y, TAB_H + 6.0);
        // kLocationBarHeight 34 and kLocationBarMargin 9.
        assert_eq!(tools.address.h, 34.0);
        assert_eq!(tools.address.x - tools.home.right(), 9.0);
        assert_eq!(tools.extensions.x - tools.address.right(), 9.0);
        assert_eq!(1280.0 - tools.menu.right(), 6.0);
        assert_eq!(chrome_height(false, 1.0), TAB_H + 46.0);
        assert_eq!(chrome_height(true, 1.0), TAB_H + 46.0 + 34.0);
    }

    #[test]
    fn tabs_use_the_standard_width_and_overlap() {
        let (tabs, _plus, _search) = tab_layout(1600.0, &plain_tabs(2), 1.0);
        assert_eq!(tabs[0].view.w, TAB_STANDARD_W);
        assert_eq!(tabs[1].view.x - tabs[0].view.x, TAB_STANDARD_W - TAB_OVERLAP);
        assert_eq!(TAB_OVERLAP, 18.0);
        // Favicon at extension + horizontal padding, title after the pre-title gap.
        assert_eq!(tabs[0].favicon.x - tabs[0].view.x, 20.0);
        assert_eq!(tabs[0].title_left - tabs[0].favicon.right(), 8.0);
        assert_eq!(tabs[0].view.right() - tabs[0].close.right(), 20.0);
    }

    #[test]
    fn tab_title_is_clipped_inside_the_tab_and_captions_stay_clear() {
        let (tabs, _plus, search) = tab_layout(1280.0, &plain_tabs(1), 1.0);
        let tab = &tabs[0];
        assert!(tab.title_right < tab.close.x);
        assert!(tab.close.right() <= tab.rect.right());
        assert!(tab.rect.w <= 240.0);
        assert!(search.right() < 1280.0 - CAPTION_W * 3.0);
        let (many, plus, _) = tab_layout(1280.0, &plain_tabs(30), 1.0);
        assert!(many.last().unwrap().rect.right() <= plus.x + 1.0);
        assert!(plus.right() < 1280.0 - CAPTION_W * 3.0);
    }

    #[test]
    fn surfaces_use_the_gm3_tokens_in_both_themes() {
        let fonts = FontBook::bundled();
        let (w, h) = (900u32, 140u32);
        for dark in [false, true] {
            let palette = if dark { &DARK } else { &LIGHT };
            let mut pixels = vec![0u32; (w * h) as usize];
            let state = test_state(&fonts, w as f32, h as f32, dark);
            paint(&mut pixels, w, h, &state);
            let (tabs, _, _) = tab_layout(w as f32, &state.tabs, 1.0);
            let x = (tabs[0].view.x + tabs[0].view.w / 2.0) as u32;
            assert_eq!(pixels[(38 * w + x) as usize], palette.toolbar, "active tab body");
            assert_eq!(pixels[(2 * w + 2) as usize], palette.frame, "frame");
            assert_eq!(pixels[(84 * w + 300) as usize], palette.toolbar, "toolbar");
        }
    }

    #[test]
    fn display_url_hides_the_scheme_and_splits_the_host() {
        assert_eq!(split_display_url("https://www.example.com/"), ("example.com", ""));
        assert_eq!(split_display_url("https://en.wikipedia.org/wiki/Main_Page"), ("en.wikipedia.org", "/wiki/Main_Page"));
        assert_eq!(split_display_url("mithu://settings"), ("mithu://settings", ""));
    }

    #[test]
    fn ring_stroke_leaves_the_middle_empty() {
        let (w, h) = (40u32, 40u32);
        let mut pixels = vec![0u32; (w * h) as usize];
        stroke_round(&mut pixels, w, h, 2.0, 2.0, 36.0, 36.0, 8.0, 2.0, 0xffffff);
        assert_eq!(pixels[(20 * w + 20) as usize], 0);
        assert_eq!(pixels[(20 * w + 2) as usize], 0xffffff);
    }

    #[test]
    fn fractional_scales_land_on_whole_pixels() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let height = chrome_height(true, scale);
            assert_eq!(height, height.round());
            let (tabs, plus, search) = tab_layout(1100.0 * scale, &plain_tabs(3), scale);
            for tab in &tabs {
                assert_eq!(tab.rect.x, tab.rect.x.round());
                assert_eq!(tab.rect.y, tab.rect.y.round());
                assert_eq!(tab.rect.w, tab.rect.w.round());
                assert_eq!(tab.close.x, tab.close.x.round());
            }
            assert_eq!(plus.x, plus.x.round());
            assert_eq!(search.y, search.y.round());
            let tools = toolbar(1100.0 * scale, scale, true);
            assert_eq!(tools.address.x, tools.address.x.round());
            assert_eq!(tools.address.h, tools.address.h.round());
            assert!(tools.home.x > tools.reload.x);
            assert!(tools.address.x >= tools.home.right() - 1.0);
            let hair = height - 1.0;
            assert_eq!(hair, hair.round());
            PAINT_SCALE.set(scale);
            let fonts = FontBook::bundled();
            let mut state = test_state(&fonts, 1100.0 * scale, 720.0 * scale, false);
            state.scale = scale;
            state.menu = Some(Popup::App);
            state.menu_labels = vec!["New tab".into(), "History".into(), "Downloads".into(), "Settings".into()];
            let menu = menu_rect(&state, Popup::App);
            assert_eq!(menu.w, (336.0 * scale).round());
            assert!(menu.right() <= state.width + 0.5);
            assert!(menu.x >= 0.0);
            let label = text_width(&fonts, "Downloads", 13.0);
            assert!(dip(52.0) + label < menu.w, "menu text fits at {scale}");
        }
    }
}
