//! Cascade computed styles from the user-agent sheet, author CSS, and `style` attributes.

use std::collections::HashMap;

use mithu_base::Color;
use mithu_css::{
    AttrOp, Combinator, Declaration, Pseudo, Selector, Stylesheet, parse_css, parse_declarations,
};
use mithu_dom::{Document, NodeId};

pub const USER_AGENT_CSS: &str = r#"
html, address, article, aside, blockquote, div, dl, fieldset, figcaption, figure,
footer, form, h1, h2, h3, h4, h5, h6, header, hr, main, nav, ol, p, pre, section,
table, ul, dd, dt { display: block; }
li { display: list-item; }
head, style, script, title, meta, link, base, template, noscript { display: none; }
table { display: block; }
thead, tbody, tfoot, tr, caption { display: block; }
td, th { display: inline-block; padding-top: 2px; padding-right: 6px; padding-bottom: 2px; padding-left: 6px; }
th { font-weight: bold; }
[hidden] { display: none; }
span, a, b, strong, i, em, code, label, small, sup, sub { display: inline; }
img, br, button, input, textarea, select, iframe, video, canvas, svg, audio { display: inline-block; }
body { margin: 8px; color: #000000; background-color: #ffffff; font-size: 16px; }
h1 { font-size: 32px; margin-top: 21px; margin-bottom: 21px; font-weight: bold; }
h2 { font-size: 24px; margin-top: 19px; margin-bottom: 19px; font-weight: bold; }
h3 { font-size: 19px; margin-top: 18px; margin-bottom: 18px; font-weight: bold; }
h4, h5, h6 { font-size: 16px; margin-top: 21px; margin-bottom: 21px; font-weight: bold; }
p { margin-top: 16px; margin-bottom: 16px; }
ul, ol { margin-top: 16px; margin-bottom: 16px; padding-left: 40px; }
li { margin-top: 4px; margin-bottom: 4px; }
pre { font-family: monospace; white-space: pre; margin-top: 16px; margin-bottom: 16px; }
code { font-family: monospace; }
blockquote { margin: 16px 40px; }
a { color: #1a0dab; text-decoration: underline; }
strong, b { font-weight: bold; }
hr { border-width: 1px; border-color: #cccccc; margin-top: 8px; margin-bottom: 8px; height: 0px; }
img, iframe, video, canvas, audio { background-color: #ececec; border-width: 1px; border-color: #bdbdbd; }
svg { overflow: hidden; }
button, input, textarea, select {
  border-width: 1px; border-color: #888888; background-color: #ffffff;
  padding-top: 4px; padding-right: 8px; padding-bottom: 4px; padding-left: 8px; color: #111111;
  white-space: nowrap;
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Inline,
    Block,
    InlineBlock,
    ListItem,
    Flex,
    /// `inline-flex`: a flex container that shrinks to its contents.
    InlineFlex,
    Grid,
    /// `display: contents`. The element generates no box; its children belong
    /// to its parent for layout.
    Contents,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatSide {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GridTrack {
    Px(f32),
    Fr(f32),
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// One `grid-column` or `grid-row` placement.
///
/// `start` is a 1-based line. `None` means the auto-placement cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSpan {
    pub start: Option<u16>,
    pub span: u16,
}

impl Default for GridSpan {
    fn default() -> Self {
        Self {
            start: None,
            span: 1,
        }
    }
}

/// A single `box-shadow`. Chromium paints a Gaussian; mithu approximates it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start,
    End,
    Center,
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSizing {
    ContentBox,
    BorderBox,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Auto,
    Px(f32),
    Percent(f32),
    Em(f32),
    Rem(f32),
    Vw(f32),
    Vh(f32),
    Calc {
        percent: f32,
        px: f32,
        em: f32,
        rem: f32,
        vw: f32,
        vh: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Px(f32),
    Number(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Nowrap,
    Pre,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub color: Color,
    pub background: Color,
    pub font_size: f32,
    pub bold: bool,
    pub mono: bool,
    pub font_family: String,
    pub text_align: TextAlign,
    pub underline: bool,
    pub white_space: WhiteSpace,
    pub width: Length,
    pub height: Length,
    pub margin: [Length; 4],
    pub padding: [Length; 4],
    pub border_width: f32,
    pub border_color: Color,
    pub line_height: LineHeight,
    pub position: Position,
    pub insets: [Length; 4],
    pub min_width: Length,
    pub max_width: Length,
    pub min_height: Length,
    pub max_height: Length,
    pub opacity: f32,
    pub flex_direction: FlexDirection,
    pub flex_reverse: bool,
    pub gap: f32,
    pub row_gap: f32,
    pub flex_wrap: bool,
    pub flex_wrap_reverse: bool,
    pub justify: Justify,
    pub align_items: AlignItems,
    pub align_self: Option<AlignItems>,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,
    pub float: FloatSide,
    pub clear: Clear,
    pub grid_columns: Vec<GridTrack>,
    pub grid_rows: Vec<GridTrack>,
    /// `grid-auto-flow: column`.
    pub column_flow: bool,
    pub auto_columns: Vec<GridTrack>,
    pub grid_areas: Vec<Vec<String>>,
    pub grid_area: String,
    pub grid_column: GridSpan,
    pub grid_row: GridSpan,
    pub shadow: Option<BoxShadow>,
    pub box_sizing: BoxSizing,
    pub overflow_hidden: bool,
    pub border_radius: f32,
    pub visible: bool,
    pub z_index: i32,
    pub text_transform: TextTransform,
    pub borders: [f32; 4],
    pub before: Option<Generated>,
    pub after: Option<Generated>,
    pub gradient: Option<(Color, Color)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub text: String,
    pub color: Color,
    pub background: Color,
    pub font_size: f32,
    pub bold: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

impl ComputedStyle {
    pub fn initial() -> Self {
        Self {
            display: Display::Inline,
            color: Color::BLACK,
            background: Color::TRANSPARENT,
            font_size: 16.0,
            bold: false,
            mono: false,
            font_family: String::new(),
            text_align: TextAlign::Left,
            underline: false,
            white_space: WhiteSpace::Normal,
            width: Length::Auto,
            height: Length::Auto,
            margin: [Length::Px(0.0); 4],
            padding: [Length::Px(0.0); 4],
            border_width: 0.0,
            border_color: Color::BLACK,
            line_height: LineHeight::Normal,
            position: Position::Static,
            insets: [Length::Auto; 4],
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            max_height: Length::Auto,
            opacity: 1.0,
            flex_direction: FlexDirection::Row,
            flex_reverse: false,
            gap: 0.0,
            row_gap: 0.0,
            flex_wrap: false,
            flex_wrap_reverse: false,
            justify: Justify::Start,
            align_items: AlignItems::Stretch,
            align_self: None,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            float: FloatSide::None,
            clear: Clear::None,
            grid_columns: Vec::new(),
            grid_rows: Vec::new(),
            column_flow: false,
            auto_columns: Vec::new(),
            grid_areas: Vec::new(),
            grid_area: String::new(),
            grid_column: GridSpan::default(),
            grid_row: GridSpan::default(),
            shadow: None,
            box_sizing: BoxSizing::ContentBox,
            overflow_hidden: false,
            border_radius: 0.0,
            visible: true,
            z_index: 0,
            text_transform: TextTransform::None,
            borders: [0.0; 4],
            before: None,
            after: None,
            gradient: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StyleState {
    pub hover: Option<NodeId>,
    pub viewport_width: f32,
}

impl Default for StyleState {
    fn default() -> Self {
        Self {
            hover: None,
            viewport_width: 1280.0,
        }
    }
}

#[derive(Clone, Debug)]
struct Ranked<T> {
    value: T,
    spec: u32,
}

#[derive(Default)]
struct Specified {
    display: Option<Ranked<Display>>,
    color: Option<Ranked<Color>>,
    background: Option<Ranked<Color>>,
    gradient: Option<Ranked<(Color, Color)>>,
    font_size: Option<Ranked<FontSize>>,
    bold: Option<Ranked<bool>>,
    mono: Option<Ranked<bool>>,
    font_family: Option<Ranked<String>>,
    text_align: Option<Ranked<TextAlign>>,
    underline: Option<Ranked<bool>>,
    white_space: Option<Ranked<WhiteSpace>>,
    width: Option<Ranked<Length>>,
    height: Option<Ranked<Length>>,
    margin: [Option<Ranked<Length>>; 4],
    padding: [Option<Ranked<Length>>; 4],
    border_width: Option<Ranked<f32>>,
    border_sides: [Option<Ranked<f32>>; 4],
    border_color: Option<Ranked<Color>>,
    line_height: Option<Ranked<LineHeight>>,
    position: Option<Ranked<Position>>,
    insets: [Option<Ranked<Length>>; 4],
    min_width: Option<Ranked<Length>>,
    max_width: Option<Ranked<Length>>,
    min_height: Option<Ranked<Length>>,
    max_height: Option<Ranked<Length>>,
    opacity: Option<Ranked<f32>>,
    flex_direction: Option<Ranked<(FlexDirection, bool)>>,
    gap: Option<Ranked<f32>>,
    row_gap: Option<Ranked<f32>>,
    flex_wrap: Option<Ranked<(bool, bool)>>,
    justify: Option<Ranked<Justify>>,
    align_items: Option<Ranked<AlignItems>>,
    align_self: Option<Ranked<AlignItems>>,
    flex_grow: Option<Ranked<f32>>,
    flex_shrink: Option<Ranked<f32>>,
    flex_basis: Option<Ranked<Length>>,
    float: Option<Ranked<FloatSide>>,
    clear: Option<Ranked<Clear>>,
    grid_columns: Option<Ranked<Vec<GridTrack>>>,
    grid_rows: Option<Ranked<Vec<GridTrack>>>,
    column_flow: Option<Ranked<bool>>,
    auto_columns: Option<Ranked<Vec<GridTrack>>>,
    grid_areas: Option<Ranked<Vec<Vec<String>>>>,
    grid_area: Option<Ranked<String>>,
    grid_column: Option<Ranked<GridSpan>>,
    grid_row: Option<Ranked<GridSpan>>,
    shadow: Option<Ranked<BoxShadow>>,
    box_sizing: Option<Ranked<BoxSizing>>,
    overflow_hidden: Option<Ranked<bool>>,
    border_radius: Option<Ranked<f32>>,
    visible: Option<Ranked<bool>>,
    z_index: Option<Ranked<i32>>,
    text_transform: Option<Ranked<TextTransform>>,
    custom: HashMap<String, Ranked<String>>,
}

#[derive(Clone, Copy, Debug)]
enum FontSize {
    Px(f32),
    Em(f32),
    Percent(f32),
}

pub fn matches_element(doc: &Document, id: NodeId, selector: &Selector) -> bool {
    matches_selector(doc, id, selector, StyleState::default())
}

pub fn style_document(doc: &Document, author_css: &str) -> Vec<ComputedStyle> {
    style_with(doc, author_css, StyleState::default())
}

/// `(family, url)` pairs from `@font-face` rules.
pub fn font_faces(css: &str) -> Vec<(String, String)> {
    parse_css(css)
        .font_faces
        .into_iter()
        .map(|face| (face.family, face.url))
        .collect()
}

pub fn style_with(doc: &Document, author_css: &str, state: StyleState) -> Vec<ComputedStyle> {
    let ua = parse_css(USER_AGENT_CSS);
    let author = parse_css(author_css);
    let mut styles = vec![ComputedStyle::initial(); doc.nodes.len()];
    let initial = ComputedStyle::initial();
    let vars = HashMap::new();
    for child in doc.node(doc.root).children.clone() {
        compute(
            doc,
            child,
            &initial,
            &ua,
            &author,
            state,
            &vars,
            &mut styles,
        );
    }
    styles
}

fn compute(
    doc: &Document,
    id: NodeId,
    parent: &ComputedStyle,
    ua: &Stylesheet,
    author: &Stylesheet,
    state: StyleState,
    parent_vars: &HashMap<String, String>,
    styles: &mut [ComputedStyle],
) {
    if doc.element_name(id).is_none() {
        return;
    }
    let (specified, vars) = cascade(doc, id, ua, author, state, parent_vars);
    let mut computed = concretize(doc, id, &specified, parent);
    computed.before = build_generated(doc, id, ua, author, state, &vars, &computed, true);
    computed.after = build_generated(doc, id, ua, author, state, &vars, &computed, false);
    styles[id.0] = computed.clone();
    for child in doc.node(id).children.clone() {
        compute(doc, child, &computed, ua, author, state, &vars, styles);
    }
}

fn cascade(
    doc: &Document,
    id: NodeId,
    ua: &Stylesheet,
    author: &Stylesheet,
    state: StyleState,
    parent_vars: &HashMap<String, String>,
) -> (Specified, HashMap<String, String>) {
    let mut specified = Specified::default();
    collect_custom(doc, id, ua, state, &mut specified);
    collect_custom(doc, id, author, state, &mut specified);
    if let Some(inline) = doc.attr(id, "style") {
        for declaration in parse_declarations(inline) {
            if declaration.name.starts_with("--") {
                let spec = 1_000 + if declaration.important { 100_000 } else { 0 };
                rank_custom(
                    &mut specified.custom,
                    &declaration.name,
                    declaration.value,
                    spec,
                );
            }
        }
    }
    let vars = resolve_custom(&specified.custom, parent_vars);
    apply_sheet(doc, id, ua, state, &vars, &mut specified);
    apply_sheet(doc, id, author, state, &vars, &mut specified);
    if let Some(inline) = doc.attr(id, "style") {
        for declaration in parse_declarations(inline) {
            let spec = 1_000 + if declaration.important { 100_000 } else { 0 };
            apply_declaration(&declaration, spec, &vars, &mut specified);
        }
    }
    (specified, vars)
}

fn collect_custom(
    doc: &Document,
    id: NodeId,
    sheet: &Stylesheet,
    state: StyleState,
    specified: &mut Specified,
) {
    for rule in &sheet.rules {
        if !media_applies(rule.media.as_ref(), state.viewport_width) {
            continue;
        }
        for selector in &rule.selectors {
            if !matches_selector(doc, id, selector, state) {
                continue;
            }
            for declaration in &rule.declarations {
                if declaration.name.starts_with("--") {
                    let spec =
                        selector.specificity() + if declaration.important { 100_000 } else { 0 };
                    rank_custom(
                        &mut specified.custom,
                        &declaration.name,
                        declaration.value.clone(),
                        spec,
                    );
                }
            }
        }
    }
}

fn rank_custom(map: &mut HashMap<String, Ranked<String>>, name: &str, value: String, spec: u32) {
    let name = name.to_ascii_lowercase();
    match map.get(&name) {
        Some(existing) if existing.spec > spec => {}
        _ => {
            map.insert(name, Ranked { value, spec });
        }
    }
}

fn resolve_custom(
    own: &HashMap<String, Ranked<String>>,
    parent: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut resolved = parent.clone();
    for (name, ranked) in own {
        resolved.insert(name.clone(), ranked.value.clone());
    }
    for _ in 0..8 {
        let snapshot = resolved.clone();
        for value in resolved.values_mut() {
            *value = expand_vars(value, &snapshot);
        }
    }
    resolved
}

fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    expand_vars_limited(input, vars, 0)
}

fn expand_vars_limited(input: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    if depth > 8 {
        return input.to_string();
    }
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.to_ascii_lowercase().find("var(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let Some(end) = matching_paren(after) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let (name, fallback) = split_var(inner);
        let name = name.trim().to_ascii_lowercase();
        if let Some(value) = vars.get(&name) {
            if value.to_ascii_lowercase().contains("var(") {
                out.push_str(&expand_vars_limited(value, vars, depth + 1));
            } else {
                out.push_str(value);
            }
        } else if let Some(fallback) = fallback {
            out.push_str(&expand_vars_limited(fallback.trim(), vars, depth + 1));
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn matching_paren(input: &str) -> Option<usize> {
    let mut depth = 1;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_var(input: &str) -> (&str, Option<&str>) {
    let mut depth = 0;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return (&input[..index], Some(&input[index + 1..])),
            _ => {}
        }
    }
    (input, None)
}

fn apply_sheet(
    doc: &Document,
    id: NodeId,
    sheet: &Stylesheet,
    state: StyleState,
    vars: &HashMap<String, String>,
    specified: &mut Specified,
) {
    for rule in &sheet.rules {
        if !media_applies(rule.media.as_ref(), state.viewport_width) {
            continue;
        }
        for selector in &rule.selectors {
            if !matches_selector(doc, id, selector, state) {
                continue;
            }
            for declaration in &rule.declarations {
                let spec = selector.specificity() + if declaration.important { 100_000 } else { 0 };
                apply_declaration(declaration, spec, vars, specified);
            }
        }
    }
}

fn matches_selector(
    doc: &Document,
    id: NodeId,
    selector: &mithu_css::Selector,
    state: StyleState,
) -> bool {
    let Some(last) = selector.compounds.last() else {
        return false;
    };
    if !matches_compound(doc, id, last, state) {
        return false;
    }
    let mut current = id;
    for index in (0..selector.compounds.len() - 1).rev() {
        let combinator = selector.combinators.get(index).copied();
        let compound = &selector.compounds[index];
        match combinator {
            Some(Combinator::Child) => {
                let Some(parent) = doc.parent_element(current) else {
                    return false;
                };
                if !matches_compound(doc, parent, compound, state) {
                    return false;
                }
                current = parent;
            }
            Some(Combinator::NextSibling) => {
                let Some(previous) = doc.previous_element_sibling(current) else {
                    return false;
                };
                if !matches_compound(doc, previous, compound, state) {
                    return false;
                }
                current = previous;
            }
            Some(Combinator::SubsequentSibling) => {
                let mut previous = doc.previous_element_sibling(current);
                let mut found = false;
                while let Some(candidate) = previous {
                    if matches_compound(doc, candidate, compound, state) {
                        current = candidate;
                        found = true;
                        break;
                    }
                    previous = doc.previous_element_sibling(candidate);
                }
                if !found {
                    return false;
                }
            }
            Some(Combinator::Descendant) | None => {
                let mut parent = doc.parent_element(current);
                let mut found = false;
                while let Some(candidate) = parent {
                    if matches_compound(doc, candidate, compound, state) {
                        current = candidate;
                        found = true;
                        break;
                    }
                    parent = doc.parent_element(candidate);
                }
                if !found {
                    return false;
                }
            }
        }
    }
    true
}

fn matches_compound(
    doc: &Document,
    id: NodeId,
    compound: &mithu_css::Compound,
    state: StyleState,
) -> bool {
    let Some(element) = doc.element(id) else {
        return false;
    };
    if let Some(tag) = &compound.tag
        && element.name != *tag
    {
        return false;
    }
    if let Some(id_attr) = &compound.id
        && element.id.as_ref() != Some(id_attr)
    {
        return false;
    }
    if !compound
        .classes
        .iter()
        .all(|class| element.classes.iter().any(|have| have == class))
    {
        return false;
    }
    if !compound
        .attrs
        .iter()
        .all(|attr| matches_attr(doc, id, attr))
    {
        return false;
    }
    compound
        .pseudos
        .iter()
        .all(|pseudo| matches_pseudo(doc, id, pseudo, state))
}

fn matches_attr(doc: &Document, id: NodeId, attr: &mithu_css::AttrSelector) -> bool {
    let Some(value) = doc.attr(id, &attr.name) else {
        return false;
    };
    let Some(op) = attr.op else {
        return true;
    };
    let expected = attr.value.as_deref().unwrap_or("");
    match op {
        AttrOp::Exact => value == expected,
        AttrOp::Includes => value.split_whitespace().any(|part| part == expected),
        AttrOp::Dash => {
            value == expected
                || value
                    .strip_prefix(expected)
                    .is_some_and(|rest| rest.starts_with('-'))
        }
        AttrOp::Prefix => value.starts_with(expected),
        AttrOp::Suffix => value.ends_with(expected),
        AttrOp::Substring => value.contains(expected),
    }
}

fn matches_pseudo(doc: &Document, id: NodeId, pseudo: &Pseudo, state: StyleState) -> bool {
    match pseudo {
        Pseudo::FirstChild => doc.previous_element_sibling(id).is_none(),
        Pseudo::LastChild => doc.next_element_sibling(id).is_none(),
        Pseudo::Nth(a, b) => doc
            .element_child_index(id)
            .is_some_and(|index| nth_matches(*a, *b, index)),
        Pseudo::Link => doc.element_name(id) == Some("a") && doc.attr(id, "href").is_some(),
        Pseudo::Empty => is_empty(doc, id),
        Pseudo::Hover => is_hovered(doc, id, state.hover),
        Pseudo::Root => doc.parent_element(id).is_none(),
        Pseudo::Before | Pseudo::After => false,
        Pseudo::Not(inner) => !matches_compound(doc, id, inner, state),
        Pseudo::Is(list) => list
            .iter()
            .any(|selector| matches_selector(doc, id, selector, state)),
        Pseudo::Where(list) => list
            .iter()
            .any(|selector| matches_selector(doc, id, selector, state)),
        Pseudo::Unmatched => false,
    }
}

fn is_hovered(doc: &Document, id: NodeId, hover: Option<NodeId>) -> bool {
    let Some(mut current) = hover else {
        return false;
    };
    loop {
        if current == id {
            return true;
        }
        match doc.parent_element(current) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

fn nth_matches(a: i32, b: i32, index: usize) -> bool {
    let index = index as i32;
    if a == 0 {
        return index == b;
    }
    let delta = index - b;
    delta % a == 0 && delta / a >= 0
}

fn is_empty(doc: &Document, id: NodeId) -> bool {
    doc.node(id)
        .children
        .iter()
        .all(|child| match &doc.node(*child).data {
            mithu_dom::NodeData::Comment(_) => true,
            mithu_dom::NodeData::Text(text) => text.trim().is_empty(),
            _ => false,
        })
}

fn apply_declaration(
    declaration: &Declaration,
    spec: u32,
    vars: &HashMap<String, String>,
    specified: &mut Specified,
) {
    if declaration.name.starts_with("--") {
        return;
    }
    let expanded = expand_vars(&declaration.value, vars);
    let value = expanded.as_str();
    match declaration.name.as_str() {
        "display" => {
            if let Some(display) = parse_display(value) {
                rank(&mut specified.display, display, spec);
            }
        }
        "color" => {
            if let Some(color) = parse_color(value) {
                rank(&mut specified.color, color, spec);
            }
        }
        "background-color" => {
            if let Some(color) = parse_color(value) {
                rank(&mut specified.background, color, spec);
            }
        }
        "background" | "background-image" => {
            if let Some(pair) = parse_linear_gradient(value) {
                rank(&mut specified.gradient, pair, spec);
            }
            if declaration.name == "background"
                && let Some(color) = parse_color(value)
            {
                rank(&mut specified.background, color, spec);
            }
        }
        "font-size" => {
            if let Some(size) = parse_font_size(value) {
                rank(&mut specified.font_size, size, spec);
            }
        }
        "font-weight" => {
            if let Some(bold) = parse_bold(value) {
                rank(&mut specified.bold, bold, spec);
            }
        }
        "font-family" => {
            rank(
                &mut specified.mono,
                value.to_ascii_lowercase().contains("mono"),
                spec,
            );
            let family = value
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(|ch| ch == '"' || ch == '\'')
                .to_string();
            if !family.is_empty() {
                rank(&mut specified.font_family, family, spec);
            }
        }
        "text-align" => {
            if let Some(align) = parse_align(value) {
                rank(&mut specified.text_align, align, spec);
            }
        }
        "text-decoration" => rank(&mut specified.underline, parse_underline(value), spec),
        "white-space" => {
            if let Some(space) = parse_white_space(value) {
                rank(&mut specified.white_space, space, spec);
            }
        }
        "width" => {
            if let Some(length) = parse_length(value) {
                rank(&mut specified.width, length, spec);
            }
        }
        "height" => {
            if let Some(length) = parse_length(value) {
                rank(&mut specified.height, length, spec);
            }
        }
        "margin" => {
            if let Some(sides) = parse_sides(value) {
                for (slot, side) in specified.margin.iter_mut().zip(sides) {
                    rank(slot, side, spec);
                }
            }
        }
        "margin-top" => set_side(&mut specified.margin, 0, value, spec),
        "margin-right" => set_side(&mut specified.margin, 1, value, spec),
        "margin-bottom" => set_side(&mut specified.margin, 2, value, spec),
        "margin-left" => set_side(&mut specified.margin, 3, value, spec),
        "margin-inline" => set_logical(&mut specified.margin, 3, 1, value, spec, false),
        "margin-block" => set_logical(&mut specified.margin, 0, 2, value, spec, false),
        "padding" => {
            if let Some(sides) = parse_sides(value) {
                for (slot, side) in specified.padding.iter_mut().zip(sides) {
                    rank(slot, zero_auto(side), spec);
                }
            }
        }
        "padding-top" => set_padding(&mut specified.padding, 0, value, spec),
        "padding-right" => set_padding(&mut specified.padding, 1, value, spec),
        "padding-bottom" => set_padding(&mut specified.padding, 2, value, spec),
        "padding-left" => set_padding(&mut specified.padding, 3, value, spec),
        "padding-inline" => set_logical(&mut specified.padding, 3, 1, value, spec, true),
        "padding-block" => set_logical(&mut specified.padding, 0, 2, value, spec, true),
        "border" => {
            if let Some((width, color)) = parse_border(value) {
                rank_uniform_border(specified, width, spec);
                if let Some(color) = color {
                    rank(&mut specified.border_color, color, spec);
                }
            }
        }
        "border-width" => {
            if let Some(sides) = parse_border_widths(value) {
                rank_uniform_border(specified, sides[0], spec);
                for (slot, side) in specified.border_sides.iter_mut().zip(sides) {
                    rank(slot, side, spec);
                }
            }
        }
        "border-top-width" => set_border_side(&mut specified.border_sides, 0, value, spec),
        "border-right-width" => set_border_side(&mut specified.border_sides, 1, value, spec),
        "border-bottom-width" => set_border_side(&mut specified.border_sides, 2, value, spec),
        "border-left-width" => set_border_side(&mut specified.border_sides, 3, value, spec),
        "border-top" => set_border_edge(specified, 0, value, spec),
        "border-right" => set_border_edge(specified, 1, value, spec),
        "border-bottom" => set_border_edge(specified, 2, value, spec),
        "border-left" => set_border_edge(specified, 3, value, spec),
        "border-color" => {
            if let Some(color) = parse_color(value) {
                rank(&mut specified.border_color, color, spec);
            }
        }
        "line-height" => {
            if let Some(height) = parse_line_height(value) {
                rank(&mut specified.line_height, height, spec);
            }
        }
        "position" => {
            if let Some(position) = parse_position(value) {
                rank(&mut specified.position, position, spec);
            }
        }
        "top" => set_side(&mut specified.insets, 0, value, spec),
        "right" => set_side(&mut specified.insets, 1, value, spec),
        "bottom" => set_side(&mut specified.insets, 2, value, spec),
        "left" => set_side(&mut specified.insets, 3, value, spec),
        "inset" => {
            if let Some(sides) = parse_sides(value) {
                for (slot, side) in specified.insets.iter_mut().zip(sides) {
                    rank(slot, side, spec);
                }
            }
        }
        "inset-inline" => set_logical(&mut specified.insets, 3, 1, value, spec, false),
        "inset-block" => set_logical(&mut specified.insets, 0, 2, value, spec, false),
        "min-width" => set_optional_length(&mut specified.min_width, value, spec),
        "max-width" => set_optional_length(&mut specified.max_width, value, spec),
        "min-height" => set_optional_length(&mut specified.min_height, value, spec),
        "max-height" => set_optional_length(&mut specified.max_height, value, spec),
        "opacity" => {
            if let Some(opacity) = parse_opacity(value) {
                rank(&mut specified.opacity, opacity, spec);
            }
        }
        "flex-direction" => {
            if let Some(direction) = parse_flex_direction(value) {
                rank(&mut specified.flex_direction, direction, spec);
            }
        }
        "gap" => {
            if let Some((row, column)) = parse_gap(value) {
                rank(&mut specified.row_gap, row, spec);
                rank(&mut specified.gap, column, spec);
            }
        }
        "row-gap" => {
            if let Some(px) = px_length(value) {
                rank(&mut specified.row_gap, px, spec);
            }
        }
        "column-gap" => {
            if let Some(px) = px_length(value) {
                rank(&mut specified.gap, px, spec);
            }
        }
        "flex-wrap" => {
            if let Some(wrap) = parse_flex_wrap(value) {
                rank(&mut specified.flex_wrap, wrap, spec);
            }
        }
        "align-self" => {
            if value.trim().eq_ignore_ascii_case("auto") {
                if specified
                    .align_self
                    .as_ref()
                    .is_none_or(|item| spec >= item.spec)
                {
                    specified.align_self = None;
                }
            } else if let Some(align) = parse_align_items(value) {
                rank(&mut specified.align_self, align, spec);
            }
        }
        "flex-basis" => {
            if let Some(length) = parse_length(value) {
                rank(&mut specified.flex_basis, length, spec);
            }
        }
        "visibility" => {
            let visible = !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "hidden" | "collapse"
            );
            rank(&mut specified.visible, visible, spec);
        }
        "z-index" => {
            if value.trim().eq_ignore_ascii_case("auto") {
                rank(&mut specified.z_index, 0, spec);
            } else if let Some(index) = value.trim().parse::<i32>().ok() {
                rank(&mut specified.z_index, index, spec);
            }
        }
        "text-transform" => {
            if let Some(transform) = parse_text_transform(value) {
                rank(&mut specified.text_transform, transform, spec);
            }
        }
        "justify-content" => {
            if let Some(justify) = parse_justify(value) {
                rank(&mut specified.justify, justify, spec);
            }
        }
        "align-items" => {
            if let Some(align) = parse_align_items(value) {
                rank(&mut specified.align_items, align, spec);
            }
        }
        "flex-grow" => {
            if let Some(grow) = value.trim().parse::<f32>().ok() {
                rank(&mut specified.flex_grow, grow.max(0.0), spec);
            }
        }
        "flex-shrink" => {
            if let Some(shrink) = value.trim().parse::<f32>().ok() {
                rank(&mut specified.flex_shrink, shrink.max(0.0), spec);
            }
        }
        "float" => {
            if let Some(side) = parse_float(value) {
                rank(&mut specified.float, side, spec);
            }
        }
        "clear" => {
            if let Some(clear) = parse_clear(value) {
                rank(&mut specified.clear, clear, spec);
            }
        }
        "grid-template-columns" => {
            if let Some(tracks) = parse_grid_tracks(value) {
                rank(&mut specified.grid_columns, tracks, spec);
            }
        }
        "grid-template-rows" => {
            if let Some(tracks) = parse_grid_tracks(value) {
                rank(&mut specified.grid_rows, tracks, spec);
            }
        }
        "grid-template" => {
            if let Some((rows, columns)) = parse_grid_template(value) {
                rank(&mut specified.grid_rows, rows, spec);
                rank(&mut specified.grid_columns, columns, spec);
            }
        }
        "grid-template-areas" => {
            rank(&mut specified.grid_areas, parse_grid_areas(value), spec);
        }
        "grid-area" => {
            let name = value
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string();
            if !name.is_empty() && !name.eq_ignore_ascii_case("auto") {
                rank(&mut specified.grid_area, name, spec);
            }
        }
        "grid-auto-flow" => {
            let column = value
                .to_ascii_lowercase()
                .split_whitespace()
                .any(|part| part == "column");
            rank(&mut specified.column_flow, column, spec);
        }
        "grid-auto-columns" => {
            if let Some(tracks) = parse_grid_tracks(value) {
                rank(&mut specified.auto_columns, tracks, spec);
            }
        }
        "grid-column" | "grid-column-start" => {
            if let Some(span) = parse_grid_span(value) {
                rank(&mut specified.grid_column, span, spec);
            }
        }
        "grid-row" | "grid-row-start" => {
            if let Some(span) = parse_grid_span(value) {
                rank(&mut specified.grid_row, span, spec);
            }
        }
        "box-shadow" => {
            if let Some(shadow) = parse_box_shadow(value) {
                rank(&mut specified.shadow, shadow, spec);
            }
        }
        "box-sizing" => {
            if let Some(sizing) = parse_box_sizing(value) {
                rank(&mut specified.box_sizing, sizing, spec);
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            let hidden = matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "hidden" | "clip" | "scroll"
            );
            rank(&mut specified.overflow_hidden, hidden, spec);
        }
        "border-radius" => {
            if let Some(Length::Px(px)) =
                parse_length(value.split_whitespace().next().unwrap_or(value))
            {
                rank(&mut specified.border_radius, px.max(0.0), spec);
            }
        }
        _ => {}
    }
}

fn rank<T>(slot: &mut Option<Ranked<T>>, value: T, spec: u32) {
    if slot.as_ref().is_none_or(|current| spec >= current.spec) {
        *slot = Some(Ranked { value, spec });
    }
}

fn set_side(sides: &mut [Option<Ranked<Length>>; 4], index: usize, value: &str, spec: u32) {
    if let Some(length) = parse_length(value) {
        rank(&mut sides[index], length, spec);
    }
}

fn set_optional_length(slot: &mut Option<Ranked<Length>>, value: &str, spec: u32) {
    if value.trim().eq_ignore_ascii_case("none") {
        rank(slot, Length::Auto, spec);
        return;
    }
    if let Some(length) = parse_length(value) {
        rank(slot, length, spec);
    }
}

fn set_logical(
    sides: &mut [Option<Ranked<Length>>; 4],
    start: usize,
    end: usize,
    value: &str,
    spec: u32,
    padding: bool,
) {
    let parts: Vec<Length> = value.split_whitespace().filter_map(parse_length).collect();
    let (first, second) = match parts.as_slice() {
        [first] => (*first, *first),
        [first, second, ..] => (*first, *second),
        _ => return,
    };
    let normalize = |length: Length| {
        if padding { zero_auto(length) } else { length }
    };
    rank(&mut sides[start], normalize(first), spec);
    rank(&mut sides[end], normalize(second), spec);
}

fn set_padding(sides: &mut [Option<Ranked<Length>>; 4], index: usize, value: &str, spec: u32) {
    if let Some(length) = parse_length(value) {
        rank(&mut sides[index], zero_auto(length), spec);
    }
}

fn zero_auto(length: Length) -> Length {
    if matches!(length, Length::Auto) {
        Length::Px(0.0)
    } else {
        length
    }
}

fn concretize(
    doc: &Document,
    id: NodeId,
    specified: &Specified,
    parent: &ComputedStyle,
) -> ComputedStyle {
    let font_size = match specified.font_size.as_ref().map(|item| item.value) {
        Some(FontSize::Px(value)) => value.max(1.0),
        Some(FontSize::Em(value)) => (parent.font_size * value).max(1.0),
        Some(FontSize::Percent(value)) => (parent.font_size * value / 100.0).max(1.0),
        None => parent.font_size,
    };
    let color = specified
        .color
        .as_ref()
        .map(|item| item.value)
        .unwrap_or(parent.color);
    let mut computed = ComputedStyle {
        display: specified
            .display
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Display::Inline),
        color,
        background: specified
            .background
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Color::TRANSPARENT),
        gradient: specified.gradient.as_ref().map(|item| item.value),
        font_size,
        bold: specified
            .bold
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.bold),
        mono: specified
            .mono
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.mono),
        font_family: specified
            .font_family
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_else(|| parent.font_family.clone()),
        text_align: specified
            .text_align
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.text_align),
        underline: specified
            .underline
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.underline),
        white_space: specified
            .white_space
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.white_space),
        width: specified
            .width
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        height: specified
            .height
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        margin: take_sides(&specified.margin, Length::Px(0.0)),
        padding: take_sides(&specified.padding, Length::Px(0.0)),
        border_width: specified
            .border_width
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(0.0),
        borders: border_sides(specified),
        before: None,
        after: None,
        border_color: specified
            .border_color
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(color),
        line_height: specified
            .line_height
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.line_height),
        position: specified
            .position
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Position::Static),
        insets: take_sides(&specified.insets, Length::Auto),
        min_width: specified
            .min_width
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        max_width: specified
            .max_width
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        min_height: specified
            .min_height
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        max_height: specified
            .max_height
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        opacity: specified
            .opacity
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0),
        flex_direction: specified
            .flex_direction
            .as_ref()
            .map(|item| item.value.0)
            .unwrap_or(FlexDirection::Row),
        flex_reverse: specified
            .flex_direction
            .as_ref()
            .is_some_and(|item| item.value.1),
        gap: specified.gap.as_ref().map(|item| item.value).unwrap_or(0.0),
        row_gap: specified
            .row_gap
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(0.0),
        flex_wrap: specified
            .flex_wrap
            .as_ref()
            .is_some_and(|item| item.value.0),
        flex_wrap_reverse: specified
            .flex_wrap
            .as_ref()
            .is_some_and(|item| item.value.1),
        justify: specified
            .justify
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Justify::Start),
        align_items: specified
            .align_items
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(AlignItems::Stretch),
        align_self: specified.align_self.as_ref().map(|item| item.value),
        flex_grow: specified
            .flex_grow
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(0.0),
        flex_shrink: specified
            .flex_shrink
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(1.0),
        flex_basis: specified
            .flex_basis
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Length::Auto),
        box_sizing: specified
            .box_sizing
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(BoxSizing::ContentBox),
        overflow_hidden: specified
            .overflow_hidden
            .as_ref()
            .is_some_and(|item| item.value),
        border_radius: specified
            .border_radius
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(0.0),
        visible: specified
            .visible
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(true),
        z_index: specified
            .z_index
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(0),
        text_transform: specified
            .text_transform
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(parent.text_transform),
        float: specified
            .float
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(FloatSide::None),
        clear: specified
            .clear
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Clear::None),
        grid_columns: specified
            .grid_columns
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_default(),
        grid_rows: specified
            .grid_rows
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_default(),
        column_flow: specified
            .column_flow
            .as_ref()
            .is_some_and(|item| item.value),
        auto_columns: specified
            .auto_columns
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_default(),
        grid_areas: specified
            .grid_areas
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_default(),
        grid_area: specified
            .grid_area
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or_default(),
        grid_column: specified
            .grid_column
            .as_ref()
            .map(|item| item.value)
            .unwrap_or_default(),
        grid_row: specified
            .grid_row
            .as_ref()
            .map(|item| item.value)
            .unwrap_or_default(),
        shadow: specified.shadow.as_ref().map(|item| item.value),
    };
    apply_presentational_hints(doc, id, &mut computed);
    computed
}

fn rank_uniform_border(specified: &mut Specified, width: f32, spec: u32) {
    rank(&mut specified.border_width, width, spec);
    for slot in &mut specified.border_sides {
        rank(slot, width, spec);
    }
}

fn set_border_side(sides: &mut [Option<Ranked<f32>>; 4], index: usize, value: &str, spec: u32) {
    if let Some(width) = parse_border_width(value) {
        rank(&mut sides[index], width.max(0.0), spec);
    }
}

fn set_border_edge(specified: &mut Specified, index: usize, value: &str, spec: u32) {
    if let Some((width, color)) = parse_border(value) {
        rank(&mut specified.border_sides[index], width, spec);
        if let Some(color) = color {
            rank(&mut specified.border_color, color, spec);
        }
    }
}

fn border_sides(specified: &Specified) -> [f32; 4] {
    let uniform = specified
        .border_width
        .as_ref()
        .map(|item| item.value)
        .unwrap_or(0.0);
    [
        specified.border_sides[0]
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(uniform),
        specified.border_sides[1]
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(uniform),
        specified.border_sides[2]
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(uniform),
        specified.border_sides[3]
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(uniform),
    ]
}

fn parse_border_widths(value: &str) -> Option<[f32; 4]> {
    let sides: Vec<f32> = value
        .split_whitespace()
        .filter_map(parse_border_width)
        .collect();
    match sides.as_slice() {
        [one] => Some([*one, *one, *one, *one]),
        [top, right] => Some([*top, *right, *top, *right]),
        [top, right, bottom] => Some([*top, *right, *bottom, *right]),
        [top, right, bottom, left, ..] => Some([*top, *right, *bottom, *left]),
        _ => None,
    }
}

fn build_generated(
    doc: &Document,
    id: NodeId,
    ua: &Stylesheet,
    author: &Stylesheet,
    state: StyleState,
    vars: &HashMap<String, String>,
    element: &ComputedStyle,
    before: bool,
) -> Option<Generated> {
    let mut content: Option<Ranked<String>> = None;
    let mut hidden: Option<u32> = None;
    let mut color = None;
    let mut background = None;
    let mut bold = None;
    let mut font_size = None;
    let mut width = None;
    let mut height = None;
    for sheet in [ua, author] {
        for rule in &sheet.rules {
            if !media_applies(rule.media.as_ref(), state.viewport_width) {
                continue;
            }
            for selector in &rule.selectors {
                let Some(stripped) = strip_generated(selector, before) else {
                    continue;
                };
                if !matches_selector(doc, id, &stripped, state) {
                    continue;
                }
                let spec = selector.specificity();
                for declaration in &rule.declarations {
                    if declaration.name.starts_with("--") {
                        continue;
                    }
                    let expanded = expand_vars(&declaration.value, vars);
                    let value = expanded.as_str();
                    let important = if declaration.important { 100_000 } else { 0 };
                    let spec = spec + important;
                    match declaration.name.as_str() {
                        "content" => match parse_content(value, doc, id) {
                            Some(None) => {
                                if hidden.is_none_or(|current| spec >= current) {
                                    hidden = Some(spec);
                                    content = None;
                                }
                            }
                            Some(Some(text)) => {
                                if hidden.is_none_or(|current| spec >= current) {
                                    hidden = None;
                                    rank(&mut content, text, spec);
                                }
                            }
                            None => {}
                        },
                        "color" => {
                            if let Some(parsed) = parse_color(value) {
                                rank(&mut color, parsed, spec);
                            }
                        }
                        "background" | "background-color" => {
                            if let Some(parsed) = parse_color(value) {
                                rank(&mut background, parsed, spec);
                            }
                        }
                        "font-weight" => {
                            if let Some(parsed) = parse_bold(value) {
                                rank(&mut bold, parsed, spec);
                            }
                        }
                        "font-size" => {
                            if let Some(size) = parse_font_size(value) {
                                rank(&mut font_size, size, spec);
                            }
                        }
                        "width" => {
                            if let Some(length) = parse_length(value) {
                                rank(&mut width, length, spec);
                            }
                        }
                        "height" => {
                            if let Some(length) = parse_length(value) {
                                rank(&mut height, length, spec);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let text = content?.value;
    let font_size = match font_size.as_ref().map(|item| item.value) {
        Some(FontSize::Px(value)) => value.max(1.0),
        Some(FontSize::Em(value)) => (element.font_size * value).max(1.0),
        Some(FontSize::Percent(value)) => (element.font_size * value / 100.0).max(1.0),
        None => element.font_size,
    };
    Some(Generated {
        text,
        color: color
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(element.color),
        background: background
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(Color::TRANSPARENT),
        font_size,
        bold: bold.as_ref().map(|item| item.value).unwrap_or(element.bold),
        width: width.as_ref().and_then(|item| match item.value {
            Length::Px(px) => Some(px.max(0.0)),
            Length::Em(em) => Some((em * font_size).max(0.0)),
            Length::Rem(rem) => Some((rem * 16.0).max(0.0)),
            _ => None,
        }),
        height: height.as_ref().and_then(|item| match item.value {
            Length::Px(px) => Some(px.max(0.0)),
            Length::Em(em) => Some((em * font_size).max(0.0)),
            Length::Rem(rem) => Some((rem * 16.0).max(0.0)),
            _ => None,
        }),
    })
}

fn strip_generated(selector: &mithu_css::Selector, before: bool) -> Option<mithu_css::Selector> {
    let mut selector = selector.clone();
    let last = selector.compounds.last_mut()?;
    let wanted = if before {
        Pseudo::Before
    } else {
        Pseudo::After
    };
    if !last.pseudos.contains(&wanted) {
        return None;
    }
    if last
        .pseudos
        .iter()
        .any(|pseudo| matches!(pseudo, Pseudo::Before | Pseudo::After) && pseudo != &wanted)
    {
        return None;
    }
    last.pseudos
        .retain(|pseudo| !matches!(pseudo, Pseudo::Before | Pseudo::After));
    Some(selector)
}

fn parse_content(value: &str, doc: &Document, id: NodeId) -> Option<Option<String>> {
    let mut rest = value.trim();
    if rest.eq_ignore_ascii_case("none") || rest.eq_ignore_ascii_case("normal") {
        return Some(None);
    }
    let mut text = String::new();
    let mut saw = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let quote = rest.chars().next()?;
        if quote == '"' || quote == '\'' {
            let mut chars = rest[1..].chars();
            let mut closed = false;
            while let Some(ch) = chars.next() {
                if ch == '\\' {
                    if let Some(escaped) = chars.next() {
                        text.push(escaped);
                    }
                    continue;
                }
                if ch == quote {
                    closed = true;
                    break;
                }
                text.push(ch);
            }
            if !closed {
                return None;
            }
            let consumed = rest.len() - chars.as_str().len();
            rest = &rest[consumed..];
            saw = true;
            continue;
        }
        let lower = rest.to_ascii_lowercase();
        if let Some(inner) = lower.strip_prefix("attr(") {
            let end = inner.find(')')?;
            let name = rest[5..5 + end].trim();
            text.push_str(doc.attr(id, name).unwrap_or(""));
            rest = &rest[5 + end + 1..];
            saw = true;
            continue;
        }
        return None;
    }
    saw.then_some(Some(text))
}

fn take_sides(sides: &[Option<Ranked<Length>>; 4], default: Length) -> [Length; 4] {
    [
        sides[0].as_ref().map(|item| item.value).unwrap_or(default),
        sides[1].as_ref().map(|item| item.value).unwrap_or(default),
        sides[2].as_ref().map(|item| item.value).unwrap_or(default),
        sides[3].as_ref().map(|item| item.value).unwrap_or(default),
    ]
}

fn apply_presentational_hints(doc: &Document, id: NodeId, style: &mut ComputedStyle) {
    let Some(name) = doc.element_name(id) else {
        return;
    };
    if name == "input"
        && doc
            .attr(id, "type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("hidden"))
    {
        style.display = Display::None;
        return;
    }
    let width_attr = attr_length(doc, id, "width");
    let height_attr = attr_length(doc, id, "height");
    let fill = |length: Length, attr: Option<Length>, fallback: f32| -> Length {
        if !matches!(length, Length::Auto) {
            return length;
        }
        attr.unwrap_or(Length::Px(fallback))
    };
    match name {
        "img" => {
            if let Some(width) = width_attr
                && matches!(style.width, Length::Auto)
            {
                style.width = width;
            }
            if let Some(height) = height_attr
                && matches!(style.height, Length::Auto)
            {
                style.height = height;
            }
        }
        "iframe" | "video" | "canvas" | "svg" => {
            style.width = fill(style.width, width_attr, 300.0);
            style.height = fill(style.height, height_attr, 150.0);
        }
        "audio" => {
            style.width = fill(style.width, width_attr, 300.0);
            style.height = fill(style.height, height_attr, 40.0);
        }
        "textarea" => {
            style.width = fill(style.width, width_attr, 240.0);
            style.height = fill(style.height, height_attr, 80.0);
        }
        "select" => {
            style.width = fill(style.width, width_attr, 180.0);
            style.height = fill(style.height, height_attr, 28.0);
        }
        "input" => {
            let kind = doc.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
            if matches!(kind.as_str(), "checkbox" | "radio") {
                style.width = fill(style.width, width_attr, 18.0);
                style.height = fill(style.height, height_attr, 18.0);
            } else if !matches!(kind.as_str(), "submit" | "button" | "reset") {
                style.width = fill(style.width, width_attr, 180.0);
                style.height = fill(style.height, height_attr, 28.0);
            }
        }
        _ => {
            if matches!(style.width, Length::Auto)
                && let Some(width) = width_attr
            {
                style.width = width;
            }
            if matches!(style.height, Length::Auto)
                && let Some(height) = height_attr
            {
                style.height = height;
            }
        }
    }
}

fn attr_length(doc: &Document, id: NodeId, name: &str) -> Option<Length> {
    let value = doc.attr(id, name)?;
    parse_length(value).or_else(|| value.parse::<f32>().ok().map(Length::Px))
}

fn parse_display(value: &str) -> Option<Display> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "block" => Display::Block,
        "inline" => Display::Inline,
        "inline-block" => Display::InlineBlock,
        "list-item" => Display::ListItem,
        "flex" => Display::Flex,
        "inline-flex" => Display::InlineFlex,
        "grid" | "inline-grid" => Display::Grid,
        "contents" => Display::Contents,
        "none" => Display::None,
        _ => return None,
    })
}

fn parse_font_size(value: &str) -> Option<FontSize> {
    let value = value.trim().to_ascii_lowercase();
    let keyword = match value.as_str() {
        "xx-small" => Some(10.0),
        "x-small" => Some(12.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "smaller" => return Some(FontSize::Em(0.85)),
        "larger" => return Some(FontSize::Em(1.2)),
        _ => None,
    };
    if let Some(px) = keyword {
        return Some(FontSize::Px(px));
    }
    match parse_length(&value)? {
        Length::Px(px) => Some(FontSize::Px(px)),
        Length::Em(em) => Some(FontSize::Em(em)),
        Length::Rem(rem) => Some(FontSize::Px(rem * 16.0)),
        Length::Percent(percent) => Some(FontSize::Percent(percent)),
        Length::Auto | Length::Vw(_) | Length::Vh(_) | Length::Calc { .. } => None,
    }
}

fn parse_position(value: &str) -> Option<Position> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "static" => Position::Static,
        "relative" => Position::Relative,
        "absolute" => Position::Absolute,
        "sticky" => Position::Sticky,
        "fixed" => Position::Fixed,
        _ => return None,
    })
}

fn parse_float(value: &str) -> Option<FloatSide> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "left" => FloatSide::Left,
        "right" => FloatSide::Right,
        "none" => FloatSide::None,
        _ => return None,
    })
}

