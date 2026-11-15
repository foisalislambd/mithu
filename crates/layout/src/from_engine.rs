//! Build Mithu's paint tree from a Stylo + Taffy + Parley frame.

use std::collections::HashMap;
use std::sync::Arc;

use mithu_base::Rect;
use mithu_dom::{Document, NodeId};
use mithu_image::Bitmap;
use mithu_style::{
    ComputedStyle, Display, Frame, Length, PlacedFragment, PlacedLine, Position, resolve,
};
use mithu_text::MeasureText;

use crate::{LayoutBox, LayoutTree, LineBox, TextFragment, VectorShape};

pub(crate) fn layout(
    doc: &Document,
    passed: &[ComputedStyle],
    viewport_width: f32,
    viewport_height: f32,
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
) -> LayoutTree {
    let fonts = measure.web_fonts();
    let sizes: Vec<_> = images
        .iter()
        .map(|(id, bitmap)| (*id, bitmap.width, bitmap.height))
        .collect();
    let (css, hover) = mithu_style::remembered();
    let frame = resolve(
        doc,
        &css,
        mithu_style::StyleState {
            hover,
            viewport_width,
            ..mithu_style::StyleState::default()
        },
        viewport_height,
        &sizes,
        &fonts,
    );
    let _ = passed;
    // Author sheets already live on the document. Callers that passed CSS
    // through `style_document` have those declarations on the elements, but
    // the cascade has to see the same sheet again. `layout` is invoked with
    // the styled document, whose `<style>` elements Stylo reads directly.
    let _ = &frame.styles;
    let root_id = document_element(doc).unwrap_or(doc.root);
    let mut root = walk(doc, &frame, images, root_id, None).unwrap_or_else(|| empty(root_id));
    if root.rect.width <= 0.0 {
        root.rect.width = viewport_width;
    }
    let content_height = content_bottom(&root);
    let mut links = Vec::new();
    collect_links(&root, &mut links);
    LayoutTree {
        background: frame.background,
        root,
        links,
        content_height,
    }
}

fn document_element(doc: &Document) -> Option<NodeId> {
    doc.node(doc.root).children.iter().copied().find(|id| {
        matches!(doc.element_name(*id), Some("html"))
    }).or_else(|| {
        doc.node(doc.root)
            .children
            .first()
            .copied()
            .filter(|id| doc.element_name(*id).is_some())
    })
}

fn walk(
    doc: &Document,
    frame: &Frame,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    href: Option<String>,
) -> Option<LayoutBox> {
    let style = frame
        .styles
        .get(id.0)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);
    if style.display == Display::None || !style.visible && style.display == Display::None {
        return None;
    }
    let source = shadow_source(doc, id);
    if style.display == Display::Contents {
        let mut boxes = Vec::new();
        for child in children_of(doc, source) {
            if let Some(layout) = walk(doc, frame, images, child, href.clone()) {
                boxes.push(layout);
            }
        }
        return Some(LayoutBox {
            children: boxes,
            ..empty(id)
        });
    }
    let placed = frame.placed.get(id.0);
    let has_box = placed.is_some_and(|item| item.has_box);
    if !has_box && doc.element_name(id).is_none() {
        return None;
    }
    let href = own_href(doc, id).or(href);
    let mut children = Vec::new();
    for child in children_of(doc, source) {
        if let Some(layout) = walk(doc, frame, images, child, href.clone()) {
            if layout.rect.width > 0.0
                || layout.rect.height > 0.0
                || !layout.lines.is_empty()
                || !layout.children.is_empty()
                || !layout.shapes.is_empty()
            {
                children.push(layout);
            }
        }
    }
    let rect = placed.map(|item| item.rect).unwrap_or_default();
    let lines = placed
        .map(|item| paint_lines(doc, &item.lines, href.clone()))
        .unwrap_or_default();
    let svg_bitmap = if doc.element_name(id) == Some("svg") {
        let markup = crate::svg::markup(doc, id, &frame.styles, style.color);
        mithu_image::decode_svg(markup.as_bytes())
    } else {
        None
    };
    let shapes = if svg_bitmap.is_some() {
        Vec::new()
    } else if doc.element_name(id) == Some("svg") {
        crate::svg::shapes(
            doc,
            &frame.styles,
            id,
            rect.width.max(1.0),
            rect.height.max(1.0),
            style.color,
        )
    } else {
        Vec::new()
    };
    let mut layout = box_from(
        doc, id, &style, placed, rect, href, children, lines, shapes, images,
    );
    if let Some(bitmap) = svg_bitmap {
        layout.image = Some(Arc::new(bitmap));
        layout.children.clear();
    }
    Some(layout)
}

