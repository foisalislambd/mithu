//! Computed styles. Cascade, selector matching, and custom properties come from
//! Servo's Stylo (via the Blitz DOM integration). This module projects those
//! computed values into the types layout and `getComputedStyle` already use.
#![allow(dead_code)]

use mithu_base::Color;
use mithu_css::{AttrOp, Combinator, Pseudo, Selector, parse_css};
use mithu_dom::{Document, NodeId};

mod engine;

pub use engine::{Frame, Placed, PlacedFragment, PlacedLine, remembered, resolve};

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
    /// SVG `fill` when a rule sets one. `None` leaves the paint default.
    pub fill: Option<Color>,
    pub background: Color,
    /// First `url(...)` in `background-image`, when one is set.
    pub background_image: Option<String>,
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
    /// `aspect-ratio` as width/height, when the used height is automatic.
    pub aspect_ratio: Option<(f32, f32)>,
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
            fill: None,
            background: Color::TRANSPARENT,
            background_image: None,
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
            aspect_ratio: None,
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
    /// Classes, ids, and tags of ancestors. A selector whose ancestor
    /// compounds need a token that is missing here cannot match.
    pub ancestor_bloom: u64,
    pub filter_ancestors: bool,
}

impl Default for StyleState {
    fn default() -> Self {
        Self {
            hover: None,
            viewport_width: 1280.0,
            ancestor_bloom: 0,
            filter_ancestors: false,
        }
    }
}

fn token_bloom(kind: u8, name: &str) -> u64 {
    let mut hash = 0x811c9dc5u64 ^ kind as u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x01000193);
    }
    (1u64 << (hash & 63)) | (1u64 << ((hash >> 8) & 63))
}

fn ancestor_token_mask(selector: &Selector) -> u64 {
    let Some(last) = selector.compounds.len().checked_sub(1) else {
        return 0;
    };
    let mut mask = 0u64;
    for (index, compound) in selector.compounds.iter().take(last).enumerate() {
        // A compound before `+` or `~` is a sibling, not an ancestor.
        if matches!(
            selector.combinators.get(index),
            Some(Combinator::NextSibling | Combinator::SubsequentSibling)
        ) {
            continue;
        }
        for class in &compound.classes {
            mask |= token_bloom(1, class);
        }
        if let Some(id) = &compound.id {
            mask |= token_bloom(2, id);
        }
        if let Some(tag) = &compound.tag {
            mask |= token_bloom(3, tag);
        }
        for attr in &compound.attrs {
            mask |= token_bloom(4, &attr.name);
        }
    }
    mask
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
    crate::engine::remember(author_css, state);
    crate::engine::resolve(doc, author_css, state, 800.0, &[], &[]).styles
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
    if state.filter_ancestors {
        let mask = ancestor_token_mask(selector);
        if mask != 0 && state.ancestor_bloom & mask != mask {
            return false;
        }
    }
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
    fn logical_size_and_aspect_ratio_are_recorded() {
        let doc = parse_html(r#"<div id="box"></div>"#);
        let styles = style_document(
            &doc,
            "#box { inline-size: 50%; min-block-size: 4rem; aspect-ratio: 16/9 }",
        );
        let style = &styles[doc.get_element_by_id("box").unwrap().0];
        assert_eq!(style.width, Length::Percent(50.0));
        // Stylo resolves `rem` at computed-value time against the root font size.
        assert_eq!(style.min_height, Length::Px(64.0));
        assert_eq!(style.aspect_ratio, Some((16.0, 9.0)));
    }

    #[test]
    fn indexed_rules_still_match_classes_tags_and_is() {
        let doc = parse_html(r#"<div class="a" id="box"><span class="b">t</span></div>"#);
        let styles = style_document(
            &doc,
            ":is(.a, span) { color: red } .b { color: blue } #box { background-color: green } div { display: flex }",
        );
        let div = doc.get_element_by_id("box").unwrap();
        let span = doc
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                matches!(doc.element_name(NodeId(index)), Some("span")).then_some(NodeId(index))
            })
            .unwrap();
        assert_eq!(styles[div.0].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[div.0].display, Display::Flex);
        assert_eq!(styles[div.0].background, Color::rgb(0, 128, 0));
        assert_eq!(styles[span.0].color, Color::rgb(0, 0, 255));
        let marked = parse_html(
            r#"<section class="on"><b data-mark="1">x</b></section><b data-mark="1">y</b>"#,
        );
        let marked_styles = style_document(
            &marked,
            "[data-mark] { color: navy } .on [data-mark] { color: teal }",
        );
        let colors: Vec<_> = marked
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                (marked.element_name(NodeId(index)) == Some("b"))
                    .then_some(marked_styles[index].color)
            })
            .collect();
        assert_eq!(colors, vec![Color::rgb(0, 128, 128), Color::rgb(0, 0, 128)]);
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
        // `calc(var(--spacing) * 14)` with `--spacing: .25rem` computes to 56px.
        assert_eq!(styles[bar.0].height, Length::Px(56.0));
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