fn parse_clear(value: &str) -> Option<Clear> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "left" => Clear::Left,
        "right" => Clear::Right,
        "both" => Clear::Both,
        "none" => Clear::None,
        _ => return None,
    })
}

fn parse_grid_span(value: &str) -> Option<GridSpan> {
    let value = value.trim().to_ascii_lowercase();
    // `span 12`, `7 / span 12`, `1 / 13`, and a bare line `2`.
    if let Some((start, end)) = value.split_once('/') {
        let start = start.trim();
        let end = end.trim();
        let line = start.parse::<u16>().ok().filter(|line| *line > 0);
        let span = if let Some(rest) = end.strip_prefix("span") {
            rest.trim().parse::<u16>().ok()?
        } else if let Some(rest) = start.strip_prefix("span") {
            let _end_line = end.parse::<u16>().ok();
            rest.trim().parse::<u16>().ok()?
        } else {
            let end_line: i32 = end.parse().ok()?;
            let start_line = line? as i32;
            if end_line <= start_line {
                return None;
            }
            (end_line - start_line) as u16
        };
        return Some(GridSpan {
            start: line,
            span: span.max(1),
        });
    }
    if let Some(rest) = value.strip_prefix("span") {
        let span = rest.trim().parse::<u16>().ok()?;
        return Some(GridSpan {
            start: None,
            span: span.max(1),
        });
    }
    let line = value.split_whitespace().next()?.parse::<u16>().ok()?;
    Some(GridSpan {
        start: Some(line.max(1)),
        span: 1,
    })
}

