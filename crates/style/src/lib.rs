//! Cascade computed styles from the user-agent sheet, author CSS, and `style` attributes.

use mithu_base::Color;
use mithu_css::{
    parse_css, parse_declarations, AttrOp, Combinator, Declaration, Pseudo, Selector, Stylesheet,
};
use mithu_dom::{Document, NodeId};

pub const USER_AGENT_CSS: &str = r#"
html, address, article, aside, blockquote, div, dl, fieldset, figcaption, figure,
footer, form, h1, h2, h3, h4, h5, h6, header, hr, main, nav, ol, p, pre, section,
table, ul, dd, dt { display: block; }
li { display: list-item; }
head, style, script, title, meta, link, base, template { display: none; }
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
img, iframe, video, canvas, svg, audio { background-color: #ececec; border-width: 1px; border-color: #bdbdbd; }
button, input, textarea, select {
  border-width: 1px; border-color: #888888; background-color: #ffffff;
  padding-top: 4px; padding-right: 8px; padding-bottom: 4px; padding-left: 8px; color: #111111;
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Inline,
    Block,
    InlineBlock,
    ListItem,
    Flex,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
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
    Pre,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub color: Color,
    pub background: Color,
    pub font_size: f32,
    pub bold: bool,
    pub mono: bool,
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
    pub justify: Justify,
    pub align_items: AlignItems,
    pub flex_grow: f32,
    pub box_sizing: BoxSizing,
    pub overflow_hidden: bool,
    pub border_radius: f32,
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
            justify: Justify::Start,
            align_items: AlignItems::Stretch,
            flex_grow: 0.0,
            box_sizing: BoxSizing::ContentBox,
            overflow_hidden: false,
            border_radius: 0.0,
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
    font_size: Option<Ranked<FontSize>>,
    bold: Option<Ranked<bool>>,
    mono: Option<Ranked<bool>>,
    text_align: Option<Ranked<TextAlign>>,
    underline: Option<Ranked<bool>>,
    white_space: Option<Ranked<WhiteSpace>>,
    width: Option<Ranked<Length>>,
    height: Option<Ranked<Length>>,
    margin: [Option<Ranked<Length>>; 4],
    padding: [Option<Ranked<Length>>; 4],
    border_width: Option<Ranked<f32>>,
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
    justify: Option<Ranked<Justify>>,
    align_items: Option<Ranked<AlignItems>>,
    flex_grow: Option<Ranked<f32>>,
    box_sizing: Option<Ranked<BoxSizing>>,
    overflow_hidden: Option<Ranked<bool>>,
    border_radius: Option<Ranked<f32>>,
}

#[derive(Clone, Copy, Debug)]
enum FontSize {
    Px(f32),
    Em(f32),
    Percent(f32),
}

pub fn matches_element(doc: &Document, id: NodeId, selector: &Selector) -> bool {
    matches_selector(doc, id, selector)
}

pub fn style_document(doc: &Document, author_css: &str) -> Vec<ComputedStyle> {
    let ua = parse_css(USER_AGENT_CSS);
    let author = parse_css(author_css);
    let mut styles = vec![ComputedStyle::initial(); doc.nodes.len()];
    let initial = ComputedStyle::initial();
    for child in doc.node(doc.root).children.clone() {
        compute(doc, child, &initial, &ua, &author, &mut styles);
    }
    styles
}

fn compute(
    doc: &Document,
    id: NodeId,
    parent: &ComputedStyle,
    ua: &Stylesheet,
    author: &Stylesheet,
    styles: &mut [ComputedStyle],
) {
    if doc.element_name(id).is_none() {
        return;
    }
    let computed = concretize(doc, id, &cascade(doc, id, ua, author), parent);
    styles[id.0] = computed.clone();
    for child in doc.node(id).children.clone() {
        compute(doc, child, &computed, ua, author, styles);
    }
}

fn cascade(doc: &Document, id: NodeId, ua: &Stylesheet, author: &Stylesheet) -> Specified {
    let mut specified = Specified::default();
    apply_sheet(doc, id, ua, &mut specified);
    apply_sheet(doc, id, author, &mut specified);
    if let Some(inline) = doc.attr(id, "style") {
        for declaration in parse_declarations(inline) {
            let spec = 1_000 + if declaration.important { 100_000 } else { 0 };
            apply_declaration(&declaration, spec, &mut specified);
        }
    }
    specified
}

fn apply_sheet(doc: &Document, id: NodeId, sheet: &Stylesheet, specified: &mut Specified) {
    for rule in &sheet.rules {
        for selector in &rule.selectors {
            if !matches_selector(doc, id, selector) {
                continue;
            }
            for declaration in &rule.declarations {
                let spec = selector.specificity() + if declaration.important { 100_000 } else { 0 };
                apply_declaration(declaration, spec, specified);
            }
        }
    }
}

fn matches_selector(doc: &Document, id: NodeId, selector: &mithu_css::Selector) -> bool {
    let Some(last) = selector.compounds.last() else {
        return false;
    };
    if !matches_compound(doc, id, last) {
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
                if !matches_compound(doc, parent, compound) {
                    return false;
                }
                current = parent;
            }
            Some(Combinator::NextSibling) => {
                let Some(previous) = doc.previous_element_sibling(current) else {
                    return false;
                };
                if !matches_compound(doc, previous, compound) {
                    return false;
                }
                current = previous;
            }
            Some(Combinator::SubsequentSibling) => {
                let mut previous = doc.previous_element_sibling(current);
                let mut found = false;
                while let Some(candidate) = previous {
                    if matches_compound(doc, candidate, compound) {
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
                    if matches_compound(doc, candidate, compound) {
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

fn matches_compound(doc: &Document, id: NodeId, compound: &mithu_css::Compound) -> bool {
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
    compound.pseudos.iter().all(|pseudo| matches_pseudo(doc, id, pseudo))
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
        AttrOp::Dash => value == expected || value.strip_prefix(expected).is_some_and(|rest| rest.starts_with('-')),
        AttrOp::Prefix => value.starts_with(expected),
        AttrOp::Suffix => value.ends_with(expected),
        AttrOp::Substring => value.contains(expected),
    }
}

fn matches_pseudo(doc: &Document, id: NodeId, pseudo: &Pseudo) -> bool {
    match pseudo {
        Pseudo::FirstChild => doc.previous_element_sibling(id).is_none(),
        Pseudo::LastChild => doc.next_element_sibling(id).is_none(),
        Pseudo::Nth(a, b) => doc
            .element_child_index(id)
            .is_some_and(|index| nth_matches(*a, *b, index)),
        Pseudo::Link => doc.element_name(id) == Some("a") && doc.attr(id, "href").is_some(),
        Pseudo::Empty => is_empty(doc, id),
        Pseudo::Not(inner) => !matches_compound(doc, id, inner),
        Pseudo::Unmatched => false,
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
    doc.node(id).children.iter().all(|child| match &doc.node(*child).data {
        mithu_dom::NodeData::Comment(_) => true,
        mithu_dom::NodeData::Text(text) => text.trim().is_empty(),
        _ => false,
    })
}

fn apply_declaration(declaration: &Declaration, spec: u32, specified: &mut Specified) {
    let value = declaration.value.as_str();
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
        "background-color" | "background" => {
            if let Some(color) = parse_color(value) {
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
        "font-family" => rank(&mut specified.mono, value.to_ascii_lowercase().contains("mono"), spec),
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
        "border" => {
            if let Some((width, color)) = parse_border(value) {
                rank(&mut specified.border_width, width, spec);
                if let Some(color) = color {
                    rank(&mut specified.border_color, color, spec);
                }
            }
        }
        "border-width" => {
            if let Some(width) = parse_border_width(value) {
                rank(&mut specified.border_width, width, spec);
            }
        }
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
        "gap" | "row-gap" | "column-gap" => {
            if let Some(Length::Px(px)) = parse_length(value) {
                rank(&mut specified.gap, px.max(0.0), spec);
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
        "box-sizing" => {
            if let Some(sizing) = parse_box_sizing(value) {
                rank(&mut specified.box_sizing, sizing, spec);
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            let hidden = matches!(value.trim().to_ascii_lowercase().as_str(), "hidden" | "clip" | "scroll");
            rank(&mut specified.overflow_hidden, hidden, spec);
        }
        "border-radius" => {
            if let Some(Length::Px(px)) = parse_length(value.split_whitespace().next().unwrap_or(value)) {
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

fn concretize(doc: &Document, id: NodeId, specified: &Specified, parent: &ComputedStyle) -> ComputedStyle {
    let font_size = match specified.font_size.as_ref().map(|item| item.value) {
        Some(FontSize::Px(value)) => value.max(1.0),
        Some(FontSize::Em(value)) => (parent.font_size * value).max(1.0),
        Some(FontSize::Percent(value)) => (parent.font_size * value / 100.0).max(1.0),
        None => parent.font_size,
    };
    let color = specified.color.as_ref().map(|item| item.value).unwrap_or(parent.color);
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
        font_size,
        bold: specified.bold.as_ref().map(|item| item.value).unwrap_or(parent.bold),
        mono: specified.mono.as_ref().map(|item| item.value).unwrap_or(parent.mono),
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
        width: specified.width.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        height: specified.height.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        margin: take_sides(&specified.margin, Length::Px(0.0)),
        padding: take_sides(&specified.padding, Length::Px(0.0)),
        border_width: specified.border_width.as_ref().map(|item| item.value).unwrap_or(0.0),
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
        min_width: specified.min_width.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        max_width: specified.max_width.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        min_height: specified.min_height.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        max_height: specified.max_height.as_ref().map(|item| item.value).unwrap_or(Length::Auto),
        opacity: specified.opacity.as_ref().map(|item| item.value).unwrap_or(1.0).clamp(0.0, 1.0),
        flex_direction: specified
            .flex_direction
            .as_ref()
            .map(|item| item.value.0)
            .unwrap_or(FlexDirection::Row),
        flex_reverse: specified.flex_direction.as_ref().is_some_and(|item| item.value.1),
        gap: specified.gap.as_ref().map(|item| item.value).unwrap_or(0.0),
        justify: specified.justify.as_ref().map(|item| item.value).unwrap_or(Justify::Start),
        align_items: specified
            .align_items
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(AlignItems::Stretch),
        flex_grow: specified.flex_grow.as_ref().map(|item| item.value).unwrap_or(0.0),
        box_sizing: specified
            .box_sizing
            .as_ref()
            .map(|item| item.value)
            .unwrap_or(BoxSizing::ContentBox),
        overflow_hidden: specified.overflow_hidden.as_ref().is_some_and(|item| item.value),
        border_radius: specified.border_radius.as_ref().map(|item| item.value).unwrap_or(0.0),
    };
    apply_presentational_hints(doc, id, &mut computed);
    computed
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
    if name == "input" && doc.attr(id, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("hidden")) {
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
        "img" | "iframe" | "video" | "canvas" | "svg" => {
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
        "flex" | "inline-flex" => Display::Flex,
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
        Length::Percent(percent) => Some(FontSize::Percent(percent)),
        Length::Auto => None,
    }
}

fn parse_position(value: &str) -> Option<Position> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "static" => Position::Static,
        "relative" => Position::Relative,
        "absolute" | "fixed" | "sticky" => Position::Absolute,
        _ => return None,
    })
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
        return percent.trim().parse::<f32>().ok().map(|number| number / 100.0);
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
        "normal" | "nowrap" => WhiteSpace::Normal,
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
            Length::Auto => return None,
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
    if let Some(number) = value.strip_suffix("em") {
        return number.trim().parse().ok().map(Length::Em);
    }
    if let Some(number) = value.strip_suffix("rem") {
        return number.trim().parse::<f32>().ok().map(|em| Length::Px(em * 16.0));
    }
    None
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
            _ => None,
        },
    }
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(color) = parse_color_token(value) {
        return Some(color);
    }
    if let Some(color) = parse_rgb(value) {
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
        3 | 4 => Some(Color::rgba(nibble(0)?, nibble(1)?, nibble(2)?, if chars.len() == 4 { nibble(3)? } else { 255 })),
        6 | 8 => Some(Color::rgba(pair(0)?, pair(2)?, pair(4)?, if chars.len() == 8 { pair(6)? } else { 255 })),
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
    Some(Color::rgb(channel(numbers[0])?, channel(numbers[1])?, channel(numbers[2])?))
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
            doc.nodes.iter().enumerate().find_map(|(index, node)| {
                matches!(doc.element_name(NodeId(index)), Some("p")).then_some(NodeId(index))
            })
        });
        let p = p.expect("p");
        assert_eq!(styles[p.0].color, Color::rgb(1, 2, 3));
        assert_eq!(styles[p.0].display, Display::Block);
    }

    #[test]
    fn flex_position_and_attribute_selector() {
        let doc = parse_html(r#"<div id="row" class="row"><b id="one">A</b><i id="two">B</i></div>"#);
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
}
