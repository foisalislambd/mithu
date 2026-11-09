//! Block and inline layout.
//!
//! Block boxes fill their containing block. Inline content wraps into line
//! boxes. Inline-block and replaced elements (images, inputs, frames) become
//! atomic boxes. Margin collapsing and floats are not implemented.

use mithu_base::{Edges, Rect};
use mithu_dom::{Document, NodeData, NodeId};
use mithu_style::{
    AlignItems, BoxSizing, ComputedStyle, Display, FlexDirection, Justify, Length, LineHeight, Position,
    TextAlign, WhiteSpace,
};
use mithu_text::MeasureText;

#[derive(Clone, Debug)]
pub struct LayoutTree {
    pub root: LayoutBox,
    pub links: Vec<Link>,
    pub content_height: f32,
    pub background: mithu_base::Color,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub href: String,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct LayoutBox {
    pub node: NodeId,
    pub rect: Rect,
    pub margin: Edges,
    pub background: mithu_base::Color,
    pub border_color: mithu_base::Color,
    pub border_width: f32,
    pub color: mithu_base::Color,
    pub font_size: f32,
    pub bold: bool,
    pub mono: bool,
    pub href: Option<String>,
    pub marker: Option<String>,
    pub children: Vec<LayoutBox>,
    pub lines: Vec<LineBox>,
    pub opacity: f32,
    pub overflow_hidden: bool,
    pub border_radius: f32,
}

#[derive(Clone, Debug)]
pub struct LineBox {
    pub baseline: f32,
    pub ascent: f32,
    pub height: f32,
    pub fragments: Vec<TextFragment>,
}

#[derive(Clone, Debug)]
pub struct TextFragment {
    pub x: f32,
    pub text: String,
    pub color: mithu_base::Color,
    pub size: f32,
    pub bold: bool,
    pub mono: bool,
    pub underline: bool,
    pub href: Option<String>,
    pub width: f32,
}

pub fn layout_tree(
    doc: &Document,
    styles: &[ComputedStyle],
    viewport_width: f32,
    measure: &dyn MeasureText,
) -> LayoutTree {
    let root_id = document_element(doc).unwrap_or(doc.root);
    let root = layout_node(doc, styles, measure, root_id, 0.0, 0.0, viewport_width.max(1.0), None, false)
        .unwrap_or_else(|| empty_box(root_id));
    let content_height = root.rect.bottom() + root.margin.bottom;
    let mut links = Vec::new();
    collect_links(&root, &mut links);
    LayoutTree {
        background: page_background(doc, styles),
        root,
        links,
        content_height,
    }
}

fn layout_node(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    containing_x: f32,
    margin_y: f32,
    containing_width: f32,
    inherited_href: Option<String>,
    shrink_to_fit: bool,
) -> Option<LayoutBox> {
    let style = style_of(styles, id);
    let display = effective_display(doc, styles, id);
    if display == Display::None {
        return None;
    }
    let href = own_href(doc, id).or(inherited_href);
    let font = style.font_size;
    let border = style.border_width.max(0.0);
    let padding = resolve_edges(&style.padding, font, containing_width, false);
    let width_is_auto = matches!(style.width, Length::Auto);
    let width_opt = resolve_length(style.width, font, containing_width).map(|width| {
        if style.box_sizing == BoxSizing::BorderBox {
            (width - padding.left - padding.right - border * 2.0).max(0.0)
        } else {
            width.max(0.0)
        }
    });
    let margin_top = resolve_length(style.margin[0], font, containing_width).unwrap_or(0.0).max(0.0);
    let margin_bottom = resolve_length(style.margin[2], font, containing_width).unwrap_or(0.0).max(0.0);
    let margin_left_opt = resolve_length(style.margin[3], font, containing_width);
    let margin_right_opt = resolve_length(style.margin[1], font, containing_width);

    let (margin_left, margin_right, mut content_w) = if let Some(width) = width_opt {
        let space = containing_width - width - padding.left - padding.right - border * 2.0;
        let (left, right) = match (margin_left_opt, margin_right_opt) {
            (None, None) if space >= 0.0 => (space / 2.0, space / 2.0),
            (None, Some(right)) => ((space - right).max(0.0), right.max(0.0)),
            (Some(left), None) => (left.max(0.0), (space - left).max(0.0)),
            (Some(left), Some(right)) => (left.max(0.0), right.max(0.0)),
            _ => (0.0, 0.0),
        };
        (left, right, width.max(0.0))
    } else {
        let left = margin_left_opt.unwrap_or(0.0).max(0.0);
        let right = margin_right_opt.unwrap_or(0.0).max(0.0);
        let content = (containing_width - left - right - padding.left - padding.right - border * 2.0).max(0.0);
        (left, right, content)
    };

    let shrink = width_is_auto && (display == Display::InlineBlock || shrink_to_fit);
    let border_x = containing_x + margin_left;
    let border_y = margin_y + margin_top;
    let content_x = border_x + border + padding.left;
    let content_y = border_y + border + padding.top;

    content_w = limit_content(&style, content_w, font, containing_width, padding.left + padding.right, border);
    let synthetic = synthetic_text(doc, id);
    let blockish = synthetic.is_none() && has_block_child(doc, styles, id);
    let (lines, mut children, flowed) = if display == Display::Flex {
        layout_flex(
            doc,
            styles,
            measure,
            id,
            &style,
            content_x,
            content_y,
            content_w,
            href.clone(),
        )
    } else if blockish {
        layout_blocks(
            doc,
            styles,
            measure,
            id,
            &style,
            content_x,
            content_y,
            content_w,
            href.clone(),
        )
    } else {
        let items = if let Some(text) = &synthetic {
            items_from_text(text, &style, measure, href.clone())
        } else {
            collect_children(doc, styles, measure, id, &style, content_w, href.clone())
        };
        if shrink && !items.is_empty() {
            content_w = preferred_width(&items, content_w);
            content_w = limit_content(&style, content_w, font, containing_width, padding.left + padding.right, border);
        }
        if is_blank(&items) {
            (Vec::new(), Vec::new(), 0.0)
        } else {
            place_items(&items, content_x, content_y, content_w, style.text_align)
        }
    };

    let mut specified_height = resolve_height(style.height, font);
    if style.box_sizing == BoxSizing::BorderBox
        && let Some(height) = specified_height
    {
        specified_height = Some((height - padding.top - padding.bottom - border * 2.0).max(0.0));
    }
    let mut content_h = flowed.max(specified_height.unwrap_or(0.0));
    if let Some(max) = resolve_height(style.max_height, font) {
        content_h = content_h.min(max);
    }
    if let Some(min) = resolve_height(style.min_height, font) {
        content_h = content_h.max(min);
    }
    children.extend(layout_absolute_children(
        doc,
        styles,
        measure,
        id,
        content_x,
        content_y,
        content_w,
        content_h,
        href.clone(),
    ));
    let rect = Rect::new(
        border_x,
        border_y,
        content_w + padding.left + padding.right + border * 2.0,
        content_h + padding.top + padding.bottom + border * 2.0,
    );
    let marker = (display == Display::ListItem).then(|| list_marker(doc, id));
    let mut laid_out = LayoutBox {
        node: id,
        rect,
        margin: Edges::new(margin_top, margin_right, margin_bottom, margin_left),
        background: style.background,
        border_color: style.border_color,
        border_width: border,
        color: style.color,
        font_size: style.font_size,
        bold: style.bold,
        mono: style.mono,
        href,
        marker,
        children,
        lines,
        opacity: style.opacity.clamp(0.0, 1.0),
        overflow_hidden: style.overflow_hidden,
        border_radius: style.border_radius,
    };
    if style.position == Position::Relative {
        let dx = resolve_length(style.insets[3], font, containing_width)
            .or_else(|| resolve_length(style.insets[1], font, containing_width).map(|right| -right))
            .unwrap_or(0.0);
        let dy = resolve_length(style.insets[0], font, containing_width)
            .or_else(|| resolve_length(style.insets[2], font, containing_width).map(|bottom| -bottom))
            .unwrap_or(0.0);
        translate(&mut laid_out, dx, dy);
    }
    Some(laid_out)
}

fn limit_content(style: &ComputedStyle, content_w: f32, font: f32, containing: f32, padding_x: f32, border: f32) -> f32 {
    let adjust = |length: f32| {
        if style.box_sizing == BoxSizing::BorderBox {
            (length - padding_x - border * 2.0).max(0.0)
        } else {
            length.max(0.0)
        }
    };
    let mut width = content_w;
    if let Some(max) = resolve_length(style.max_width, font, containing) {
        width = width.min(adjust(max));
    }
    if let Some(min) = resolve_length(style.min_width, font, containing) {
        width = width.max(adjust(min));
    }
    width.max(0.0)
}

fn layout_blocks(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    href: Option<String>,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let mut y = content_y;
    let mut group = Vec::new();
    let mut lines = Vec::new();
    let mut children = Vec::new();
    let flush = |group: &mut Vec<NodeId>, y: &mut f32, lines: &mut Vec<LineBox>, children: &mut Vec<LayoutBox>| {
        if group.is_empty() {
            return;
        }
        let mut items = Vec::new();
        for child in group.drain(..) {
            items.extend(collect_node(doc, styles, measure, child, style, content_w, href.clone()));
        }
        if is_blank(&items) {
            return;
        }
        let (new_lines, atoms, height) = place_items(&items, content_x, *y, content_w, style.text_align);
        lines.extend(new_lines);
        children.extend(atoms);
        *y += height;
    };
    for child in doc.node(id).children.clone() {
        if is_block_child(doc, styles, child) {
            flush(&mut group, &mut y, &mut lines, &mut children);
            if let Some(child_box) = layout_node(doc, styles, measure, child, content_x, y, content_w, href.clone(), false) {
                y = child_box.rect.bottom() + child_box.margin.bottom;
                children.push(child_box);
            }
        } else if !is_skipped(doc, styles, child) {
            group.push(child);
        }
    }
    flush(&mut group, &mut y, &mut lines, &mut children);
    (lines, children, (y - content_y).max(0.0))
}

fn collect_children(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    style: &ComputedStyle,
    available: f32,
    href: Option<String>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for child in doc.node(id).children.clone() {
        items.extend(collect_node(doc, styles, measure, child, style, available, href.clone()));
    }
    items
}

fn collect_node(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    parent_style: &ComputedStyle,
    available: f32,
    href: Option<String>,
) -> Vec<Item> {
    match &doc.node(id).data {
        NodeData::Text(text) => items_from_text(text, parent_style, measure, href),
        NodeData::Comment(_) | NodeData::Document => Vec::new(),
        NodeData::Element(_) => {
            if is_skipped(doc, styles, id) {
                return Vec::new();
            }
            if doc.element_name(id) == Some("br") {
                return vec![Item::Break];
            }
            let style = style_of(styles, id);
            let href = own_href(doc, id).or(href);
            let display = effective_display(doc, styles, id);
            if matches!(display, Display::Block | Display::ListItem | Display::InlineBlock | Display::Flex)
                || synthetic_text(doc, id).is_some()
            {
                let Some(layout) = layout_node(doc, styles, measure, id, 0.0, 0.0, available, href, false) else {
                    return Vec::new();
                };
                let width = layout.margin.left + layout.rect.width + layout.margin.right;
                let height = layout.margin.top + layout.rect.height + layout.margin.bottom;
                return vec![Item::Atomic { layout, width, height }];
            }
            collect_children(doc, styles, measure, id, &style, available, href)
        }
    }
}

fn place_items(items: &[Item], content_x: f32, y: f32, content_w: f32, align: TextAlign) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let mut built: Vec<(Vec<usize>, f32)> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut width = 0.0_f32;
    let flush = |current: &mut Vec<usize>, width: &mut f32, built: &mut Vec<(Vec<usize>, f32)>| {
        while current
            .last()
            .is_some_and(|index| matches!(items[*index], Item::Space { sticky: false, .. }))
        {
            if let Some(index) = current.pop()
                && let Item::Space { width: space, .. } = items[index]
            {
                *width -= space;
            }
        }
        if !current.is_empty() {
            built.push((std::mem::take(current), (*width).max(0.0)));
        }
        *width = 0.0;
    };
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Break => flush(&mut current, &mut width, &mut built),
            Item::Space { sticky: false, .. } if current.is_empty() => {}
            Item::Space { width: space, .. } => {
                current.push(index);
                width += space;
            }
            Item::Word(word) => {
                if !current.is_empty() && width + word.width > content_w {
                    flush(&mut current, &mut width, &mut built);
                }
                current.push(index);
                width += word.width;
            }
            Item::Atomic { width: atom, .. } => {
                if !current.is_empty() && width + atom > content_w {
                    flush(&mut current, &mut width, &mut built);
                }
                current.push(index);
                width += atom;
            }
        }
    }
    flush(&mut current, &mut width, &mut built);

    let mut lines = Vec::new();
    let mut atoms = Vec::new();
    let mut cursor_y = y;
    for (pieces, ink) in built {
        let extra = (content_w - ink).max(0.0);
        let offset = match align {
            TextAlign::Left => 0.0,
            TextAlign::Center => extra / 2.0,
            TextAlign::Right => extra,
        };
        let mut x = content_x + offset;
        let ascent = pieces
            .iter()
            .map(|index| match &items[*index] {
                Item::Word(word) => word.ascent,
                Item::Atomic { height, .. } => *height,
                _ => 0.0,
            })
            .fold(0.0_f32, f32::max);
        let height = pieces
            .iter()
            .map(|index| match &items[*index] {
                Item::Word(word) => word.line_height,
                Item::Atomic { height, .. } => *height,
                _ => 0.0,
            })
            .fold(ascent, f32::max)
            .max(1.0);
        let baseline = cursor_y + ascent;
        let mut fragments = Vec::new();
        for index in pieces {
            match &items[index] {
                Item::Word(word) => {
                    fragments.push(TextFragment {
                        x,
                        text: word.text.clone(),
                        color: word.color,
                        size: word.size,
                        bold: word.bold,
                        mono: word.mono,
                        underline: word.underline,
                        href: word.href.clone(),
                        width: word.width,
                    });
                    x += word.width;
                }
                Item::Atomic { layout, width: atom, .. } => {
                    let mut layout = layout.clone();
                    translate(&mut layout, x, cursor_y);
                    atoms.push(layout);
                    x += atom;
                }
                Item::Space { width: space, .. } => x += space,
                Item::Break => {}
            }
        }
        lines.push(LineBox {
            baseline,
            ascent,
            height,
            fragments,
        });
        cursor_y += height;
    }
    (lines, atoms, (cursor_y - y).max(0.0))
}