fn parse_box_shadow(value: &str) -> Option<BoxShadow> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return None;
    }
    let first = first_shadow(value);
    let mut color = Color::rgba(0, 0, 0, 76);
    let mut lengths = Vec::new();
    for token in shadow_tokens(first) {
        if token.eq_ignore_ascii_case("inset") {
            continue;
        }
        if let Some(parsed) = parse_color(&token) {
            color = parsed;
            continue;
        }
        if let Some(Length::Px(px)) = parse_length(&token) {
            lengths.push(px);
        }
    }
    if lengths.is_empty() {
        return None;
    }
    Some(BoxShadow {
        x: lengths.first().copied().unwrap_or(0.0),
        y: lengths.get(1).copied().unwrap_or(0.0),
        blur: lengths.get(2).copied().unwrap_or(0.0).max(0.0),
        spread: lengths.get(3).copied().unwrap_or(0.0),
        color,
    })
}

fn first_shadow(value: &str) -> &str {
    let mut depth = 0;
    for (index, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ',' if depth == 0 => return &value[..index],
            _ => {}
        }
    }
    value
}

fn shadow_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ch if ch.is_whitespace() && depth == 0 => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

fn parse_grid_template(value: &str) -> Option<(Vec<GridTrack>, Vec<GridTrack>)> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Some((Vec::new(), Vec::new()));
    }
    let mut depth = 0i32;
    let mut split = None;
    for (index, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            '/' if depth == 0 => {
                split = Some(index);
                break;
            }
            _ => {}
        }
    }
    let Some(at) = split else {
        return Some((parse_grid_tracks(value)?, Vec::new()));
    };
    Some((
        parse_grid_tracks(value[..at].trim())?,
        parse_grid_tracks(value[at + 1..].trim())?,
    ))
}