fn box_from(
    _doc: &Document,
    id: NodeId,
    style: &ComputedStyle,
    placed: Option<&mithu_style::Placed>,
    rect: Rect,
    href: Option<String>,
    children: Vec<LayoutBox>,
    lines: Vec<LineBox>,
    shapes: Vec<VectorShape>,
    images: &HashMap<NodeId, Arc<Bitmap>>,
) -> LayoutBox {
    let borders = placed.map(|item| item.borders).unwrap_or(style.borders);
    LayoutBox {
        node: id,
        rect,
        margin: placed.map(|item| item.margin).unwrap_or_default(),
        background: style.background,
        border_color: style.border_color,
        border_width: borders.into_iter().fold(0.0, f32::max),
        color: style.color,
        font_size: style.font_size,
        bold: style.bold,
        mono: style.mono,
        href: href.clone(),
        marker: None,
        children,
        lines,
        opacity: style.opacity,
        overflow_hidden: style.overflow_hidden,
        border_radius: style.border_radius,
        image: images.get(&id).cloned(),
        visible: style.visible,
        z_index: style.z_index,
        borders,
        fixed: style.position == Position::Fixed,
        gradient: style.gradient,
        shapes,
        sticky: style.position == Position::Sticky,
        sticky_top: sticky_inset(style.insets[0]),
        sticky_bottom: sticky_inset(style.insets[2]),
        shadow: style.shadow,
        frame: None,
    }
}

fn sticky_inset(length: Length) -> Option<f32> {
    match length {
        Length::Px(value) => Some(value),
        Length::Auto => None,
        Length::Percent(value) => Some(value),
        _ => None,
    }
}

fn paint_lines(doc: &Document, lines: &[PlacedLine], href: Option<String>) -> Vec<LineBox> {
    lines
        .iter()
        .map(|line| LineBox {
            baseline: line.baseline,
            ascent: line.ascent,
            height: line.height,
            fragments: line
                .fragments
                .iter()
                .map(|fragment| {
                    let link = href_at(doc, fragment.node).or_else(|| href.clone());
                    fragment_of(fragment, link)
                })
                .collect(),
        })
        .collect()
}

fn href_at(doc: &Document, id: NodeId) -> Option<String> {
    let mut current = Some(id);
    while let Some(node) = current {
        if let Some(href) = own_href(doc, node) {
            return Some(href);
        }
        current = doc.nodes.get(node.0).and_then(|item| item.parent);
    }
    None
}

fn fragment_of(fragment: &PlacedFragment, href: Option<String>) -> TextFragment {
    TextFragment {
        x: fragment.x,
        text: fragment.text.clone(),
        color: fragment.color,
        size: fragment.size,
        bold: fragment.bold,
        mono: fragment.mono,
        family: fragment.family.clone(),
        underline: fragment.underline,
        href,
        width: fragment.width,
        node: fragment.node,
    }
}

fn children_of(doc: &Document, id: NodeId) -> Vec<NodeId> {
    doc.nodes
        .get(id.0)
        .map(|node| node.children.clone())
        .unwrap_or_default()
}

fn shadow_source(doc: &Document, id: NodeId) -> NodeId {
    doc.attr(id, "mithu-shadow")
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|index| *index < doc.nodes.len())
        .map(NodeId)
        .unwrap_or(id)
}

fn own_href(doc: &Document, id: NodeId) -> Option<String> {
    if doc.element_name(id) == Some("a") {
        doc.attr(id, "href")
            .filter(|href| !href.is_empty())
            .map(str::to_string)
    } else {
        None
    }
}

fn empty(id: NodeId) -> LayoutBox {
    LayoutBox {
        node: id,
        rect: Rect::default(),
        margin: mithu_base::Edges::ZERO,
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
        image: None,
        visible: true,
        z_index: 0,
        borders: [0.0; 4],
        fixed: false,
        gradient: None,
        shapes: Vec::new(),
        sticky: false,
        sticky_top: None,
        sticky_bottom: None,
        shadow: None,
        frame: None,
    }
}

fn content_bottom(box_: &LayoutBox) -> f32 {
    let mut bottom = box_.rect.bottom() + box_.margin.bottom;
    for child in &box_.children {
        bottom = bottom.max(content_bottom(child));
    }
    bottom
}

fn collect_links(box_: &LayoutBox, links: &mut Vec<crate::Link>) {
    if let Some(href) = &box_.href {
        links.push(crate::Link {
            href: href.clone(),
            rect: box_.rect,
        });
    }
    for line in &box_.lines {
        for fragment in &line.fragments {
            if let Some(href) = &fragment.href {
                links.push(crate::Link {
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
        collect_links(child, links);
    }
}