fn preferred_width(items: &[Item], available: f32) -> f32 {
    let mut max_line: f32 = 0.0;
    let mut line: f32 = 0.0;
    let mut empty = true;
    for item in items {
        match item {
            Item::Break => {
                max_line = max_line.max(line);
                line = 0.0;
                empty = true;
            }
            Item::Space { sticky: false, .. } if empty => {}
            Item::Space { width, .. } => {
                line += width;
                empty = false;
            }
            Item::Word(word) => {
                if !empty && line + word.width > available {
                    max_line = max_line.max(line);
                    line = 0.0;
                }
                line += word.width;
                empty = false;
            }
            Item::Atomic { width, .. } => {
                if !empty && line + width > available {
                    max_line = max_line.max(line);
                    line = 0.0;
                }
                line += width;
                empty = false;
            }
        }
    }
    max_line.max(line).min(available).max(0.0)
}

fn items_from_text(text: &str, style: &ComputedStyle, measure: &dyn MeasureText, href: Option<String>) -> Vec<Item> {
    let pre = style.white_space == WhiteSpace::Pre;
    let normalized = if pre {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        collapse_space(text)
    };
    if normalized.is_empty() {
        return Vec::new();
    }
    let (ascent, line_height) = line_metrics(style, measure);
    let mut items = Vec::new();
    if pre {
        for (index, line) in normalized.split('\n').enumerate() {
            if index > 0 {
                items.push(Item::Break);
            }
            push_runs(&mut items, line, style, measure, href.clone(), ascent, line_height, true);
        }
    } else {
        push_runs(&mut items, &normalized, style, measure, href, ascent, line_height, false);
    }
    items
}