fn parse_grid_areas(value: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in value.chars() {
        match ch {
            '\'' | '"' if !quoted => {
                quoted = true;
                current.clear();
            }
            '\'' | '"' if quoted => {
                quoted = false;
                let cells = current
                    .split_whitespace()
                    .map(|cell| {
                        if cell == "." {
                            String::new()
                        } else {
                            cell.to_string()
                        }
                    })
                    .collect::<Vec<_>>();
                if !cells.is_empty() {
                    rows.push(cells);
                }
                current.clear();
            }
            _ if quoted => current.push(ch),
            _ => {}
        }
    }
    rows
}

fn parse_grid_tracks(value: &str) -> Option<Vec<GridTrack>> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let mut tokens = tokenize_tracks(value);
    let mut tracks = Vec::new();
    while !tokens.is_empty() {
        if tokens[0].starts_with('[') {
            while !tokens.is_empty() {
                let token = tokens.remove(0);
                if token.ends_with(']') {
                    break;
                }
            }
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("repeat") {
            tokens.remove(0);
            if tokens.first().is_some_and(|token| token == "(") {
                tokens.remove(0);
            }
            let count: usize = tokens.first()?.parse().ok()?;
            tokens.remove(0);
            if tokens.first().is_some_and(|token| token == ",") {
                tokens.remove(0);
            }
            let mut inner = Vec::new();
            while let Some(token) = tokens.first() {
                if token == ")" {
                    tokens.remove(0);
                    break;
                }
                inner.push(parse_track(token)?);
                tokens.remove(0);
            }
            if inner.is_empty() || count > 64 {
                return None;
            }
            for _ in 0..count {
                tracks.extend(inner.iter().cloned());
            }
        } else if tokens[0].eq_ignore_ascii_case("minmax")
            || tokens[0].eq_ignore_ascii_case("fit-content")
        {
            let kind = tokens.remove(0).to_ascii_lowercase();
            if tokens.first().is_some_and(|token| token == "(") {
                tokens.remove(0);
            }
            let mut inner = Vec::new();
            while let Some(token) = tokens.first() {
                if token == ")" {
                    tokens.remove(0);
                    break;
                }
                if token == "," {
                    tokens.remove(0);
                    continue;
                }
                inner.push(tokens.remove(0));
            }
            let track = if kind == "fit-content" {
                inner.first().and_then(|token| parse_track(token))
            } else {
                inner
                    .iter()
                    .rev()
                    .find_map(|token| parse_track(token))
                    .or(Some(GridTrack::Auto))
            };
            tracks.push(track?);
        } else {
            tracks.push(parse_track(&tokens.remove(0))?);
        }
    }
    Some(tracks)
}

fn tokenize_tracks(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch == '(' || ch == ')' || ch == ',' {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
            }
            current.clear();
            tokens.push(ch.to_string());
        } else if ch.is_whitespace() {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

fn parse_track(token: &str) -> Option<GridTrack> {
    let token = token.trim().to_ascii_lowercase();
    if token == "auto" || token == "min-content" || token == "max-content" {
        return Some(GridTrack::Auto);
    }
    if let Some(number) = token.strip_suffix("fr") {
        let fr = number.trim().parse::<f32>().ok()?;
        return Some(GridTrack::Fr(fr.max(0.0)));
    }
    match parse_length(&token)? {
        Length::Px(px) => Some(GridTrack::Px(px.max(0.0))),
        // Grid tracks are resolved against the root font (16px), the same
        // default Chromium uses before a page sets a root size.
        Length::Rem(value) | Length::Em(value) => Some(GridTrack::Px((value * 16.0).max(0.0))),
        Length::Vw(value) => Some(GridTrack::Px((value * 12.8).max(0.0))),
        Length::Vh(value) => Some(GridTrack::Px((value * 8.0).max(0.0))),
        Length::Percent(value) => Some(GridTrack::Px(value.max(0.0))),
        Length::Calc { px, rem, em, .. } => Some(GridTrack::Px((px + (rem + em) * 16.0).max(0.0))),
        Length::Auto => Some(GridTrack::Auto),
    }
}

fn parse_flex_direction(value: &str) -> Option<(FlexDirection, bool)> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "row" => (FlexDirection::Row, false),
        "row-reverse" => (FlexDirection::Row, true),
        "column" => (FlexDirection::Column, false),
        "column-reverse" => (FlexDirection::Column, true),
        _ => return None,
    })
}