fn push_runs(
    items: &mut Vec<Item>,
    text: &str,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    ascent: f32,
    line_height: f32,
    sticky: bool,
) {
    let mut current = String::new();
    let mut spaces = false;
    let flush = |items: &mut Vec<Item>, current: &mut String, spaces: bool| {
        if current.is_empty() {
            return;
        }
        let text = std::mem::take(current);
        if spaces {
            let width = measure.width(&text, style.font_size, style.bold, style.mono);
            items.push(Item::Space { width, sticky });
        } else {
            let width = measure.width(&text, style.font_size, style.bold, style.mono);
            items.push(Item::Word(Word {
                text,
                color: style.color,
                size: style.font_size,
                bold: style.bold,
                mono: style.mono,
                underline: style.underline,
                href: href.clone(),
                width,
                ascent,
                line_height,
            }));
        }
    };
    for ch in text.chars() {
        let is_space = ch == ' ';
        if current.is_empty() {
            spaces = is_space;
            current.push(ch);
        } else if is_space == spaces {
            current.push(ch);
        } else {
            flush(items, &mut current, spaces);
            spaces = is_space;
            current.push(ch);
        }
    }
    flush(items, &mut current, spaces);
}

fn line_metrics(style: &ComputedStyle, measure: &dyn MeasureText) -> (f32, f32) {
    let ascent = measure.ascent(style.font_size, style.mono).max(1.0);
    let natural = measure.line_height(style.font_size, style.mono).max(ascent);
    let height = match style.line_height {
        LineHeight::Normal => natural,
        LineHeight::Px(px) => px.max(ascent),
        LineHeight::Number(factor) => (style.font_size * factor).max(ascent),
        LineHeight::Percent(percent) => (style.font_size * percent / 100.0).max(ascent),
    };
    (ascent, height)
}