fn parse_justify(value: &str) -> Option<Justify> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "flex-start" | "start" | "left" => Justify::Start,
        "flex-end" | "end" | "right" => Justify::End,
        "center" => Justify::Center,
        "space-between" => Justify::SpaceBetween,
        "space-around" | "space-evenly" => Justify::Center,
        _ => return None,
    })
}

fn parse_align_items(value: &str) -> Option<AlignItems> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "stretch" => AlignItems::Stretch,
        "flex-start" | "start" => AlignItems::Start,
        "center" => AlignItems::Center,
        "flex-end" | "end" => AlignItems::End,
        _ => return None,
    })
}

fn parse_box_sizing(value: &str) -> Option<BoxSizing> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "border-box" => BoxSizing::BorderBox,
        "content-box" => BoxSizing::ContentBox,
        _ => return None,
    })
}

fn parse_opacity(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .map(|number| number / 100.0);
    }
    value.parse().ok()
}

fn parse_bold(value: &str) -> Option<bool> {
    let value = value.trim().to_ascii_lowercase();
    if matches!(value.as_str(), "bold" | "bolder") {
        return Some(true);
    }
    if matches!(value.as_str(), "normal" | "lighter") {
        return Some(false);
    }
    value.parse::<u16>().ok().map(|weight| weight >= 600)
}

fn parse_align(value: &str) -> Option<TextAlign> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => TextAlign::Left,
        "right" | "end" => TextAlign::Right,
        "center" => TextAlign::Center,
        "justify" => TextAlign::Left,
        _ => return None,
    })
}

fn parse_underline(value: &str) -> bool {
    let mut underline = false;
    for token in value.split_whitespace() {
        match token.to_ascii_lowercase().as_str() {
            "underline" => underline = true,
            "none" => underline = false,
            _ => {}
        }
    }
    underline
}

fn parse_white_space(value: &str) -> Option<WhiteSpace> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "pre" | "pre-wrap" | "break-spaces" => WhiteSpace::Pre,
        "nowrap" => WhiteSpace::Nowrap,
        "normal" => WhiteSpace::Normal,
        _ => return None,
    })
}

fn parse_line_height(value: &str) -> Option<LineHeight> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("normal") {
        return Some(LineHeight::Normal);
    }
    if let Some(length) = parse_length(value) {
        return Some(match length {
            Length::Px(px) => LineHeight::Px(px),
            Length::Percent(percent) => LineHeight::Percent(percent),
            Length::Em(em) => LineHeight::Number(em),
            Length::Rem(rem) => LineHeight::Px(rem * 16.0),
            Length::Auto | Length::Vw(_) | Length::Vh(_) | Length::Calc { .. } => return None,
        });
    }
    value.parse::<f32>().ok().map(LineHeight::Number)
}

pub fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") {
        return Some(Length::Auto);
    }
    if value == "0" {
        return Some(Length::Px(0.0));
    }
    if let Some(number) = value.strip_suffix("px") {
        return number.trim().parse().ok().map(Length::Px);
    }
    if let Some(number) = value.strip_suffix('%') {
        return number.trim().parse().ok().map(Length::Percent);
    }
    if let Some(length) = parse_calc(value) {
        return Some(length);
    }
    if let Some(number) = value.strip_suffix("rem") {
        return number.trim().parse().ok().map(Length::Rem);
    }
    if let Some(number) = value.strip_suffix("em") {
        return number.trim().parse().ok().map(Length::Em);
    }
    if let Some(number) = value.strip_suffix("vw") {
        return number.trim().parse().ok().map(Length::Vw);
    }
    if let Some(number) = value.strip_suffix("vh") {
        return number.trim().parse().ok().map(Length::Vh);
    }
    None
}