struct Word {
    text: String,
    color: mithu_base::Color,
    size: f32,
    bold: bool,
    mono: bool,
    underline: bool,
    href: Option<String>,
    width: f32,
    ascent: f32,
    line_height: f32,
}

enum Item {
    Word(Word),
    Space { width: f32, sticky: bool },
    Break,
    Atomic { layout: LayoutBox, width: f32, height: f32 },
}

fn is_blank(items: &[Item]) -> bool {
    items.iter().all(|item| matches!(item, Item::Space { .. } | Item::Break))
}

fn translate(box_: &mut LayoutBox, dx: f32, dy: f32) {
    box_.rect.x += dx;
    box_.rect.y += dy;
    for line in &mut box_.lines {
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.x += dx;
        }
    }
    for child in &mut box_.children {
        translate(child, dx, dy);
    }
}

fn collect_links(box_: &LayoutBox, out: &mut Vec<Link>) {
    if let Some(href) = &box_.href
        && box_.rect.width > 0.0
        && box_.rect.height > 0.0
    {
        out.push(Link {
            href: href.clone(),
            rect: box_.rect,
        });
    }
    for line in &box_.lines {
        for fragment in &line.fragments {
            if let Some(href) = &fragment.href {
                out.push(Link {
                    href: href.clone(),
                    rect: Rect::new(
                        fragment.x,
                        line.baseline - line.ascent,
                        fragment.width.max(1.0),
                        line.height.max(1.0),
                    ),
                });
            }
        }
    }
    for child in &box_.children {
        collect_links(child, out);
    }
}