fn parse_calc(value: &str) -> Option<Length> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("calc(") || !lower.ends_with(')') {
        return None;
    }
    let inner = &trimmed[5..trimmed.len() - 1];
    let mut parts = Vec::new();
    let mut sign = 1.0;
    let mut current = String::new();
    let mut depth = 0;
    for ch in inner.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            '+' if depth == 0 => {
                parts.push((sign, std::mem::take(&mut current)));
                sign = 1.0;
            }
            '-' if depth == 0 && !current.trim().is_empty() => {
                parts.push((sign, std::mem::take(&mut current)));
                sign = -1.0;
            }
            _ => current.push(ch),
        }
    }
    parts.push((sign, current));
    let mut percent = 0.0;
    let mut px = 0.0;
    let mut em = 0.0;
    let mut rem = 0.0;
    let mut vw = 0.0;
    let mut vh = 0.0;
    for (sign, term) in parts {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let (factor, length) = term_length(term)?;
        add_calc(
            &mut percent,
            &mut px,
            &mut em,
            &mut rem,
            &mut vw,
            &mut vh,
            length,
            sign * factor,
        );
    }
    Some(simplify_calc(percent, px, em, rem, vw, vh))
}

fn term_length(term: &str) -> Option<(f32, Length)> {
    if let Some((left, right)) = split_top(term, '*') {
        let (number, expr) = if let Some(number) = left.trim().parse::<f32>().ok() {
            (number, right.trim())
        } else {
            (right.trim().parse::<f32>().ok()?, left.trim())
        };
        return Some((number, parse_length(expr)?));
    }
    if let Some((left, right)) = split_top(term, '/') {
        let number = right.trim().parse::<f32>().ok()?;
        if number == 0.0 {
            return None;
        }
        return Some((1.0 / number, parse_length(left.trim())?));
    }
    Some((1.0, parse_length(term)?))
}

fn split_top<'a>(input: &'a str, op: char) -> Option<(&'a str, &'a str)> {
    let mut depth = 0;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if ch == op && depth == 0 => {
                return Some((&input[..index], &input[index + ch.len_utf8()..]));
            }
            _ => {}
        }
    }
    None
}

fn add_calc(
    percent: &mut f32,
    px: &mut f32,
    em: &mut f32,
    rem: &mut f32,
    vw: &mut f32,
    vh: &mut f32,
    length: Length,
    factor: f32,
) {
    match length {
        Length::Px(value) => *px += value * factor,
        Length::Percent(value) => *percent += value * factor,
        Length::Em(value) => *em += value * factor,
        Length::Rem(value) => *rem += value * factor,
        Length::Vw(value) => *vw += value * factor,
        Length::Vh(value) => *vh += value * factor,
        Length::Calc {
            percent: more_percent,
            px: more_px,
            em: more_em,
            rem: more_rem,
            vw: more_vw,
            vh: more_vh,
        } => {
            *percent += more_percent * factor;
            *px += more_px * factor;
            *em += more_em * factor;
            *rem += more_rem * factor;
            *vw += more_vw * factor;
            *vh += more_vh * factor;
        }
        Length::Auto => {}
    }
}

fn simplify_calc(percent: f32, px: f32, em: f32, rem: f32, vw: f32, vh: f32) -> Length {
    if percent == 0.0 && em == 0.0 && rem == 0.0 && vw == 0.0 && vh == 0.0 {
        Length::Px(px)
    } else if px == 0.0 && em == 0.0 && rem == 0.0 && vw == 0.0 && vh == 0.0 {
        Length::Percent(percent)
    } else if percent == 0.0 && px == 0.0 && rem == 0.0 && vw == 0.0 && vh == 0.0 {
        Length::Em(em)
    } else if percent == 0.0 && px == 0.0 && em == 0.0 && vw == 0.0 && vh == 0.0 {
        Length::Rem(rem)
    } else if percent == 0.0 && px == 0.0 && em == 0.0 && rem == 0.0 && vh == 0.0 {
        Length::Vw(vw)
    } else if percent == 0.0 && px == 0.0 && em == 0.0 && rem == 0.0 && vw == 0.0 {
        Length::Vh(vh)
    } else {
        Length::Calc {
            percent,
            px,
            em,
            rem,
            vw,
            vh,
        }
    }
}

fn px_length(value: &str) -> Option<f32> {
    match parse_length(value)? {
        Length::Px(px) => Some(px.max(0.0)),
        Length::Em(em) => Some((em * 16.0).max(0.0)),
        Length::Rem(rem) => Some((rem * 16.0).max(0.0)),
        Length::Percent(_) | Length::Vw(_) | Length::Vh(_) | Length::Calc { .. } | Length::Auto => {
            None
        }
    }
}

fn parse_gap(value: &str) -> Option<(f32, f32)> {
    let sides: Vec<f32> = value.split_whitespace().filter_map(px_length).collect();
    match sides.as_slice() {
        [one] => Some((*one, *one)),
        [row, column, ..] => Some((*row, *column)),
        _ => None,
    }
}

fn media_applies(media: Option<&mithu_css::Media>, width: f32) -> bool {
    let Some(media) = media else {
        return true;
    };
    media.alternatives.iter().any(|bound| {
        bound.min_width.is_none_or(|min| width + 0.01 >= min)
            && bound.max_width.is_none_or(|max| width <= max + 0.01)
    })
}

fn parse_text_transform(value: &str) -> Option<TextTransform> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "none" => TextTransform::None,
        "uppercase" => TextTransform::Uppercase,
        "lowercase" => TextTransform::Lowercase,
        "capitalize" => TextTransform::Capitalize,
        _ => return None,
    })
}

fn parse_flex_wrap(value: &str) -> Option<(bool, bool)> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "nowrap" => (false, false),
        "wrap" => (true, false),
        "wrap-reverse" => (true, true),
        _ => return None,
    })
}

fn parse_sides(value: &str) -> Option<[Length; 4]> {
    let sides: Vec<Length> = value.split_whitespace().filter_map(parse_length).collect();
    match sides.as_slice() {
        [a] => Some([*a, *a, *a, *a]),
        [a, b] => Some([*a, *b, *a, *b]),
        [a, b, c] => Some([*a, *b, *c, *b]),
        [a, b, c, d, ..] => Some([*a, *b, *c, *d]),
        _ => None,
    }
}

fn parse_border(value: &str) -> Option<(f32, Option<Color>)> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Some((0.0, None));
    }
    let mut width = None;
    let mut color = None;
    for token in value.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset"
        ) {
            continue;
        }
        if let Some(px) = parse_border_width(token) {
            width = Some(px);
        } else if let Some(parsed) = parse_color_token(token) {
            color = Some(parsed);
        }
    }
    if width.is_none() && color.is_none() {
        return None;
    }
    Some((width.unwrap_or(3.0), color))
}

fn parse_border_width(value: &str) -> Option<f32> {
    match value.trim().to_ascii_lowercase().as_str() {
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        "0" => Some(0.0),
        _ => match parse_length(value)? {
            Length::Px(px) => Some(px),
            Length::Em(em) => Some(em * 16.0),
            Length::Rem(rem) => Some(rem * 16.0),
            Length::Percent(_)
            | Length::Vw(_)
            | Length::Vh(_)
            | Length::Calc { .. }
            | Length::Auto => None,
        },
    }
}

fn parse_linear_gradient(value: &str) -> Option<(Color, Color)> {
    let lower = value.to_ascii_lowercase();
    let start = lower.find("linear-gradient(")?;
    let body = &value[start + "linear-gradient(".len()..];
    let end = matching_paren(body)?;
    let mut colors = Vec::new();
    for argument in split_top_level(&body[..end]) {
        let piece = argument.trim();
        if piece.is_empty() {
            continue;
        }
        let token = piece.split_whitespace().next().unwrap_or(piece);
        if let Some(color) = parse_color(token).or_else(|| parse_color(piece)) {
            colors.push(color);
        }
    }
    if colors.len() >= 2 {
        Some((colors[0], colors[colors.len() - 1]))
    } else {
        None
    }
}