fn document_element(doc: &Document) -> Option<NodeId> {
    doc.node(doc.root)
        .children
        .iter()
        .copied()
        .find(|id| doc.element_name(*id).is_some())
}

fn style_of(styles: &[ComputedStyle], id: NodeId) -> ComputedStyle {
    styles.get(id.0).cloned().unwrap_or_else(ComputedStyle::initial)
}

fn effective_display(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> Display {
    let display = style_of(styles, id).display;
    if display == Display::None {
        return Display::None;
    }
    if display == Display::Inline && contains_block(doc, styles, id) {
        return Display::Block;
    }
    display
}

fn contains_block(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    for child in &doc.node(id).children {
        if doc.element_name(*child).is_none() {
            continue;
        }
        let display = style_of(styles, *child).display;
        if display == Display::None {
            continue;
        }
        if matches!(display, Display::Block | Display::ListItem | Display::Flex) || contains_block(doc, styles, *child) {
            return true;
        }
    }
    false
}

fn has_block_child(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    doc.node(id).children.iter().any(|child| is_block_child(doc, styles, *child))
}

fn is_block_child(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    if is_absolute(doc, styles, id) {
        return false;
    }
    doc.element_name(id).is_some_and(|_| {
        matches!(effective_display(doc, styles, id), Display::Block | Display::ListItem | Display::Flex)
    })
}

fn is_absolute(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    doc.element_name(id).is_some() && style_of(styles, id).position == Position::Absolute
}

fn is_skipped(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    if is_absolute(doc, styles, id) {
        return true;
    }
    match &doc.node(id).data {
        NodeData::Comment(_) => true,
        NodeData::Element(_) => effective_display(doc, styles, id) == Display::None,
        _ => false,
    }
}

fn own_href(doc: &Document, id: NodeId) -> Option<String> {
    if doc.element_name(id) == Some("a") {
        doc.attr(id, "href").filter(|href| !href.is_empty()).map(str::to_string)
    } else {
        None
    }
}

fn synthetic_text(doc: &Document, id: NodeId) -> Option<String> {
    let name = doc.element_name(id)?;
    match name {
        "input" => {
            let kind = doc.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
            if matches!(kind.as_str(), "checkbox" | "radio") {
                let checked = doc.attr(id, "checked").is_some();
                return Some(if checked { "✓".to_string() } else { String::new() });
            }
            if matches!(kind.as_str(), "submit" | "button" | "reset") {
                let fallback = if kind == "submit" { "Submit" } else { "Button" };
                return Some(doc.attr(id, "value").unwrap_or(fallback).to_string());
            }
            Some(doc.attr(id, "value").or_else(|| doc.attr(id, "placeholder")).unwrap_or("").to_string())
        }
        "img" | "iframe" | "video" | "canvas" | "svg" | "audio" => {
            Some(doc.attr(id, "alt").unwrap_or(name).to_string())
        }
        _ => None,
    }
}

fn list_marker(doc: &Document, id: NodeId) -> String {
    let Some(parent) = doc.parent_element(id) else {
        return "•".to_string();
    };
    if doc.element_name(parent) != Some("ol") {
        return "•".to_string();
    }
    let mut number = 1;
    for sibling in &doc.node(parent).children {
        if *sibling == id {
            return format!("{number}.");
        }
        if doc.element_name(*sibling) == Some("li") {
            number += 1;
        }
    }
    "•".to_string()
}

fn resolve_edges(sides: &[Length; 4], font: f32, containing: f32, allow_auto: bool) -> Edges {
    let side = |length: Length| resolve_length(length, font, containing).unwrap_or(0.0).max(0.0);
    let _ = allow_auto;
    Edges::new(side(sides[0]), side(sides[1]), side(sides[2]), side(sides[3]))
}

fn resolve_length(length: Length, font: f32, containing: f32) -> Option<f32> {
    match length {
        Length::Auto => None,
        Length::Px(px) => Some(px),
        Length::Percent(percent) => Some(containing * percent / 100.0),
        Length::Em(em) => Some(font * em),
    }
}

fn resolve_height(length: Length, font: f32) -> Option<f32> {
    match length {
        Length::Auto | Length::Percent(_) => None,
        Length::Px(px) => Some(px.max(0.0)),
        Length::Em(em) => Some((font * em).max(0.0)),
    }
}

fn page_background(doc: &Document, styles: &[ComputedStyle]) -> mithu_base::Color {
    let Some(html) = document_element(doc) else {
        return mithu_base::Color::WHITE;
    };
    let mut color = mithu_base::Color::WHITE;
    let html_bg = style_of(styles, html).background;
    if html_bg.a > 0 {
        color = html_bg;
    }
    if let Some(body) = doc.node(html).children.iter().copied().find(|id| doc.element_name(*id) == Some("body")) {
        let body_bg = style_of(styles, body).background;
        if body_bg.a > 0 {
            color = body_bg;
        }
    }
    color
}

fn collapse_space(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in text.chars() {
        if is_ascii_space(ch) {
            pending = true;
        } else {
            if pending {
                out.push(' ');
                pending = false;
            }
            out.push(ch);
        }
    }
    if pending {
        out.push(' ');
    }
    out
}

fn is_ascii_space(ch: char) -> bool {
    matches!(ch, ' ' | '\n' | '\r' | '\t' | '\u{000C}')
}

fn layout_flex(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    href: Option<String>,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let row = style.flex_direction == FlexDirection::Row;
    let mut items = Vec::new();
    for child in doc.node(id).children.clone() {
        if let NodeData::Text(text) = &doc.node(child).data {
            if text.trim().is_empty() {
                continue;
            }
            items.push((text_flex_item(child, text, style, measure, href.clone(), content_w), 0.0));
            continue;
        }
        if is_skipped(doc, styles, child) {
            continue;
        }
        let child_style = style_of(styles, child);
        let shrink = if row {
            matches!(child_style.width, Length::Auto)
        } else {
            matches!(child_style.width, Length::Auto) && style.align_items != AlignItems::Stretch
        };
        if let Some(layout) = layout_node(doc, styles, measure, child, 0.0, 0.0, content_w, href.clone(), shrink) {
            items.push((layout, child_style.flex_grow.max(0.0)));
        }
    }
    if style.flex_reverse {
        items.reverse();
    }
    if items.is_empty() {
        return (Vec::new(), Vec::new(), 0.0);
    }
    let gap = style.gap.max(0.0);
    let count = items.len();
    let outer = |layout: &LayoutBox, main: bool| -> f32 {
        if main == row {
            layout.margin.left + layout.rect.width + layout.margin.right
        } else {
            layout.margin.top + layout.rect.height + layout.margin.bottom
        }
    };
    let bases: Vec<f32> = items.iter().map(|(layout, _)| outer(layout, true)).collect();
    let gap_total = gap * (count.saturating_sub(1)) as f32;
    let used: f32 = bases.iter().sum::<f32>() + gap_total;
    let free = if row { content_w - used } else { 0.0 };
    let grow_sum: f32 = items.iter().map(|(_, grow)| *grow).sum();
    let mut extra = vec![0.0; count];
    if row && free > 0.0 && grow_sum > 0.0 {
        for (index, (_, grow)) in items.iter().enumerate() {
            extra[index] = free * grow / grow_sum;
        }
    }
    let leftover = if row && free > 0.0 && grow_sum == 0.0 { free } else { 0.0 };
    let between = if style.justify == Justify::SpaceBetween && count > 1 {
        leftover / (count - 1) as f32
    } else {
        0.0
    };
    let start = match style.justify {
        Justify::End => leftover,
        Justify::Center => leftover / 2.0,
        Justify::Start | Justify::SpaceBetween => 0.0,
    };
    let cross: Vec<f32> = items.iter().map(|(layout, _)| outer(layout, false)).collect();
    let line = cross.iter().copied().fold(0.0_f32, f32::max);
    let mut cursor = if row { content_x + start } else { content_y + start };
    let mut placed = Vec::new();
    for (index, (mut layout, _)) in items.into_iter().enumerate() {
        if row && extra[index] > 0.0 {
            layout.rect.width += extra[index];
        }
        let main = bases[index] + extra[index];
        let cross_size = cross[index];
        let shift = match style.align_items {
            AlignItems::End => (line - cross_size).max(0.0),
            AlignItems::Center => (line - cross_size).max(0.0) / 2.0,
            AlignItems::Stretch | AlignItems::Start => 0.0,
        };
        if style.align_items == AlignItems::Stretch && row && matches!(style_of(styles, layout.node).height, Length::Auto) {
            layout.rect.height = (line - layout.margin.top - layout.margin.bottom).max(layout.rect.height);
        }
        if style.align_items == AlignItems::Stretch && !row && matches!(style_of(styles, layout.node).width, Length::Auto) {
            layout.rect.width = (content_w - layout.margin.left - layout.margin.right).max(layout.rect.width);
        }
        let (x, y) = if row {
            (cursor, content_y + shift)
        } else {
            (content_x + shift, cursor)
        };
        translate(&mut layout, x, y);
        cursor += main + gap + between;
        placed.push(layout);
    }
    let height = if row {
        line
    } else {
        (cursor - content_y - gap).max(0.0)
    };
    (Vec::new(), placed, height)
}

fn text_flex_item(
    node: NodeId,
    text: &str,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    content_w: f32,
) -> LayoutBox {
    let items = items_from_text(text, style, measure, href.clone());
    let width = preferred_width(&items, content_w).min(content_w).max(0.0);
    let (lines, children, height) = if is_blank(&items) {
        (Vec::new(), Vec::new(), 0.0)
    } else {
        place_items(&items, 0.0, 0.0, width, style.text_align)
    };
    LayoutBox {
        node,
        rect: Rect::new(0.0, 0.0, width, height),
        margin: Edges::ZERO,
        background: mithu_base::Color::TRANSPARENT,
        border_color: style.color,
        border_width: 0.0,
        color: style.color,
        font_size: style.font_size,
        bold: style.bold,
        mono: style.mono,
        href,
        marker: None,
        children,
        lines,
        opacity: 1.0,
        overflow_hidden: false,
        border_radius: 0.0,
    }
}

fn layout_absolute_children(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    content_h: f32,
    href: Option<String>,
) -> Vec<LayoutBox> {
    let mut placed = Vec::new();
    for child in doc.node(id).children.clone() {
        if !is_absolute(doc, styles, child) {
            continue;
        }
        let Some(mut layout) = layout_node(
            doc,
            styles,
            measure,
            child,
            content_x,
            content_y,
            content_w,
            href.clone(),
            false,
        ) else {
            continue;
        };
        let style = style_of(styles, child);
        let font = style.font_size;
        let dx = if let Some(left) = resolve_length(style.insets[3], font, content_w) {
            content_x + left - layout.rect.x
        } else if let Some(right) = resolve_length(style.insets[1], font, content_w) {
            content_x + content_w - layout.rect.width - right - layout.rect.x
        } else {
            0.0
        };
        let dy = if let Some(top) = resolve_length(style.insets[0], font, content_w) {
            content_y + top - layout.rect.y
        } else if let Some(bottom) = resolve_length(style.insets[2], font, content_w) {
            content_y + content_h - layout.rect.height - bottom - layout.rect.y
        } else {
            0.0
        };
        translate(&mut layout, dx, dy);
        placed.push(layout);
    }
    placed
}

fn empty_box(node: NodeId) -> LayoutBox {
    LayoutBox {
        node,
        rect: Rect::default(),
        margin: Edges::ZERO,
        background: mithu_base::Color::TRANSPARENT,
        border_color: mithu_base::Color::BLACK,
        border_width: 0.0,
        color: mithu_base::Color::BLACK,
        font_size: 16.0,
        bold: false,
        mono: false,
        href: None,
        marker: None,
        children: Vec::new(),
        lines: Vec::new(),
        opacity: 1.0,
        overflow_hidden: false,
        border_radius: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithu_html::parse_html;
    use mithu_style::style_document;
    use mithu_text::NaiveMetrics;

    fn find<'a>(box_: &'a LayoutBox, id: NodeId) -> Option<&'a LayoutBox> {
        if box_.node == id {
            return Some(box_);
        }
        box_.children.iter().find_map(|child| find(child, id))
    }

    #[test]
    fn sizes_an_explicit_block() {
        let doc = parse_html(r#"<div id="box" style="width:100px;height:50px"></div>"#);
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, &NaiveMetrics);
        let id = doc.get_element_by_id("box").unwrap();
        let found = find(&tree.root, id).unwrap();
        assert!((found.rect.x - 8.0).abs() < 0.5, "x {}", found.rect.x);
        assert!((found.rect.y - 8.0).abs() < 0.5, "y {}", found.rect.y);
        assert!((found.rect.width - 100.0).abs() < 0.5, "w {}", found.rect.width);
        assert!((found.rect.height - 50.0).abs() < 0.5, "h {}", found.rect.height);
    }

    #[test]
    fn flex_row_and_absolute_child() {
        let doc = parse_html(
            r#"<div id="row" style="display:flex;gap:10px">
                <div id="a" style="width:40px;height:20px"></div>
                <div id="b" style="width:60px;height:20px"></div>
            </div>
            <div id="host" style="position:relative;width:200px;height:80px">
                <div id="pin" style="position:absolute;top:12px;left:30px;width:10px;height:10px"></div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, &NaiveMetrics);
        let row = find(&tree.root, doc.get_element_by_id("row").unwrap()).unwrap();
        let a = find(row, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(row, doc.get_element_by_id("b").unwrap()).unwrap();
        assert!((b.rect.x - (a.rect.x + 40.0 + 10.0)).abs() < 0.5, "gap {}", b.rect.x - a.rect.x);
        let host = find(&tree.root, doc.get_element_by_id("host").unwrap()).unwrap();
        let pin = find(host, doc.get_element_by_id("pin").unwrap()).unwrap();
        assert!((pin.rect.x - (host.rect.x + 30.0)).abs() < 0.5, "x {}", pin.rect.x);
        assert!((pin.rect.y - (host.rect.y + 12.0)).abs() < 0.5, "y {}", pin.rect.y);
    }
}