fn split_top_level(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&input[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(color) = parse_color_token(value) {
        return Some(color);
    }
    if let Some(color) = parse_rgb(value) {
        return Some(color);
    }
    if let Some(color) = parse_hsl(value) {
        return Some(color);
    }
    value.split_whitespace().find_map(parse_color_token)
}

fn parse_color_token(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(color) = parse_hex(value) {
        return Some(color);
    }
    if let Some(color) = parse_rgb(value) {
        return Some(color);
    }
    Some(match value.to_ascii_lowercase().as_str() {
        "transparent" => Color::TRANSPARENT,
        "black" => Color::BLACK,
        "white" => Color::WHITE,
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "navy" => Color::rgb(0, 0, 128),
        "teal" => Color::rgb(0, 128, 128),
        "purple" => Color::rgb(128, 0, 128),
        "orange" => Color::rgb(255, 165, 0),
        "yellow" => Color::rgb(255, 255, 0),
        "maroon" => Color::rgb(128, 0, 0),
        "olive" => Color::rgb(128, 128, 0),
        "lime" => Color::rgb(0, 255, 0),
        "aqua" => Color::rgb(0, 255, 255),
        "fuchsia" => Color::rgb(255, 0, 255),
        "whitesmoke" => Color::rgb(245, 245, 245),
        "pink" => Color::rgb(255, 192, 203),
        "brown" => Color::rgb(165, 42, 42),
        "gold" => Color::rgb(255, 215, 0),
        "coral" => Color::rgb(255, 127, 80),
        "crimson" => Color::rgb(220, 20, 60),
        "indigo" => Color::rgb(75, 0, 130),
        "violet" => Color::rgb(238, 130, 238),
        "cyan" => Color::rgb(0, 255, 255),
        "magenta" => Color::rgb(255, 0, 255),
        "beige" => Color::rgb(245, 245, 220),
        "khaki" => Color::rgb(240, 230, 140),
        "tomato" => Color::rgb(255, 99, 71),
        "chocolate" => Color::rgb(210, 105, 30),
        "darkgray" | "darkgrey" => Color::rgb(169, 169, 169),
        "lightgray" | "lightgrey" => Color::rgb(211, 211, 211),
        "dimgray" | "dimgrey" => Color::rgb(105, 105, 105),
        "darkred" => Color::rgb(139, 0, 0),
        "darkgreen" => Color::rgb(0, 100, 0),
        "darkblue" => Color::rgb(0, 0, 139),
        "lightblue" => Color::rgb(173, 216, 230),
        "skyblue" => Color::rgb(135, 206, 235),
        "royalblue" => Color::rgb(65, 105, 225),
        "steelblue" => Color::rgb(70, 130, 180),
        "forestgreen" => Color::rgb(34, 139, 34),
        "seagreen" => Color::rgb(46, 139, 87),
        "limegreen" => Color::rgb(50, 205, 50),
        "orangered" => Color::rgb(255, 69, 0),
        "darkorange" => Color::rgb(255, 140, 0),
        "salmon" => Color::rgb(250, 128, 114),
        "tan" => Color::rgb(210, 180, 140),
        "wheat" => Color::rgb(245, 222, 179),
        "ivory" => Color::rgb(255, 255, 240),
        "azure" => Color::rgb(240, 255, 255),
        "lavender" => Color::rgb(230, 230, 250),
        "plum" => Color::rgb(221, 160, 221),
        "turquoise" => Color::rgb(64, 224, 208),
        "rebeccapurple" => Color::rgb(102, 51, 153),
        _ => return None,
    })
}

fn parse_hex(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    let chars: Vec<char> = hex.chars().collect();
    let pair = |index: usize| -> Option<u8> {
        let hi = chars.get(index)?.to_digit(16)? as u8;
        let lo = chars.get(index + 1)?.to_digit(16)? as u8;
        Some((hi << 4) | lo)
    };
    let nibble = |index: usize| -> Option<u8> {
        let digit = chars.get(index)?.to_digit(16)? as u8;
        Some((digit << 4) | digit)
    };
    match chars.len() {
        3 | 4 => Some(Color::rgba(
            nibble(0)?,
            nibble(1)?,
            nibble(2)?,
            if chars.len() == 4 { nibble(3)? } else { 255 },
        )),
        6 | 8 => Some(Color::rgba(
            pair(0)?,
            pair(2)?,
            pair(4)?,
            if chars.len() == 8 { pair(6)? } else { 255 },
        )),
        _ => None,
    }
}

fn parse_rgb(value: &str) -> Option<Color> {
    let lower = value.trim().to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let inner = inner.replace('/', " ").replace(',', " ");
    let numbers: Vec<&str> = inner.split_whitespace().take(4).collect();
    if numbers.len() < 3 {
        return None;
    }
    let channel = |token: &str| -> Option<u8> {
        if let Some(percent) = token.strip_suffix('%') {
            let value = percent.trim().parse::<f32>().ok()?;
            return Some((value / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        let value = token.parse::<f32>().ok()?;
        Some(value.round().clamp(0.0, 255.0) as u8)
    };
    let alpha = match numbers.get(3) {
        Some(token) => parse_alpha(token)?,
        None => 255,
    };
    Some(Color::rgba(
        channel(numbers[0])?,
        channel(numbers[1])?,
        channel(numbers[2])?,
        alpha,
    ))
}

fn parse_alpha(token: &str) -> Option<u8> {
    if let Some(percent) = token.strip_suffix('%') {
        let value = percent.trim().parse::<f32>().ok()?;
        return Some((value / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    let value = token.parse::<f32>().ok()?;
    Some((value * 255.0).round().clamp(0.0, 255.0) as u8)
}

fn parse_hsl(value: &str) -> Option<Color> {
    let lower = value.trim().to_ascii_lowercase();
    let inner = lower
        .strip_prefix("hsla(")
        .or_else(|| lower.strip_prefix("hsl("))?
        .strip_suffix(')')?;
    let inner = inner.replace('/', " ").replace(',', " ");
    let numbers: Vec<&str> = inner.split_whitespace().take(4).collect();
    if numbers.len() < 3 {
        return None;
    }
    let hue = numbers[0].trim_end_matches("deg").parse::<f32>().ok()?;
    let saturation = numbers[1].strip_suffix('%')?.trim().parse::<f32>().ok()? / 100.0;
    let lightness = numbers[2].strip_suffix('%')?.trim().parse::<f32>().ok()? / 100.0;
    let alpha = match numbers.get(3) {
        Some(token) => parse_alpha(token)?,
        None => 255,
    };
    let (red, green, blue) = hsl_channels(hue, saturation, lightness);
    Some(Color::rgba(red, green, blue, alpha))
}

fn hsl_channels(hue: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let section = hue / 60.0;
    let x = chroma * (1.0 - (section.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match section as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let match_light = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + match_light) * 255.0).round().clamp(0.0, 255.0) as u8;
    (channel(red), channel(green), channel(blue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithu_html::parse_html;

    #[test]
    fn author_color_overrides_user_agent() {
        let doc = parse_html("<p style=\"color: rgb(1, 2, 3)\">Hi</p>");
        let styles = style_document(&doc, "p { color: red }");
        let p = doc.get_element_by_id("").or_else(|| {
            doc.nodes.iter().enumerate().find_map(|(index, _node)| {
                matches!(doc.element_name(NodeId(index)), Some("p")).then_some(NodeId(index))
            })
        });
        let p = p.expect("p");
        assert_eq!(styles[p.0].color, Color::rgb(1, 2, 3));
        assert_eq!(styles[p.0].display, Display::Block);
    }

    #[test]
    fn flex_position_and_attribute_selector() {
        let doc =
            parse_html(r#"<div id="row" class="row"><b id="one">A</b><i id="two">B</i></div>"#);
        let styles = style_document(
            &doc,
            r#"
            .row { display: flex; gap: 8px; opacity: 0.5; position: relative; }
            .row > :first-child { color: navy }
            b[id=one] + i { color: teal }
            "#,
        );
        let row = doc.get_element_by_id("row").unwrap();
        assert_eq!(styles[row.0].display, Display::Flex);
        assert_eq!(styles[row.0].gap, 8.0);
        assert_eq!(styles[row.0].opacity, 0.5);
        assert_eq!(styles[row.0].position, Position::Relative);
        let one = doc.get_element_by_id("one").unwrap();
        let two = doc.get_element_by_id("two").unwrap();
        assert_eq!(styles[one.0].color, Color::rgb(0, 0, 128));
        assert_eq!(styles[two.0].color, Color::rgb(0, 128, 128));
    }

    #[test]
    fn child_combinator_matches() {
        let doc = parse_html("<div><p class='a'>Hi</p></div>");
        let styles = style_document(&doc, "div > p.a { color: #00ff00 }");
        let p = doc
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| (doc.element_name(NodeId(index)) == Some("p")).then_some(index))
            .unwrap();
        assert_eq!(styles[p].color, Color::rgb(0, 255, 0));
    }

    #[test]
    fn custom_properties_calc_and_hover() {
        let doc = parse_html(
            r#"<div id="box" style="--ink: hsl(0, 100%, 50%)"><p id="text">Hi</p></div>"#,
        );
        let styles = style_document(
            &doc,
            "p { color: var(--ink); width: calc(100% - 20px) } #text { background: rgba(0, 0, 0, 0.5) }",
        );
        let text = doc.get_element_by_id("text").unwrap();
        assert_eq!(styles[text.0].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[text.0].background.a, 128);
        assert_eq!(
            styles[text.0].width,
            Length::Calc {
                percent: 100.0,
                px: -20.0,
                em: 0.0,
                rem: 0.0,
                vw: 0.0,
                vh: 0.0
            }
        );
        let idle = style_document(&doc, "#box:hover { color: tomato }");
        let box_id = doc.get_element_by_id("box").unwrap();
        assert_ne!(idle[box_id.0].color, Color::rgb(255, 99, 71));
        let hovered = style_with(
            &doc,
            "#box:hover { color: tomato }",
            StyleState {
                hover: Some(text),
                ..StyleState::default()
            },
        );
        assert_eq!(hovered[box_id.0].color, Color::rgb(255, 99, 71));
        assert!(
            style_document(
                &doc,
                "#box { flex-wrap: wrap; visibility: hidden; z-index: 2 }"
            )[box_id.0]
                .flex_wrap
        );
        assert!(!style_document(&doc, "#box { visibility: hidden }")[box_id.0].visible);
        let narrow = style_with(
            &doc,
            "@media (min-width: 900px) { #text { color: blue } } @media (max-width: 400px) { #text { color: gold } }",
            StyleState {
                viewport_width: 320.0,
                ..StyleState::default()
            },
        );
        assert_eq!(narrow[text.0].color, Color::rgb(255, 215, 0));
        assert_eq!(
            style_document(&doc, "#text { text-transform: uppercase }")[text.0].text_transform,
            TextTransform::Uppercase
        );
    }

    #[test]
    fn root_variables_and_logical_spacing() {
        let doc = parse_html(r#"<div id="bar" class="mx-auto h-14 flex"></div>"#);
        let styles = style_document(
            &doc,
            r#"
            @layer theme { :root { --spacing: .25rem; --ink: 210 40% 98%; } }
            @layer utilities {
              .mx-auto { margin-inline: auto }
              .h-14 { height: calc(var(--spacing) * 14) }
              .flex { display: flex }
              .bg { background-color: hsl(var(--ink)) }
            }
            "#,
        );
        let bar = doc.get_element_by_id("bar").unwrap();
        assert_eq!(styles[bar.0].display, Display::Flex);
        assert_eq!(styles[bar.0].margin[1], Length::Auto);
        assert_eq!(styles[bar.0].margin[3], Length::Auto);
        assert_eq!(styles[bar.0].height, Length::Rem(3.5));
        let html = doc
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (doc.element_name(NodeId(index)) == Some("html")).then_some(index)
            })
            .unwrap();
        let painted = style_document(
            &doc,
            "@layer theme { :root { --ink: 210 40% 98% } } html { background: hsl(var(--ink)) }",
        );
        assert_eq!(painted[html].background, Color::rgb(248, 250, 252));
    }
}
