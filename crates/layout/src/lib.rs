//! Block and inline layout.
//!
//! Block boxes fill their containing block. Inline content wraps into line
//! boxes. Inline-block and replaced elements (images, inputs, frames) become
//! atomic boxes. Floats sit beside the following blocks, and grid places
//! children into tracks. Margin collapsing is not implemented.

use std::collections::HashMap;
use std::sync::Arc;

use mithu_base::{Edges, Rect};
use mithu_dom::{Document, NodeData, NodeId};
use mithu_image::Bitmap;
use mithu_style::{
    AlignItems, BoxSizing, Clear, ComputedStyle, Display, FlexDirection, FloatSide, Generated,
    GridTrack, Justify, Length, LineHeight, Position, TextAlign, TextTransform, WhiteSpace,
};
use mithu_text::MeasureText;

mod svg;

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
    pub image: Option<Arc<Bitmap>>,
    pub visible: bool,
    pub z_index: i32,
    pub borders: [f32; 4],
    pub fixed: bool,
    pub gradient: Option<(mithu_base::Color, mithu_base::Color)>,
    pub shapes: Vec<VectorShape>,
    /// `position: sticky`. The box stays in flow; paint pins it.
    pub sticky: bool,
    pub sticky_top: Option<f32>,
    pub sticky_bottom: Option<f32>,
    pub shadow: Option<mithu_style::BoxShadow>,
    /// A nested document painted inside an iframe.
    pub frame: Option<Box<LayoutTree>>,
}

/// One filled or stroked contour in the border box's local coordinates.
#[derive(Clone, Debug)]
pub struct VectorShape {
    pub fill: Option<mithu_base::Color>,
    pub stroke: Option<mithu_base::Color>,
    pub stroke_width: f32,
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Copy)]
struct View {
    width: f32,
    height: f32,
    root: f32,
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
    pub family: String,
    pub underline: bool,
    pub href: Option<String>,
    pub width: f32,
    pub node: NodeId,
}

pub fn layout_tree(
    doc: &Document,
    styles: &[ComputedStyle],
    viewport_width: f32,
    viewport_height: f32,
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
) -> LayoutTree {
    let root_id = document_element(doc).unwrap_or(doc.root);
    let view = View {
        width: viewport_width.max(1.0),
        height: viewport_height.max(1.0),
        root: styles
            .get(root_id.0)
            .map(|style| style.font_size)
            .unwrap_or(16.0),
    };
    let root = layout_node(
        doc,
        styles,
        measure,
        images,
        root_id,
        0.0,
        0.0,
        viewport_width.max(1.0),
        None,
        false,
        view,
    )
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
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    containing_x: f32,
    margin_y: f32,
    containing_width: f32,
    inherited_href: Option<String>,
    shrink_to_fit: bool,
    view: View,
) -> Option<LayoutBox> {
    let mut style = style_of(styles, id);
    let image = (doc.element_name(id) == Some("img"))
        .then(|| images.get(&id).cloned())
        .flatten();
    if doc.element_name(id) == Some("img") {
        if let Some(bitmap) = &image {
            apply_intrinsic(&mut style, bitmap);
        } else {
            // The HTML width/height attributes are the intrinsic size until the
            // bitmap arrives. A missing image is 300×150 only when both are absent.
            let attr_w = attribute_px(doc, id, "width");
            let attr_h = attribute_px(doc, id, "height");
            if matches!(style.width, Length::Auto) {
                style.width = Length::Px(attr_w.unwrap_or(300.0));
            }
            if matches!(style.height, Length::Auto) {
                style.height = Length::Px(attr_h.unwrap_or(150.0));
            }
        }
    }
    let display = effective_display(doc, styles, id);
    if display == Display::None || display == Display::Contents {
        return None;
    }
    let href = own_href(doc, id).or(inherited_href);
    if doc.element_name(id) == Some("table") {
        return layout_table(
            doc,
            styles,
            measure,
            images,
            id,
            containing_x,
            margin_y,
            containing_width,
            href,
            view,
        );
    }
    if doc.element_name(id) == Some("svg") {
        svg::apply_size(&mut style, doc, id);
    }
    let font = style.font_size;
    let borders = [
        style.borders[0].max(0.0),
        style.borders[1].max(0.0),
        style.borders[2].max(0.0),
        style.borders[3].max(0.0),
    ];
    let border_w = borders[1] + borders[3];
    let border_h = borders[0] + borders[2];
    let padding = resolve_edges(&style.padding, font, containing_width, view);
    let width_is_auto = matches!(style.width, Length::Auto);
    let width_opt = resolve_length(style.width, font, containing_width, view).map(|width| {
        if style.box_sizing == BoxSizing::BorderBox {
            (width - padding.left - padding.right - border_w).max(0.0)
        } else {
            width.max(0.0)
        }
    });
    let margin_top = resolve_length(style.margin[0], font, containing_width, view)
        .unwrap_or(0.0)
        .max(0.0);
    let margin_bottom = resolve_length(style.margin[2], font, containing_width, view)
        .unwrap_or(0.0)
        .max(0.0);
    let margin_left_opt = resolve_length(style.margin[3], font, containing_width, view);
    let margin_right_opt = resolve_length(style.margin[1], font, containing_width, view);

    let (margin_left, margin_right, mut content_w) = if let Some(width) = width_opt {
        let space = containing_width - width - padding.left - padding.right - border_w;
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
        let content =
            (containing_width - left - right - padding.left - padding.right - border_w).max(0.0);
        (left, right, content)
    };

    let shrink = width_is_auto
        && (display == Display::InlineBlock || display == Display::InlineFlex || shrink_to_fit);
    let border_x = containing_x + margin_left;
    let border_y = margin_y + margin_top;
    let content_x = border_x + borders[3] + padding.left;
    let content_y = border_y + borders[0] + padding.top;

    content_w = limit_content(
        &style,
        content_w,
        font,
        containing_width,
        padding.left + padding.right,
        border_w,
        view,
    );
    let synthetic = if image.is_some() {
        None
    } else {
        synthetic_text(doc, id)
    };
    let blockish = synthetic.is_none() && has_block_child(doc, styles, id);
    let inline_flex = display == Display::InlineFlex;
    // A float (and inline-flex) with `width: auto` uses the shrink-to-fit width.
    // Stretching it to the line is what made Wikipedia's star icon push the
    // tagline into a one-word column.
    let fit = shrink_to_fit || inline_flex;
    let (lines, mut children, flowed) = if display == Display::Flex || inline_flex {
        let mut laid = layout_flex(
            doc,
            styles,
            measure,
            images,
            id,
            &style,
            content_x,
            content_y,
            content_w,
            href.clone(),
            view,
        );
        if fit && width_is_auto {
            let gaps = style.gap.max(0.0) * laid.1.len().saturating_sub(1) as f32;
            let used = laid
                .1
                .iter()
                .map(|child| child.margin.left + child.rect.width + child.margin.right)
                .sum::<f32>()
                + gaps;
            if used + 0.5 < content_w {
                content_w = used.max(0.0);
                laid = layout_flex(
                    doc,
                    styles,
                    measure,
                    images,
                    id,
                    &style,
                    content_x,
                    content_y,
                    content_w,
                    href.clone(),
                    view,
                );
            }
        }
        laid
    } else if display == Display::Grid {
        layout_grid(
            doc,
            styles,
            measure,
            images,
            id,
            &style,
            content_x,
            content_y,
            content_w,
            href.clone(),
            view,
        )
    } else if blockish {
        if shrink && width_is_auto {
            let intrinsic = max_content_width(doc, styles, measure, id, content_w);
            if intrinsic + 0.5 < content_w {
                content_w = intrinsic;
            }
        }
        layout_blocks(
            doc,
            styles,
            measure,
            images,
            id,
            &style,
            content_x,
            content_y,
            content_w,
            href.clone(),
            view,
        )
    } else {
        let mut items = if let Some(text) = &synthetic {
            items_from_text(text, &style, measure, href.clone(), id)
        } else {
            collect_children(
                doc,
                styles,
                measure,
                images,
                id,
                &style,
                content_w,
                href.clone(),
                view,
            )
        };
        items = surround_items(items, &style, measure, href.clone(), id, content_w);
        if shrink && !items.is_empty() {
            let cap = if style.white_space == WhiteSpace::Nowrap {
                f32::MAX / 4.0
            } else {
                content_w
            };
            content_w = preferred_width(&items, cap);
            content_w = limit_content(
                &style,
                content_w,
                font,
                containing_width,
                padding.left + padding.right,
                border_w,
                view,
            );
        }
        if is_blank(&items) {
            (Vec::new(), Vec::new(), 0.0)
        } else {
            place_items(
                &items,
                content_x,
                content_y,
                wrap_width(&style, content_w),
                style.text_align,
            )
        }
    };

    let mut specified_height = resolve_height(style.height, font, view);
    if style.box_sizing == BoxSizing::BorderBox
        && let Some(height) = specified_height
    {
        specified_height = Some((height - padding.top - padding.bottom - border_h).max(0.0));
    }
    // A specified height is the border box's content size. Content that does
    // not fit overflows; it does not stretch the box the way `height: auto` does.
    let mut content_h = specified_height.unwrap_or(flowed);
    if let Some(max) = resolve_height(style.max_height, font, view) {
        content_h = content_h.min(max);
    }
    if let Some(min) = resolve_height(style.min_height, font, view) {
        content_h = content_h.max(min);
    }
    children.extend(layout_absolute_children(
        doc,
        styles,
        measure,
        images,
        id,
        content_x,
        content_y,
        content_w,
        content_h,
        href.clone(),
        view,
    ));
    let rect = Rect::new(
        border_x,
        border_y,
        content_w + padding.left + padding.right + border_w,
        content_h + padding.top + padding.bottom + border_h,
    );
    let marker = (display == Display::ListItem).then(|| list_marker(doc, id));
    let mut laid_out = LayoutBox {
        node: id,
        rect,
        margin: Edges::new(margin_top, margin_right, margin_bottom, margin_left),
        background: style.background,
        gradient: style.gradient,
        border_color: style.border_color,
        border_width: borders[0],
        borders,
        fixed: style.position == Position::Fixed,
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
        image,
        visible: style.visible,
        z_index: if style.position == Position::Static {
            0
        } else {
            style.z_index
        },
        shapes: Vec::new(),
        sticky: style.position == Position::Sticky,
        sticky_top: resolve_length(style.insets[0], font, view.height, view),
        sticky_bottom: resolve_length(style.insets[2], font, view.height, view),
        shadow: style.shadow,
        frame: None,
    };
    if doc.element_name(id) == Some("svg") {
        let shapes = svg::shapes(
            doc,
            styles,
            id,
            laid_out.rect.width,
            laid_out.rect.height,
            style.color,
        );
        if !shapes.is_empty() {
            laid_out.background = mithu_base::Color::TRANSPARENT;
            laid_out.border_width = 0.0;
            laid_out.borders = [0.0; 4];
            laid_out.shapes = shapes;
        }
    }
    if style.position == Position::Relative {
        let dx = resolve_length(style.insets[3], font, containing_width, view)
            .or_else(|| {
                resolve_length(style.insets[1], font, containing_width, view).map(|right| -right)
            })
            .unwrap_or(0.0);
        let dy = resolve_length(style.insets[0], font, containing_width, view)
            .or_else(|| {
                resolve_length(style.insets[2], font, containing_width, view).map(|bottom| -bottom)
            })
            .unwrap_or(0.0);
        translate(&mut laid_out, dx, dy);
    }
    // A definite block or grid area keeps its containing width. Growing it to
    // overflowing children is what stretched Wikipedia's toolbar across the
    // article column. Shrink-to-fit boxes still grow to their max-content.
    if shrink || matches!(display, Display::InlineBlock | Display::InlineFlex) {
        expand_ink(&mut laid_out);
    }
    Some(laid_out)
}

fn limit_content(
    style: &ComputedStyle,
    content_w: f32,
    font: f32,
    containing: f32,
    padding_x: f32,
    border: f32,
    view: View,
) -> f32 {
    let adjust = |length: f32| {
        if style.box_sizing == BoxSizing::BorderBox {
            (length - padding_x - border).max(0.0)
        } else {
            length.max(0.0)
        }
    };
    let mut width = content_w;
    if let Some(max) = resolve_length(style.max_width, font, containing, view) {
        width = width.min(adjust(max));
    }
    if let Some(min) = resolve_length(style.min_width, font, containing, view) {
        width = width.max(adjust(min));
    }
    width.max(0.0)
}

fn layout_blocks(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    href: Option<String>,
    view: View,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let mut y = content_y;
    let mut group = Vec::new();
    let mut lines = Vec::new();
    let mut children = Vec::new();
    if let Some(before) = generated_block(
        id,
        style.before.as_ref(),
        style,
        measure,
        href.clone(),
        content_x,
        y,
        content_w,
    ) {
        y = before.rect.bottom();
        children.push(before);
    }
    let flush = |group: &mut Vec<NodeId>,
                 y: &mut f32,
                 lines: &mut Vec<LineBox>,
                 children: &mut Vec<LayoutBox>| {
        if group.is_empty() {
            return;
        }
        let mut items = Vec::new();
        for child in group.drain(..) {
            items.extend(collect_node(
                doc,
                styles,
                measure,
                images,
                child,
                style,
                content_w,
                href.clone(),
                view,
            ));
        }
        if is_blank(&items) {
            return;
        }
        let (new_lines, atoms, height) = place_items(
            &items,
            content_x,
            *y,
            wrap_width(style, content_w),
            style.text_align,
        );
        lines.extend(new_lines);
        children.extend(atoms);
        *y += height;
    };
    let mut floats = Vec::new();
    for child in box_children(doc, styles, id) {
        let floated = style_of(styles, child).float;
        if matches!(floated, FloatSide::Left | FloatSide::Right) && !is_skipped(doc, styles, child)
        {
            flush(&mut group, &mut y, &mut lines, &mut children);
            if let Some(placed) = place_float(
                doc,
                styles,
                measure,
                images,
                child,
                floated,
                content_x,
                &mut y,
                content_w,
                href.clone(),
                view,
                &mut floats,
            ) {
                children.push(placed);
            }
        } else if is_block_child(doc, styles, child) {
            flush(&mut group, &mut y, &mut lines, &mut children);
            y = clear_to(y, style_of(styles, child).clear, &floats);
            let (band_x, band_w) = float_band(y, content_x, content_w, &floats);
            if let Some(child_box) = layout_node(
                doc,
                styles,
                measure,
                images,
                child,
                band_x,
                y,
                band_w,
                href.clone(),
                false,
                view,
            ) {
                y = child_box.rect.bottom() + child_box.margin.bottom;
                children.push(child_box);
            }
        } else if !is_skipped(doc, styles, child) {
            group.push(child);
        }
    }
    flush(&mut group, &mut y, &mut lines, &mut children);
    if let Some(after) = generated_block(
        id,
        style.after.as_ref(),
        style,
        measure,
        href.clone(),
        content_x,
        y,
        content_w,
    ) {
        y = after.rect.bottom();
        children.push(after);
    }
    (lines, children, (y - content_y).max(0.0))
}

struct FloatBox {
    side: FloatSide,
    bottom: f32,
    edge: f32,
}

fn float_band(y: f32, content_x: f32, content_w: f32, floats: &[FloatBox]) -> (f32, f32) {
    let mut left = content_x;
    let mut right = content_x + content_w;
    for float_box in floats {
        if y + 0.5 >= float_box.bottom {
            continue;
        }
        if float_box.side == FloatSide::Left {
            left = left.max(float_box.edge);
        } else {
            right = right.min(float_box.edge);
        }
    }
    (left, (right - left).max(0.0))
}

fn clear_to(y: f32, clear: Clear, floats: &[FloatBox]) -> f32 {
    let mut next = y;
    for float_box in floats {
        let hit = match clear {
            Clear::None => false,
            Clear::Both => true,
            Clear::Left => float_box.side == FloatSide::Left,
            Clear::Right => float_box.side == FloatSide::Right,
        };
        if hit {
            next = next.max(float_box.bottom);
        }
    }
    next
}

fn place_float(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    side: FloatSide,
    content_x: f32,
    y: &mut f32,
    content_w: f32,
    href: Option<String>,
    view: View,
    floats: &mut Vec<FloatBox>,
) -> Option<LayoutBox> {
    let mut cursor = *y;
    for _ in 0..4 {
        let (band_x, band_w) = float_band(cursor, content_x, content_w, floats);
        let shrink = matches!(style_of(styles, id).width, Length::Auto);
        let mut layout = layout_node(
            doc,
            styles,
            measure,
            images,
            id,
            band_x,
            cursor,
            band_w.max(1.0),
            href.clone(),
            shrink,
            view,
        )?;
        let outer = layout.margin.left + layout.rect.width + layout.margin.right;
        if outer > band_w + 0.5 && band_w + 0.5 < content_w {
            cursor = floats
                .iter()
                .map(|float_box| float_box.bottom)
                .fold(cursor, f32::max)
                + 0.1;
            continue;
        }
        if side == FloatSide::Right {
            let target = band_x + band_w - outer;
            let dx = target - layout.rect.x + layout.margin.left;
            translate(&mut layout, dx, 0.0);
        }
        let bottom = layout.rect.bottom() + layout.margin.bottom;
        let edge = if side == FloatSide::Left {
            layout.rect.right() + layout.margin.right
        } else {
            layout.rect.x - layout.margin.left
        };
        floats.push(FloatBox { side, bottom, edge });
        return Some(layout);
    }
    None
}

fn layout_grid(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    href: Option<String>,
    view: View,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let gap = style.gap.max(0.0);
    let explicit_rows = style.grid_rows.len();
    let mut cells = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut cursor_col = 0usize;
    let mut cursor_row = 0usize;
    let mut max_column = style.grid_columns.len().max(1);
    for child in box_children(doc, styles, id) {
        if is_skipped(doc, styles, child) {
            continue;
        }
        if matches!(&doc.node(child).data, NodeData::Text(text) if text.trim().is_empty()) {
            continue;
        }
        let area = style_of(styles, child);
        let named = find_grid_area(&style.grid_areas, &area.grid_area);
        let span = (named
            .map(|(_, _, span, _)| span)
            .unwrap_or(area.grid_column.span as usize))
        .max(1);
        let row_span = (named
            .map(|(_, _, _, span)| span)
            .unwrap_or(area.grid_row.span as usize))
        .max(1);
        let explicit_col = named.map(|(column, _, _, _)| column).or_else(|| {
            area.grid_column
                .start
                .map(|line| (line as usize).saturating_sub(1))
        });
        let explicit_row = named.map(|(_, row, _, _)| row).or_else(|| {
            area.grid_row
                .start
                .map(|line| (line as usize).saturating_sub(1))
        });
        let columns_now = max_column.max(explicit_col.unwrap_or(0) + span);
        let (column, row) = if style.column_flow
            && explicit_rows > 0
            && explicit_col.is_none()
            && explicit_row.is_none()
        {
            if cursor_row + row_span > explicit_rows && cursor_row != 0 {
                cursor_row = 0;
                cursor_col += 1;
            }
            let placed = (cursor_col, cursor_row);
            cursor_row += row_span;
            if cursor_row >= explicit_rows {
                cursor_row = 0;
                cursor_col += 1;
            }
            placed
        } else if let (Some(column), Some(row)) = (explicit_col, explicit_row) {
            (column, row)
        } else if let Some(column) = explicit_col {
            let mut row = 0usize;
            while grid_occupied(&occupied, row, column, row_span, span) {
                row += 1;
            }
            (column, row)
        } else if let Some(row) = explicit_row {
            let mut column = 0usize;
            while column + span <= columns_now.max(column + span)
                && grid_occupied(&occupied, row, column, row_span, span)
            {
                column += 1;
            }
            (column, row)
        } else {
            let limit = if style.grid_columns.is_empty() && style.auto_columns.is_empty() {
                1
            } else {
                style.grid_columns.len().max(1)
            };
            if cursor_col + span > limit && cursor_col != 0 {
                cursor_col = 0;
                cursor_row += 1;
            }
            while grid_occupied(&occupied, cursor_row, cursor_col, row_span, span) {
                cursor_col += 1;
                if cursor_col + span > limit {
                    cursor_col = 0;
                    cursor_row += 1;
                }
            }
            let placed = (cursor_col, cursor_row);
            cursor_col += span;
            if cursor_col >= limit {
                cursor_col = 0;
                cursor_row += 1;
            }
            placed
        };
        max_column = max_column.max(column + span);
        grid_mark(
            &mut occupied,
            row,
            column,
            row_span,
            span,
            max_column.max(column + span),
        );
        cells.push((child, column, span, row, row_span));
    }
    let tracks = if !style.grid_columns.is_empty() {
        let mut tracks = style.grid_columns.clone();
        let fill = style
            .auto_columns
            .first()
            .cloned()
            .unwrap_or(GridTrack::Auto);
        while tracks.len() < max_column {
            tracks.push(fill.clone());
        }
        tracks
    } else if !style.auto_columns.is_empty() {
        let fill = style.auto_columns[0].clone();
        vec![fill; max_column.max(1)]
    } else if max_column > 1 {
        vec![GridTrack::Fr(1.0); max_column]
    } else {
        vec![GridTrack::Fr(1.0)]
    };
    let widths = resolve_tracks(&tracks, content_w, gap);
    let row_count = cells
        .iter()
        .map(|(_, _, _, row, row_span)| *row + *row_span)
        .max()
        .unwrap_or(0);
    let mut row_heights = vec![0.0_f32; row_count];
    let mut laid = Vec::new();
    for (child, column, span, row, row_span) in cells {
        let column = column.min(widths.len().saturating_sub(1));
        let span = span.min(widths.len() - column).max(1);
        let x = content_x + widths[..column].iter().sum::<f32>() + gap * column as f32;
        let width =
            widths[column..column + span].iter().sum::<f32>() + gap * span.saturating_sub(1) as f32;
        if let Some(mut layout) = layout_node(
            doc,
            styles,
            measure,
            images,
            child,
            0.0,
            0.0,
            width.max(0.0),
            href.clone(),
            false,
            view,
        ) {
            let dx = x - layout.rect.x + layout.margin.left;
            translate(&mut layout, dx, 0.0);
            let outer = layout.margin.top + layout.rect.height + layout.margin.bottom;
            laid.push((row, row_span, outer, layout));
        }
    }
    for (row, row_span, outer, _) in &laid {
        if *row_span <= 1 && *row < row_heights.len() {
            row_heights[*row] = row_heights[*row].max(*outer);
        }
    }
    for (row, row_span, outer, _) in &laid {
        if *row_span <= 1 || row_count == 0 {
            continue;
        }
        let end = (*row + *row_span).min(row_count).max(*row + 1);
        let gaps = gap * (end - *row).saturating_sub(1) as f32;
        let have = row_heights[*row..end].iter().sum::<f32>() + gaps;
        if *outer > have {
            let last = end - 1;
            row_heights[last] += *outer - have;
        }
    }
    let mut y = content_y;
    let mut row_y = vec![content_y; row_count];
    for row in 0..row_count {
        row_y[row] = y;
        y += row_heights[row];
        if row + 1 < row_count {
            y += gap;
        }
    }
    let mut placed = Vec::new();
    for (row, _, _, mut layout) in laid {
        let dy = row_y[row] + layout.margin.top - layout.rect.y;
        translate(&mut layout, 0.0, dy);
        placed.push(layout);
    }
    let height = if placed.is_empty() {
        0.0
    } else {
        (y - content_y).max(0.0)
    };
    (Vec::new(), placed, height)
}

fn find_grid_area(areas: &[Vec<String>], name: &str) -> Option<(usize, usize, usize, usize)> {
    if name.is_empty() {
        return None;
    }
    let mut min_col = usize::MAX;
    let mut min_row = usize::MAX;
    let mut max_col = 0usize;
    let mut max_row = 0usize;
    let mut found = false;
    for (row, line) in areas.iter().enumerate() {
        for (column, cell) in line.iter().enumerate() {
            if cell == name {
                found = true;
                min_col = min_col.min(column);
                min_row = min_row.min(row);
                max_col = max_col.max(column);
                max_row = max_row.max(row);
            }
        }
    }
    found.then_some((
        min_col,
        min_row,
        max_col - min_col + 1,
        max_row - min_row + 1,
    ))
}

fn grid_occupied(
    occupied: &[Vec<bool>],
    row: usize,
    column: usize,
    row_span: usize,
    span: usize,
) -> bool {
    for row in row..row + row_span {
        let Some(line) = occupied.get(row) else {
            continue;
        };
        for column in column..column + span {
            if line.get(column).copied().unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn grid_mark(
    occupied: &mut Vec<Vec<bool>>,
    row: usize,
    column: usize,
    row_span: usize,
    span: usize,
    columns: usize,
) {
    let rows = row + row_span;
    let width = columns.max(column + span);
    while occupied.len() < rows {
        occupied.push(vec![false; width]);
    }
    for row in row..rows {
        if occupied[row].len() < width {
            occupied[row].resize(width, false);
        }
        for column in column..(column + span).min(width) {
            occupied[row][column] = true;
        }
    }
}

fn resolve_tracks(tracks: &[GridTrack], content_w: f32, gap: f32) -> Vec<f32> {
    if tracks.is_empty() {
        return vec![content_w.max(0.0)];
    }
    let gaps = gap * tracks.len().saturating_sub(1) as f32;
    let fixed: f32 = tracks
        .iter()
        .map(|track| match track {
            GridTrack::Px(px) => px.max(0.0),
            _ => 0.0,
        })
        .sum();
    let free = (content_w - gaps - fixed).max(0.0);
    let fr: f32 = tracks
        .iter()
        .map(|track| match track {
            GridTrack::Fr(value) => value.max(0.0),
            GridTrack::Auto => 1.0,
            GridTrack::Px(_) => 0.0,
        })
        .sum();
    tracks
        .iter()
        .map(|track| match track {
            GridTrack::Px(px) => px.max(0.0),
            GridTrack::Fr(value) if fr > 0.0 => free * value.max(0.0) / fr,
            GridTrack::Auto if fr > 0.0 => free / fr,
            GridTrack::Fr(_) | GridTrack::Auto => 0.0,
        })
        .collect()
}

fn collect_children(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    style: &ComputedStyle,
    available: f32,
    href: Option<String>,
    view: View,
) -> Vec<Item> {
    let mut items = Vec::new();
    for child in box_children(doc, styles, id) {
        items.extend(collect_node(
            doc,
            styles,
            measure,
            images,
            child,
            style,
            available,
            href.clone(),
            view,
        ));
    }
    items
}

fn collect_node(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    parent_style: &ComputedStyle,
    available: f32,
    href: Option<String>,
    view: View,
) -> Vec<Item> {
    match &doc.node(id).data {
        NodeData::Text(text) => items_from_text(text, parent_style, measure, href, id),
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
            // An inline image is a replaced box. Walking its (empty) children
            // would drop it, so the line beside an infobox paints through it.
            let replaced = matches!(
                doc.element_name(id),
                Some("img" | "video" | "canvas" | "iframe" | "svg" | "input" | "textarea" | "select")
            );
            if matches!(
                display,
                Display::Block
                    | Display::ListItem
                    | Display::InlineBlock
                    | Display::Flex
                    | Display::InlineFlex
                    | Display::Grid
            ) || replaced
                || synthetic_text(doc, id).is_some()
            {
                let Some(layout) = layout_node(
                    doc, styles, measure, images, id, 0.0, 0.0, available, href, false, view,
                ) else {
                    return Vec::new();
                };
                let width = layout.margin.left + layout.rect.width + layout.margin.right;
                let height = layout.margin.top + layout.rect.height + layout.margin.bottom;
                return vec![Item::Atomic {
                    layout,
                    width,
                    height,
                }];
            }
            collect_children(
                doc, styles, measure, images, id, &style, available, href, view,
            )
        }
    }
}

fn wrap_width(style: &ComputedStyle, content_w: f32) -> f32 {
    if style.white_space == WhiteSpace::Nowrap {
        f32::MAX / 4.0
    } else {
        content_w
    }
}

fn place_items(
    items: &[Item],
    content_x: f32,
    y: f32,
    content_w: f32,
    align: TextAlign,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
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
                        family: word.family.clone(),
                        underline: word.underline,
                        href: word.href.clone(),
                        width: word.width,
                        node: word.node,
                    });
                    x += word.width;
                }
                Item::Atomic {
                    layout,
                    width: atom,
                    ..
                } => {
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

fn apply_transform(text: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::new();
            let mut boundary = true;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    boundary = true;
                    out.push(ch);
                } else if boundary {
                    out.extend(ch.to_uppercase());
                    boundary = false;
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

fn items_from_text(
    text: &str,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    node: NodeId,
) -> Vec<Item> {
    let pre = style.white_space == WhiteSpace::Pre;
    let normalized = if pre {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        collapse_space(text)
    };
    let normalized = apply_transform(&normalized, style.text_transform);
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
            push_runs(
                &mut items,
                line,
                style,
                measure,
                href.clone(),
                node,
                ascent,
                line_height,
                true,
            );
        }
    } else {
        push_runs(
            &mut items,
            &normalized,
            style,
            measure,
            href,
            node,
            ascent,
            line_height,
            false,
        );
    }
    items
}

fn push_runs(
    items: &mut Vec<Item>,
    text: &str,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    node: NodeId,
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
            let width = measure.width_family(
                &text,
                style.font_size,
                style.bold,
                style.mono,
                &style.font_family,
            );
            items.push(Item::Space { width, sticky });
        } else {
            let width = measure.width_family(
                &text,
                style.font_size,
                style.bold,
                style.mono,
                &style.font_family,
            );
            items.push(Item::Word(Word {
                text,
                color: style.color,
                size: style.font_size,
                bold: style.bold,
                mono: style.mono,
                family: style.font_family.clone(),
                underline: style.underline,
                href: href.clone(),
                width,
                ascent,
                line_height,
                node,
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
    family: String,
    underline: bool,
    href: Option<String>,
    width: f32,
    ascent: f32,
    line_height: f32,
    node: NodeId,
}

enum Item {
    Word(Word),
    Space {
        width: f32,
        sticky: bool,
    },
    Break,
    Atomic {
        layout: LayoutBox,
        width: f32,
        height: f32,
    },
}

fn is_blank(items: &[Item]) -> bool {
    items
        .iter()
        .all(|item| matches!(item, Item::Space { .. } | Item::Break))
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
    styles
        .get(id.0)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial)
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
    for child in box_children(doc, styles, id) {
        if doc.element_name(child).is_none() {
            continue;
        }
        let display = style_of(styles, child).display;
        if display == Display::None {
            continue;
        }
        if matches!(
            display,
            Display::Block | Display::ListItem | Display::Flex | Display::Grid
        ) || contains_block(doc, styles, child)
        {
            return true;
        }
    }
    false
}

fn has_block_child(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    box_children(doc, styles, id)
        .iter()
        .any(|child| is_block_child(doc, styles, *child))
}

/// Children that generate boxes. `display: contents` elements are replaced by
/// their own children, matching the element they would have been laid out in.
fn box_children(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    append_box_children(doc, styles, id, &mut out);
    out
}

fn append_box_children(
    doc: &Document,
    styles: &[ComputedStyle],
    id: NodeId,
    out: &mut Vec<NodeId>,
) {
    for child in doc.node(id).children.clone() {
        if doc.element_name(child).is_some() && style_of(styles, child).display == Display::Contents
        {
            append_box_children(doc, styles, child, out);
        } else {
            out.push(child);
        }
    }
}

fn is_block_child(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    if is_absolute(doc, styles, id) {
        return false;
    }
    doc.element_name(id).is_some_and(|_| {
        matches!(
            effective_display(doc, styles, id),
            Display::Block | Display::ListItem | Display::Flex | Display::Grid
        )
    })
}

fn is_absolute(doc: &Document, styles: &[ComputedStyle], id: NodeId) -> bool {
    doc.element_name(id).is_some()
        && matches!(
            style_of(styles, id).position,
            Position::Absolute | Position::Fixed
        )
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
        doc.attr(id, "href")
            .filter(|href| !href.is_empty())
            .map(str::to_string)
    } else {
        None
    }
}

fn attribute_px(doc: &Document, id: NodeId, name: &str) -> Option<f32> {
    let text = doc.attr(id, name)?;
    let number: String = text
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let value = number.parse::<f32>().ok()?;
    (value > 0.0).then_some(value)
}

/// Max-content width of a shrink-to-fit box, capped by the available line.
/// Block children contribute their own max-content instead of stretching.
fn max_content_width(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    id: NodeId,
    cap: f32,
) -> f32 {
    let style = style_of(styles, id);
    let mut line = 0.0f32;
    let mut widest = 0.0f32;
    let break_line = |line: &mut f32, widest: &mut f32| {
        *widest = widest.max(*line);
        *line = 0.0;
    };
    for child in doc.node(id).children.clone() {
        match &doc.node(child).data {
            NodeData::Text(text) => {
                for word in text.split_whitespace() {
                    let width = measure.width_family(
                        word,
                        style.font_size,
                        style.bold,
                        style.mono,
                        &style.font_family,
                    );
                    let space = if line > 0.0 {
                        measure.width(" ", style.font_size, style.bold, style.mono)
                    } else {
                        0.0
                    };
                    if line > 0.0 && line + space + width > cap {
                        break_line(&mut line, &mut widest);
                    }
                    line += space + width;
                    widest = widest.max(line);
                }
            }
            NodeData::Element(_) => {
                if is_skipped(doc, styles, child) {
                    continue;
                }
                let child_style = style_of(styles, child);
                let specified = resolve_length(
                    child_style.width,
                    child_style.font_size,
                    cap,
                    View {
                        width: cap,
                        height: cap,
                        root: style.font_size,
                    },
                );
                let child_width = if let Some(width) = specified {
                    width
                } else if doc.element_name(child) == Some("img") {
                    attribute_px(doc, child, "width").unwrap_or(300.0)
                } else {
                    max_content_width(doc, styles, measure, child, cap)
                };
                let block = matches!(
                    child_style.display,
                    Display::Block | Display::Flex | Display::Grid | Display::ListItem
                );
                if block {
                    break_line(&mut line, &mut widest);
                    widest = widest.max(child_width.min(cap));
                } else {
                    if line > 0.0 && line + child_width > cap {
                        break_line(&mut line, &mut widest);
                    }
                    line += child_width;
                    widest = widest.max(line);
                }
            }
            _ => {}
        }
    }
    widest.max(line).min(cap).max(0.0)
}

fn synthetic_text(doc: &Document, id: NodeId) -> Option<String> {
    let name = doc.element_name(id)?;
    match name {
        "input" => {
            let kind = doc.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
            if matches!(kind.as_str(), "checkbox" | "radio") {
                let checked = doc.attr(id, "checked").is_some();
                return Some(if checked {
                    "✓".to_string()
                } else {
                    String::new()
                });
            }
            if matches!(kind.as_str(), "submit" | "button" | "reset") {
                let fallback = if kind == "submit" { "Submit" } else { "Button" };
                return Some(doc.attr(id, "value").unwrap_or(fallback).to_string());
            }
            let value = doc
                .attr(id, "value")
                .or_else(|| doc.attr(id, "placeholder"))
                .unwrap_or("")
                .to_string();
            if kind == "password" {
                return Some("•".repeat(value.chars().count()));
            }
            Some(value)
        }
        "img" | "iframe" | "video" | "canvas" | "svg" | "audio" => {
            Some(doc.attr(id, "alt").unwrap_or("").to_string())
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

fn resolve_edges(sides: &[Length; 4], font: f32, containing: f32, view: View) -> Edges {
    let side = |length: Length| {
        resolve_length(length, font, containing, view)
            .unwrap_or(0.0)
            .max(0.0)
    };
    Edges::new(
        side(sides[0]),
        side(sides[1]),
        side(sides[2]),
        side(sides[3]),
    )
}

fn resolve_length(length: Length, font: f32, containing: f32, view: View) -> Option<f32> {
    match length {
        Length::Auto => None,
        Length::Px(px) => Some(px),
        Length::Percent(percent) => Some(containing * percent / 100.0),
        Length::Em(em) => Some(font * em),
        Length::Rem(rem) => Some(view.root * rem),
        Length::Vw(vw) => Some(view.width * vw / 100.0),
        Length::Vh(vh) => Some(view.height * vh / 100.0),
        Length::Calc {
            percent,
            px,
            em,
            rem,
            vw,
            vh,
        } => Some(
            containing * percent / 100.0
                + px
                + font * em
                + view.root * rem
                + view.width * vw / 100.0
                + view.height * vh / 100.0,
        ),
    }
}

fn resolve_height(length: Length, font: f32, view: View) -> Option<f32> {
    match length {
        Length::Auto | Length::Percent(_) => None,
        Length::Calc { percent, .. } if percent != 0.0 => None,
        other => resolve_length(other, font, 0.0, view).map(|value| value.max(0.0)),
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
    if let Some(body) = doc
        .node(html)
        .children
        .iter()
        .copied()
        .find(|id| doc.element_name(*id) == Some("body"))
    {
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
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    href: Option<String>,
    view: View,
) -> (Vec<LineBox>, Vec<LayoutBox>, f32) {
    let row = style.flex_direction == FlexDirection::Row;
    let mut items = Vec::new();
    if let Some(before) = generated_block(
        id,
        style.before.as_ref(),
        style,
        measure,
        href.clone(),
        0.0,
        0.0,
        content_w,
    ) {
        items.push((before, 0.0, 0.0));
    }
    for child in box_children(doc, styles, id) {
        if let NodeData::Text(text) = &doc.node(child).data {
            if text.trim().is_empty() {
                continue;
            }
            items.push((
                text_flex_item(child, text, style, measure, href.clone(), content_w),
                0.0,
                1.0,
            ));
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
        // nowrap labels (buttons) are measured at max-content. The flex line
        // may shrink them afterwards, but not below that label.
        let available = if shrink && child_style.white_space == WhiteSpace::Nowrap {
            content_w.max(100_000.0)
        } else {
            content_w
        };
        if let Some(mut layout) = layout_node(
            doc,
            styles,
            measure,
            images,
            child,
            0.0,
            0.0,
            available,
            href.clone(),
            shrink,
            view,
        ) {
            let child_style = style_of(styles, child);
            if row
                && !matches!(child_style.flex_basis, Length::Auto)
                && let Some(basis) = resolve_length(
                    child_style.flex_basis,
                    child_style.font_size,
                    content_w,
                    view,
                )
            {
                layout.rect.width = basis.max(0.0);
            }
            items.push((
                layout,
                child_style.flex_grow.max(0.0),
                child_style.flex_shrink.max(0.0),
            ));
        }
    }
    if let Some(after) = generated_block(
        id,
        style.after.as_ref(),
        style,
        measure,
        href.clone(),
        0.0,
        0.0,
        content_w,
    ) {
        items.push((after, 0.0, 0.0));
    }
    if style.flex_reverse {
        items.reverse();
    }
    if items.is_empty() {
        return (Vec::new(), Vec::new(), 0.0);
    }
    let main_gap = (if row { style.gap } else { style.row_gap }).max(0.0);
    let cross_gap = (if row { style.row_gap } else { style.gap }).max(0.0);
    let mut lines = if style.flex_wrap && row {
        let mut lines = Vec::new();
        let mut line = Vec::new();
        let mut used = 0.0;
        for item in items {
            let width = outer_main(&item.0, true);
            let next = if line.is_empty() {
                width
            } else {
                used + main_gap + width
            };
            if !line.is_empty() && next > content_w && content_w > 0.0 {
                lines.push(std::mem::take(&mut line));
                used = 0.0;
            }
            used = if line.is_empty() {
                width
            } else {
                used + main_gap + width
            };
            line.push(item);
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    } else {
        vec![items]
    };
    if style.flex_wrap_reverse {
        lines.reverse();
    }
    let mut placed = Vec::new();
    let mut cursor_cross = if row { content_y } else { content_x };
    for line in lines {
        let (boxes, cross) = place_flex_line(
            styles,
            style,
            line,
            content_x,
            content_y,
            content_w,
            row,
            main_gap,
            cursor_cross,
        );
        cursor_cross += cross + cross_gap;
        placed.extend(boxes);
    }
    let height = if placed.is_empty() {
        0.0
    } else if row {
        (cursor_cross - content_y - cross_gap).max(0.0)
    } else {
        (cursor_cross - content_x - cross_gap).max(0.0)
    };
    (Vec::new(), placed, height)
}

fn outer_main(layout: &LayoutBox, row: bool) -> f32 {
    if row {
        layout.margin.left + layout.rect.width + layout.margin.right
    } else {
        layout.margin.top + layout.rect.height + layout.margin.bottom
    }
}

fn place_flex_line(
    styles: &[ComputedStyle],
    style: &ComputedStyle,
    items: Vec<(LayoutBox, f32, f32)>,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    row: bool,
    gap: f32,
    cross_origin: f32,
) -> (Vec<LayoutBox>, f32) {
    let count = items.len();
    let outer = |layout: &LayoutBox, main: bool| -> f32 {
        if main == row {
            layout.margin.left + layout.rect.width + layout.margin.right
        } else {
            layout.margin.top + layout.rect.height + layout.margin.bottom
        }
    };
    let bases: Vec<f32> = items
        .iter()
        .map(|(layout, _, _)| outer(layout, true))
        .collect();
    let gap_total = gap * (count.saturating_sub(1)) as f32;
    let used: f32 = bases.iter().sum::<f32>() + gap_total;
    let free = if row { content_w - used } else { 0.0 };
    let grow_sum: f32 = items.iter().map(|(_, grow, _)| *grow).sum();
    let shrink_weight: f32 = items
        .iter()
        .zip(bases.iter())
        .map(|((_, _, shrink), base)| shrink.max(0.0) * base)
        .sum();
    let mut extra = vec![0.0; count];
    if row && free > 0.0 && grow_sum > 0.0 {
        for (index, (_, grow, _)) in items.iter().enumerate() {
            extra[index] = free * grow / grow_sum;
        }
    } else if row && free < 0.0 && shrink_weight > 0.0 {
        for (index, ((layout, _, shrink), base)) in items.iter().zip(bases.iter()).enumerate() {
            let proposed = free * shrink.max(0.0) * base / shrink_weight;
            // Automatic minimum size: a flex item does not shrink below its
            // min-content contribution (CSS flexbox §4.5).
            let floor = content_floor(layout) + layout.margin.left + layout.margin.right;
            let limit = (floor - base).min(0.0);
            extra[index] = proposed.max(limit).max(-base);
        }
    }
    let leftover = if row && free > 0.0 && grow_sum == 0.0 {
        free
    } else {
        0.0
    };
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
    let cross: Vec<f32> = items
        .iter()
        .map(|(layout, _, _)| outer(layout, false))
        .collect();
    let line = cross.iter().copied().fold(0.0_f32, f32::max);
    let mut cursor = if row {
        content_x + start
    } else {
        content_y + start
    };
    let mut placed = Vec::new();
    for (index, (mut layout, _, _)) in items.into_iter().enumerate() {
        if row && extra[index] != 0.0 {
            layout.rect.width = (layout.rect.width + extra[index]).max(0.0);
        }
        let main = bases[index] + extra[index];
        let cross_size = cross[index];
        let align = style_of(styles, layout.node)
            .align_self
            .unwrap_or(style.align_items);
        let shift = match align {
            AlignItems::End => (line - cross_size).max(0.0),
            AlignItems::Center => (line - cross_size).max(0.0) / 2.0,
            AlignItems::Stretch | AlignItems::Start => 0.0,
        };
        if align == AlignItems::Stretch
            && row
            && matches!(style_of(styles, layout.node).height, Length::Auto)
        {
            layout.rect.height =
                (line - layout.margin.top - layout.margin.bottom).max(layout.rect.height);
        }
        if align == AlignItems::Stretch
            && !row
            && matches!(style_of(styles, layout.node).width, Length::Auto)
        {
            layout.rect.width =
                (content_w - layout.margin.left - layout.margin.right).max(layout.rect.width);
        }
        let (x, y) = if row {
            (cursor, cross_origin + shift)
        } else {
            (cross_origin + shift, cursor)
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
    (placed, height)
}

fn text_flex_item(
    node: NodeId,
    text: &str,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    content_w: f32,
) -> LayoutBox {
    let items = items_from_text(text, style, measure, href.clone(), node);
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
        gradient: None,
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
        image: None,
        visible: true,
        z_index: 0,
        borders: [0.0; 4],
        fixed: false,
        shapes: Vec::new(),
        sticky: false,
        sticky_top: None,
        sticky_bottom: None,
        shadow: None,
        frame: None,
    }
}

fn layout_absolute_children(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    content_h: f32,
    href: Option<String>,
    view: View,
) -> Vec<LayoutBox> {
    let mut placed = Vec::new();
    for child in box_children(doc, styles, id) {
        if !is_absolute(doc, styles, child) {
            continue;
        }
        let style = style_of(styles, child);
        let fixed = style.position == Position::Fixed;
        let (box_w, box_h) = if fixed {
            (view.width, view.height)
        } else {
            (content_w, content_h)
        };
        let (inset_x, inset_y) = if fixed {
            (0.0, 0.0)
        } else {
            (content_x, content_y)
        };
        let Some(mut layout) = layout_node(
            doc,
            styles,
            measure,
            images,
            child,
            content_x,
            content_y,
            box_w,
            href.clone(),
            false,
            view,
        ) else {
            continue;
        };
        let style = style_of(styles, child);
        let font = style.font_size;
        let dx = if let Some(left) = resolve_length(style.insets[3], font, box_w, view) {
            inset_x + left - layout.rect.x
        } else if let Some(right) = resolve_length(style.insets[1], font, box_w, view) {
            inset_x + box_w - layout.rect.width - right - layout.rect.x
        } else {
            0.0
        };
        let dy = if let Some(top) = resolve_length(style.insets[0], font, box_h, view) {
            inset_y + top - layout.rect.y
        } else if let Some(bottom) = resolve_length(style.insets[2], font, box_h, view) {
            inset_y + box_h - layout.rect.height - bottom - layout.rect.y
        } else {
            0.0
        };
        translate(&mut layout, dx, dy);
        placed.push(layout);
    }
    placed
}

fn surround_items(
    mut items: Vec<Item>,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    node: NodeId,
    content_w: f32,
) -> Vec<Item> {
    if let Some(before) = generated_items(
        node,
        style.before.as_ref(),
        style,
        measure,
        href.clone(),
        content_w,
    ) {
        let mut lead = before;
        lead.append(&mut items);
        items = lead;
    }
    if let Some(after) =
        generated_items(node, style.after.as_ref(), style, measure, href, content_w)
    {
        items.extend(after);
    }
    items
}

fn generated_items(
    node: NodeId,
    generated: Option<&Generated>,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    content_w: f32,
) -> Option<Vec<Item>> {
    let generated = generated?;
    if generated.width.is_some() || generated.height.is_some() || generated.background.a > 0 {
        let layout = generated_block(
            node,
            Some(generated),
            style,
            measure,
            href,
            0.0,
            0.0,
            content_w,
        )?;
        let width = layout.rect.width;
        let height = layout.rect.height;
        return Some(vec![Item::Atomic {
            layout,
            width,
            height,
        }]);
    }
    let styled = style_for_generated(style, generated);
    let items = items_from_text(&generated.text, &styled, measure, href, node);
    (!items.is_empty()).then_some(items)
}

fn generated_block(
    node: NodeId,
    generated: Option<&Generated>,
    style: &ComputedStyle,
    measure: &dyn MeasureText,
    href: Option<String>,
    x: f32,
    y: f32,
    content_w: f32,
) -> Option<LayoutBox> {
    let generated = generated?;
    if generated.text.is_empty()
        && generated.width.is_none()
        && generated.height.is_none()
        && generated.background.a == 0
    {
        return None;
    }
    let styled = style_for_generated(style, generated);
    let items = items_from_text(&generated.text, &styled, measure, href.clone(), node);
    let text_width = preferred_width(&items, content_w);
    let used = generated
        .width
        .unwrap_or(text_width.min(content_w))
        .max(0.0);
    let (lines, children, text_height) = if is_blank(&items) {
        (Vec::new(), Vec::new(), 0.0)
    } else {
        place_items(&items, x, y, used.max(1.0), styled.text_align)
    };
    let height = generated.height.unwrap_or(text_height).max(0.0);
    Some(LayoutBox {
        node,
        rect: Rect::new(x, y, used, height),
        margin: Edges::ZERO,
        background: generated.background,
        gradient: None,
        border_color: styled.color,
        border_width: 0.0,
        color: styled.color,
        font_size: styled.font_size,
        bold: styled.bold,
        mono: styled.mono,
        href,
        marker: None,
        children,
        lines,
        opacity: 1.0,
        overflow_hidden: false,
        border_radius: 0.0,
        image: None,
        visible: true,
        z_index: 0,
        borders: [0.0; 4],
        fixed: false,
        shapes: Vec::new(),
        sticky: false,
        sticky_top: None,
        sticky_bottom: None,
        shadow: None,
        frame: None,
    })
}

fn style_for_generated(style: &ComputedStyle, generated: &Generated) -> ComputedStyle {
    let mut styled = style.clone();
    styled.color = generated.color;
    styled.background = generated.background;
    styled.font_size = generated.font_size;
    styled.bold = generated.bold;
    styled.before = None;
    styled.after = None;
    styled
}

fn empty_box(node: NodeId) -> LayoutBox {
    LayoutBox {
        node,
        rect: Rect::default(),
        margin: Edges::ZERO,
        background: mithu_base::Color::TRANSPARENT,
        gradient: None,
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
        shapes: Vec::new(),
        sticky: false,
        sticky_top: None,
        sticky_bottom: None,
        shadow: None,
        frame: None,
    }
}

/// Grow a box so a nowrap label is inside the border, not painted past it.
fn expand_ink(box_: &mut LayoutBox) {
    let border_right = box_.borders[1].max(0.0);
    let mut edge = box_.rect.right();
    for line in &box_.lines {
        for fragment in &line.fragments {
            edge = edge.max(fragment.x + fragment.width + border_right);
        }
    }
    for child in &box_.children {
        edge = edge.max(child.rect.right());
    }
    if edge > box_.rect.right() + 0.5 && edge < box_.rect.x + 8000.0 {
        box_.rect.width += edge - box_.rect.right();
    }
}

fn content_floor(layout: &LayoutBox) -> f32 {
    let mut floor = 0.0_f32;
    for line in &layout.lines {
        let width = line.fragments.iter().map(|fragment| fragment.width).sum();
        floor = floor.max(width);
    }
    let mut across = 0.0_f32;
    for child in &layout.children {
        let child_floor = content_floor(child);
        floor = floor.max(child_floor);
        across += child_floor;
    }
    floor.max(across)
}

fn apply_intrinsic(style: &mut ComputedStyle, bitmap: &Bitmap) {
    let width = bitmap.width.max(1) as f32;
    let height = bitmap.height.max(1) as f32;
    match (style.width, style.height) {
        (Length::Auto, Length::Auto) => {
            style.width = Length::Px(width);
            style.height = Length::Px(height);
        }
        (Length::Auto, Length::Px(used_height)) => {
            style.width = Length::Px(width * used_height / height);
        }
        (Length::Px(used_width), Length::Auto) => {
            style.height = Length::Px(height * used_width / width);
        }
        _ => {}
    }
}

pub fn hit_test(tree: &LayoutTree, x: f32, y: f32) -> Option<NodeId> {
    hit_box(&tree.root, x, y)
}

fn hit_box(box_: &LayoutBox, x: f32, y: f32) -> Option<NodeId> {
    for child in box_.children.iter().rev() {
        if let Some(id) = hit_box(child, x, y) {
            return Some(id);
        }
    }
    for line in &box_.lines {
        for fragment in &line.fragments {
            let rect = Rect::new(
                fragment.x,
                line.baseline - line.ascent,
                fragment.width.max(1.0),
                line.height.max(1.0),
            );
            if rect.contains(x, y) {
                return Some(fragment.node);
            }
        }
    }
    if box_.rect.contains(x, y) {
        return Some(box_.node);
    }
    None
}

#[derive(Clone)]
struct TableCell {
    id: NodeId,
    column: usize,
    colspan: usize,
    rowspan: usize,
}

fn layout_table(
    doc: &Document,
    styles: &[ComputedStyle],
    measure: &dyn MeasureText,
    images: &HashMap<NodeId, Arc<Bitmap>>,
    id: NodeId,
    containing_x: f32,
    margin_y: f32,
    containing_width: f32,
    href: Option<String>,
    view: View,
) -> Option<LayoutBox> {
    let style = style_of(styles, id);
    let font = style.font_size;
    let borders = [
        style.borders[0].max(0.0),
        style.borders[1].max(0.0),
        style.borders[2].max(0.0),
        style.borders[3].max(0.0),
    ];
    let border_w = borders[1] + borders[3];
    let border_h = borders[0] + borders[2];
    let padding = resolve_edges(&style.padding, font, containing_width, view);
    let margin_top = resolve_length(style.margin[0], font, containing_width, view)
        .unwrap_or(0.0)
        .max(0.0);
    let margin_bottom = resolve_length(style.margin[2], font, containing_width, view)
        .unwrap_or(0.0)
        .max(0.0);
    let margin_left = resolve_length(style.margin[3], font, containing_width, view)
        .unwrap_or(0.0)
        .max(0.0);
    let available =
        (containing_width - margin_left - padding.left - padding.right - border_w).max(0.0);
    let specified = resolve_length(style.width, font, containing_width, view).map(|width| {
        if style.box_sizing == BoxSizing::BorderBox {
            (width - padding.left - padding.right - border_w).max(0.0)
        } else {
            width.max(0.0)
        }
    });
    let rows = table_rows(doc, id);
    let mut grid: Vec<Vec<TableCell>> = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut columns = 0usize;
    for (row_index, row) in rows.iter().enumerate() {
        if occupied.len() <= row_index {
            occupied.push(Vec::new());
        }
        let mut placed = Vec::new();
        let mut column = 0usize;
        for cell in row {
            while occupied[row_index].get(column) == Some(&true) {
                column += 1;
            }
            let colspan = doc
                .attr(*cell, "colspan")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1);
            let rowspan = doc
                .attr(*cell, "rowspan")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(1);
            for extra in 0..rowspan {
                let target = row_index + extra;
                if occupied.len() <= target {
                    occupied.resize_with(target + 1, Vec::new);
                }
                let row_occ = &mut occupied[target];
                let end = column + colspan;
                if row_occ.len() < end {
                    row_occ.resize(end, false);
                }
                for slot in column..end {
                    row_occ[slot] = true;
                }
            }
            placed.push(TableCell {
                id: *cell,
                column,
                colspan,
                rowspan,
            });
            column += colspan;
        }
        columns = columns.max(column);
        grid.push(placed);
    }
    let mut min_w = vec![0.0f32; columns];
    let mut max_w = vec![0.0f32; columns];
    for row in &grid {
        for cell in row {
            if cell.colspan == 0 {
                continue;
            }
            let cell_style = style_of(styles, cell.id);
            let text = doc.text_content(cell.id);
            let mut longest = 0.0f32;
            for word in text.split_whitespace() {
                longest = longest.max(measure.width_family(
                    word,
                    cell_style.font_size,
                    cell_style.bold,
                    cell_style.mono,
                    &cell_style.font_family,
                ));
            }
            let pad = resolve_edges(&cell_style.padding, cell_style.font_size, available, view);
            let cell_border = cell_style.borders[1].max(0.0) + cell_style.borders[3].max(0.0);
            let min = longest + pad.left + pad.right + cell_border;
            let max = layout_node(
                doc,
                styles,
                measure,
                images,
                cell.id,
                0.0,
                0.0,
                10_000.0,
                href.clone(),
                true,
                view,
            )
            .map(|layout| layout.rect.width)
            .unwrap_or(min)
            .max(min);
            if cell.colspan == 1 {
                min_w[cell.column] = min_w[cell.column].max(min);
                max_w[cell.column] = max_w[cell.column].max(max);
            } else {
                let share_min = min / cell.colspan as f32;
                let share_max = max / cell.colspan as f32;
                for offset in 0..cell.colspan {
                    let column = cell.column + offset;
                    if column < columns {
                        min_w[column] = min_w[column].max(share_min);
                        max_w[column] = max_w[column].max(share_max);
                    }
                }
            }
        }
    }
    let sum_min: f32 = min_w.iter().sum();
    let sum_max: f32 = max_w.iter().sum();
    let target = specified
        .unwrap_or(sum_max.min(available).max(sum_min))
        .max(sum_min);
    let mut widths = max_w.clone();
    if target >= sum_max && sum_max > 0.0 {
        let extra = target - sum_max;
        for (width, max) in widths.iter_mut().zip(max_w.iter()) {
            *width += extra * max / sum_max;
        }
    } else if sum_max > sum_min {
        let overflow = (sum_max - target).max(0.0);
        let room = sum_max - sum_min;
        for (width, (min, max)) in widths.iter_mut().zip(min_w.iter().zip(max_w.iter())) {
            *width = max - overflow * (max - min) / room;
        }
    }
    let mut children = Vec::new();
    let y = margin_y + margin_top + borders[0] + padding.top;
    let x0 = containing_x + margin_left + borders[3] + padding.left;
    let mut row_heights = vec![0.0f32; grid.len()];
    let mut laid: Vec<(usize, TableCell, LayoutBox)> = Vec::new();
    for (row_index, row) in grid.iter().enumerate() {
        for cell in row {
            if cell.colspan == 0 {
                continue;
            }
            let width: f32 = widths.iter().skip(cell.column).take(cell.colspan).sum();
            if let Some(layout) = layout_node(
                doc,
                styles,
                measure,
                images,
                cell.id,
                0.0,
                0.0,
                width,
                href.clone(),
                false,
                view,
            ) {
                let span = cell.rowspan.max(1);
                if span == 1 {
                    row_heights[row_index] = row_heights[row_index].max(layout.rect.height);
                }
                laid.push((
                    row_index,
                    TableCell {
                        id: cell.id,
                        column: cell.column,
                        colspan: cell.colspan,
                        rowspan: span,
                    },
                    layout,
                ));
            }
        }
    }
    for (row_index, cell, layout) in &laid {
        if cell.rowspan > 1 {
            let covered: f32 = row_heights.iter().skip(*row_index).take(cell.rowspan).sum();
            if layout.rect.height > covered {
                let extra = layout.rect.height - covered;
                let last = (*row_index + cell.rowspan - 1).min(row_heights.len() - 1);
                row_heights[last] += extra;
            }
        }
    }
    for (row_index, cell, mut layout) in laid {
        let x = x0 + widths.iter().take(cell.column).sum::<f32>();
        let y_cell = y + row_heights.iter().take(row_index).sum::<f32>();
        let dx = x - layout.rect.x;
        let dy = y_cell - layout.rect.y;
        translate(&mut layout, dx, dy);
        children.push(layout);
    }
    let content_h: f32 = row_heights.iter().sum();
    let content_w: f32 = widths.iter().sum();
    Some(LayoutBox {
        node: id,
        rect: Rect::new(
            containing_x + margin_left,
            margin_y + margin_top,
            content_w + padding.left + padding.right + border_w,
            content_h + padding.top + padding.bottom + border_h,
        ),
        margin: Edges::new(margin_top, 0.0, margin_bottom, margin_left),
        background: style.background,
        gradient: style.gradient,
        border_color: style.border_color,
        border_width: borders[0],
        borders,
        fixed: style.position == Position::Fixed,
        color: style.color,
        font_size: style.font_size,
        bold: style.bold,
        mono: style.mono,
        href,
        marker: None,
        children,
        lines: Vec::new(),
        opacity: style.opacity.clamp(0.0, 1.0),
        overflow_hidden: style.overflow_hidden,
        border_radius: style.border_radius,
        image: None,
        visible: style.visible,
        z_index: style.z_index,
        shapes: Vec::new(),
        sticky: style.position == Position::Sticky,
        sticky_top: resolve_length(style.insets[0], style.font_size, view.height, view),
        sticky_bottom: resolve_length(style.insets[2], style.font_size, view.height, view),
        shadow: style.shadow,
        frame: None,
    })
}

fn table_rows(doc: &Document, table: NodeId) -> Vec<Vec<NodeId>> {
    let mut rows = Vec::new();
    fn walk(doc: &Document, id: NodeId, rows: &mut Vec<Vec<NodeId>>) {
        for child in doc.node(id).children.clone() {
            match doc.element_name(child) {
                Some("tr") => {
                    let cells = doc
                        .node(child)
                        .children
                        .iter()
                        .copied()
                        .filter(|cell| matches!(doc.element_name(*cell), Some("td" | "th")))
                        .collect();
                    rows.push(cells);
                }
                Some("thead" | "tbody" | "tfoot") => walk(doc, child, rows),
                _ => {}
            }
        }
    }
    walk(doc, table, &mut rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use mithu_html::parse_html;
    use mithu_style::style_document;
    use mithu_text::NaiveMetrics;
    use std::sync::Arc;

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
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let id = doc.get_element_by_id("box").unwrap();
        let found = find(&tree.root, id).unwrap();
        assert!((found.rect.x - 8.0).abs() < 0.5, "x {}", found.rect.x);
        assert!((found.rect.y - 8.0).abs() < 0.5, "y {}", found.rect.y);
        assert!(
            (found.rect.width - 100.0).abs() < 0.5,
            "w {}",
            found.rect.width
        );
        assert!(
            (found.rect.height - 50.0).abs() < 0.5,
            "h {}",
            found.rect.height
        );
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
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let row = find(&tree.root, doc.get_element_by_id("row").unwrap()).unwrap();
        let a = find(row, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(row, doc.get_element_by_id("b").unwrap()).unwrap();
        assert!(
            (b.rect.x - (a.rect.x + 40.0 + 10.0)).abs() < 0.5,
            "gap {}",
            b.rect.x - a.rect.x
        );
        let host = find(&tree.root, doc.get_element_by_id("host").unwrap()).unwrap();
        let pin = find(host, doc.get_element_by_id("pin").unwrap()).unwrap();
        assert!(
            (pin.rect.x - (host.rect.x + 30.0)).abs() < 0.5,
            "x {}",
            pin.rect.x
        );
        assert!(
            (pin.rect.y - (host.rect.y + 12.0)).abs() < 0.5,
            "y {}",
            pin.rect.y
        );
    }

    #[test]
    fn image_uses_its_pixel_size() {
        let doc = parse_html(r#"<img id="pic" src="pixel.png" style="border-width:0">"#);
        let styles = style_document(&doc, "");
        let id = doc.get_element_by_id("pic").unwrap();
        let bitmap = Arc::new(mithu_image::Bitmap {
            width: 20,
            height: 10,
            pixels: vec![0xFFFF0000; 200],
        });
        let mut images = HashMap::new();
        images.insert(id, bitmap);
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &images);
        let found = find(&tree.root, id).unwrap();
        assert!(
            (found.rect.width - 20.0).abs() < 0.5,
            "w {}",
            found.rect.width
        );
        assert!(
            (found.rect.height - 10.0).abs() < 0.5,
            "h {}",
            found.rect.height
        );
        assert!(found.image.is_some());
    }

    #[test]
    fn flex_wraps_and_viewport_units_resolve() {
        let doc = parse_html(
            r#"<div id="row" style="display:flex;flex-wrap:wrap;width:100px;gap:0">
                <div id="a" style="width:60px;height:10px"></div>
                <div id="b" style="width:60px;height:10px"></div>
            </div>
            <div id="wide" style="width:50vw;height:10vh"></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 200.0, 100.0, &NaiveMetrics, &HashMap::new());
        let row = find(&tree.root, doc.get_element_by_id("row").unwrap()).unwrap();
        let a = find(row, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(row, doc.get_element_by_id("b").unwrap()).unwrap();
        assert!(
            b.rect.y > a.rect.y,
            "wrapped y {} then {}",
            a.rect.y,
            b.rect.y
        );
        assert!((row.rect.width - 100.0).abs() < 1.0);
        let wide = find(&tree.root, doc.get_element_by_id("wide").unwrap()).unwrap();
        assert!(
            (wide.rect.width - 100.0).abs() < 1.0,
            "vw {}",
            wide.rect.width
        );
        assert!(
            (wide.rect.height - 10.0).abs() < 1.0,
            "vh {}",
            wide.rect.height
        );
    }

    #[test]
    fn uppercase_changes_the_painted_text() {
        let doc = parse_html(r#"<p id="p" style="text-transform:uppercase">ab cd</p>"#);
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let p = find(&tree.root, doc.get_element_by_id("p").unwrap()).unwrap();
        let text: String = p
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter().map(|fragment| fragment.text.clone()))
            .collect();
        assert_eq!(text, "ABCD");
    }

    fn all_text(box_: &LayoutBox) -> String {
        let mut text: String = box_
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter().map(|fragment| fragment.text.clone()))
            .collect();
        for child in &box_.children {
            text.push_str(&all_text(child));
        }
        text
    }

    #[test]
    fn generated_content_is_laid_out() {
        let doc = parse_html(
            r#"<style>div::before { content: "X" } div::after { content: "Y" }</style><div id="box">Z</div>"#,
        );
        let styles = style_document(&doc, &doc.stylesheets.join("\n"));
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let host = find(&tree.root, doc.get_element_by_id("box").unwrap()).unwrap();
        assert_eq!(all_text(host), "XZY");
    }

    #[test]
    fn fixed_box_uses_the_viewport() {
        let doc = parse_html(
            r#"<div style="height:2000px"></div><div id="pin" style="position:fixed;top:0;left:0;width:10px;height:10px"></div><div id="held" style="position:fixed;top:50%;width:10px;height:10px"></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 300.0, &NaiveMetrics, &HashMap::new());
        let pin = find(&tree.root, doc.get_element_by_id("pin").unwrap()).unwrap();
        assert!(pin.fixed);
        assert!((pin.rect.y - 0.0).abs() < 0.5, "y {}", pin.rect.y);
        assert!((pin.rect.x - 0.0).abs() < 0.5, "x {}", pin.rect.x);
        let held = find(&tree.root, doc.get_element_by_id("held").unwrap()).unwrap();
        assert!(
            (held.rect.y - 150.0).abs() < 0.5,
            "percent y {}",
            held.rect.y
        );
        assert!((held.rect.x - 8.0).abs() < 0.5, "static x {}", held.rect.x);
    }

    #[test]
    fn a_bottom_border_adds_only_that_side() {
        let doc = parse_html(
            r#"<div id="box" style="width:100px;height:50px;border-bottom:4px solid red"></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let host = find(&tree.root, doc.get_element_by_id("box").unwrap()).unwrap();
        assert!(
            (host.borders[2] - 4.0).abs() < 0.1,
            "bottom {}",
            host.borders[2]
        );
        assert!(
            host.borders[0].abs() < 0.1
                && host.borders[1].abs() < 0.1
                && host.borders[3].abs() < 0.1
        );
        assert!(
            (host.rect.height - 54.0).abs() < 0.5,
            "h {}",
            host.rect.height
        );
        assert!(
            (host.rect.width - 100.0).abs() < 0.5,
            "w {}",
            host.rect.width
        );
    }

    #[test]
    fn a_right_float_shrinks_to_its_content() {
        let doc = parse_html(
            r#"<div id="host">
              <div id="flag" style="float:right;display:flex"><span style="display:inline-block;width:20px;height:20px"></span></div>
              <div id="tag">From Wikipedia, the free encyclopedia</div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 800.0, 600.0, &NaiveMetrics, &HashMap::new());
        let flag = find(&tree.root, doc.get_element_by_id("flag").unwrap()).unwrap();
        let tag = find(&tree.root, doc.get_element_by_id("tag").unwrap()).unwrap();
        assert!(
            flag.rect.width < 80.0,
            "float stretched to {}",
            flag.rect.width
        );
        assert!(
            tag.rect.width > 200.0,
            "tagline squeezed to {}",
            tag.rect.width
        );
        assert!(flag.rect.x > tag.rect.x + 100.0, "float x {}", flag.rect.x);
    }

    #[test]
    fn an_svg_arc_and_use_paint_and_css_fill_is_kept() {
        let doc = parse_html(
            r##"<svg id="logo" width="80" height="24" viewBox="0 0 80 24">
              <defs><path id="mark" d="M2 20 L10 2 L18 20 Z"/></defs>
              <use href="#mark"></use>
              <path id="word" class="word" d="M30 16 a8 8 0 0 1 16 0"/>
            </svg>"##,
        );
        let styles = style_document(&doc, ".word { fill: #0a5dbd; }");
        let tree = layout_tree(&doc, &styles, 200.0, 80.0, &NaiveMetrics, &HashMap::new());
        let logo = find(&tree.root, doc.get_element_by_id("logo").unwrap()).unwrap();
        assert!(
            logo.shapes.len() >= 2,
            "expected the symbol and the arc, got {}",
            logo.shapes.len()
        );
        let word = logo
            .shapes
            .iter()
            .find(|shape| shape.points.len() > 4)
            .expect("the arc should flatten to several points");
        assert_eq!(word.fill, Some(mithu_base::Color::rgb(0x0a, 0x5d, 0xbd)));
    }

    #[test]
    fn a_left_float_narrows_the_next_block() {
        let doc = parse_html(
            r#"<div id="host" style="width:300px"><div id="side" style="float:left;width:80px;height:40px"></div><div id="main" style="height:40px"></div><div id="below" style="clear:both;height:10px"></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let side = find(&tree.root, doc.get_element_by_id("side").unwrap()).unwrap();
        let main = find(&tree.root, doc.get_element_by_id("main").unwrap()).unwrap();
        let below = find(&tree.root, doc.get_element_by_id("below").unwrap()).unwrap();
        assert!(
            (side.rect.width - 80.0).abs() < 0.5,
            "side {}",
            side.rect.width
        );
        assert!(
            (main.rect.x - (side.rect.right())).abs() < 1.0,
            "main x {} side right {}",
            main.rect.x,
            side.rect.right()
        );
        assert!(
            (main.rect.width - 220.0).abs() < 1.0,
            "main w {}",
            main.rect.width
        );
        assert!(
            below.rect.y + 0.5 >= side.rect.bottom(),
            "below {} side {}",
            below.rect.y,
            side.rect.bottom()
        );
    }

    #[test]
    fn grid_places_tracks_and_a_span() {
        let doc = parse_html(
            r#"<div id="grid" style="display:grid;width:300px;grid-template-columns:100px 1fr;gap:10px"><div id="a" style="height:20px"></div><div id="b" style="height:20px"></div><div id="c" style="grid-column:span 2;height:10px"></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let a = find(&tree.root, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(&tree.root, doc.get_element_by_id("b").unwrap()).unwrap();
        let c = find(&tree.root, doc.get_element_by_id("c").unwrap()).unwrap();
        assert!((a.rect.width - 100.0).abs() < 0.5, "a {}", a.rect.width);
        assert!((b.rect.width - 190.0).abs() < 0.5, "b {}", b.rect.width);
        assert!(
            (b.rect.x - (a.rect.right() + 10.0)).abs() < 1.0,
            "b x {}",
            b.rect.x
        );
        assert!((c.rect.width - 300.0).abs() < 1.0, "c {}", c.rect.width);
        assert!(c.rect.y > a.rect.y, "c y {} a y {}", c.rect.y, a.rect.y);
    }

    #[test]
    fn grid_column_span_is_wider_than_one_track() {
        let doc = parse_html(
            r#"<div id="grid" style="display:grid;width:240px;grid-template-columns:repeat(6, 1fr)"><div id="lead" style="grid-column:2 / span 3;height:10px"></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let lead = find(&tree.root, doc.get_element_by_id("lead").unwrap()).unwrap();
        assert!(
            (lead.rect.width - 120.0).abs() < 2.0,
            "lead {}",
            lead.rect.width
        );
        assert!(
            lead.rect.x > 20.0,
            "lead starts on the second track, x {}",
            lead.rect.x
        );
    }

    #[test]
    fn grid_row_shares_a_line_and_sticky_stays_in_flow() {
        let doc = parse_html(
            r#"<div id="grid" style="display:grid;width:200px;grid-template-columns:1fr 1fr">
              <div id="a" style="grid-column:1;grid-row:1;height:20px"></div>
              <div id="b" style="grid-column:2;grid-row:1;height:30px"></div>
              <div id="stick" style="position:sticky;top:0;height:10px">bar</div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let a = find(&tree.root, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(&tree.root, doc.get_element_by_id("b").unwrap()).unwrap();
        assert!(
            (a.rect.y - b.rect.y).abs() < 1.0,
            "same row a {} b {}",
            a.rect.y,
            b.rect.y
        );
        assert!(b.rect.x > a.rect.x, "b is to the right");
        let stick = find(&tree.root, doc.get_element_by_id("stick").unwrap()).unwrap();
        assert!(stick.sticky, "sticky is a flag, not an absolute box");
        assert!(
            stick.rect.y + 0.5 >= a.rect.bottom(),
            "sticky stays in flow"
        );
        assert!(stick.sticky_top.is_some());
        assert!(stick.sticky_bottom.is_none());
    }

    #[test]
    fn vector_header_areas_and_column_flow() {
        let doc = parse_html(
            r#"<header id="bar" style="display:grid;width:400px;grid-template:auto / 160px minmax(0,1fr);grid-template-areas:'headerStart headerEnd'">
              <div id="start" style="grid-area:headerStart;height:20px">logo</div>
              <div id="end" style="grid-area:headerEnd;height:20px">links</div>
            </header>
            <div id="flow" style="display:grid;width:400px;grid-template-rows:repeat(2, auto);grid-auto-flow:column;grid-auto-columns:100px">
              <div id="c1" style="height:10px"></div>
              <div id="c2" style="height:10px"></div>
              <div id="c3" style="height:10px"></div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let start = find(&tree.root, doc.get_element_by_id("start").unwrap()).unwrap();
        let end = find(&tree.root, doc.get_element_by_id("end").unwrap()).unwrap();
        assert!(
            (start.rect.y - end.rect.y).abs() < 1.0,
            "header row start {} end {}",
            start.rect.y,
            end.rect.y
        );
        assert!(
            end.rect.x > start.rect.right() - 1.0,
            "end sits to the right"
        );
        let first = find(&tree.root, doc.get_element_by_id("c1").unwrap()).unwrap();
        let third = find(&tree.root, doc.get_element_by_id("c3").unwrap()).unwrap();
        assert!(
            third.rect.x > first.rect.x + 50.0,
            "column flow wraps to the next column"
        );
    }

    #[test]
    fn rem_grid_track_keeps_a_content_column() {
        let doc = parse_html(
            r#"<div id="page" style="display:grid;width:1200px;grid-template-columns:minmax(0, 59.25rem) min-content">
              <div id="main" style="height:20px">article</div>
              <div id="side" style="width:40px;height:20px">end</div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 1200.0, 800.0, &NaiveMetrics, &HashMap::new());
        let main = find(&tree.root, doc.get_element_by_id("main").unwrap()).unwrap();
        let side = find(&tree.root, doc.get_element_by_id("side").unwrap()).unwrap();
        assert!(
            main.rect.width > 400.0,
            "rem column collapsed to {}",
            main.rect.width
        );
        assert!(side.rect.x > main.rect.x + 200.0, "side x {}", side.rect.x);
    }

    #[test]
    fn vector_toolbar_sits_above_a_wide_lead() {
        let doc = parse_html(
            r#"<div class="mw-page-container-inner">
              <div class="vector-column-start" id="toc">Contents</div>
              <div class="mw-content-container">
                <main class="mw-body">
                  <header class="vector-page-titlebar" id="title">
                    <h1>Rust</h1>
                    <div class="mw-portlet-lang vector-dropdown">
                      <span>60 languages</span>
                      <div class="vector-dropdown-content">English French German Spanish Italian Portuguese Dutch Swedish Norwegian Danish Finnish</div>
                    </div>
                  </header>
                  <div class="vector-page-toolbar" id="bar">Read Edit View history</div>
                  <div class="vector-column-end" id="end">Appearance</div>
                  <div id="bodyContent">
                    <div id="siteSub">From Wikipedia, the free encyclopedia</div>
                    <p id="lead">Rust is a general-purpose programming language.</p>
                  </div>
                </main>
              </div>
            </div>"#,
        );
        let styles = style_document(
            &doc,
            r#"
            .mw-page-container-inner { display:grid; column-gap:24px;
              grid-template: min-content 1fr min-content / 12.25rem minmax(0, 1fr);
              grid-template-areas: "siteNotice siteNotice" "columnStart pageContent" "footer footer"; }
            .vector-column-start { grid-area: columnStart; }
            .mw-content-container { grid-area: pageContent; }
            @media screen and (min-width: 1120px) {
              .mw-body { display:grid;
                grid-template: min-content min-content min-content 1fr / minmax(0, 59.25rem) min-content;
                grid-template-areas: "titlebar-cx ." "titlebar columnEnd" "toolbar columnEnd" "content columnEnd"; }
              .mw-body .vector-page-titlebar { grid-area: titlebar; }
              .mw-body .vector-page-toolbar { grid-area: toolbar; }
              .mw-body #bodyContent { grid-area: content; }
              .mw-body .vector-column-end { grid-area: columnEnd; }
            }
            .vector-dropdown .vector-dropdown-content { position:absolute; top:100%; height:0; overflow:hidden; }
            .mw-body .mw-portlet-lang { float:right; }
            #siteSub { display:none; }
            "#,
        );
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        let bar = find(&tree.root, doc.get_element_by_id("bar").unwrap()).unwrap();
        let lead = find(&tree.root, doc.get_element_by_id("lead").unwrap()).unwrap();
        let toc = find(&tree.root, doc.get_element_by_id("toc").unwrap()).unwrap();
        assert!(
            lead.rect.width > 400.0,
            "lead squeezed to {}",
            lead.rect.width
        );
        assert!(
            lead.rect.y + 1.0 >= bar.rect.bottom(),
            "lead {} overlaps toolbar bottom {}",
            lead.rect.y,
            bar.rect.bottom()
        );
        assert!(
            lead.rect.x > toc.rect.right() - 2.0,
            "lead x {} toc right {}",
            lead.rect.x,
            toc.rect.right()
        );
        assert!(
            (bar.rect.width - lead.rect.width).abs() < 80.0,
            "toolbar {} lead {}",
            bar.rect.width,
            lead.rect.width
        );
    }

    #[test]
    fn an_image_width_attribute_shrinks_its_right_float() {
        let doc = parse_html(
            r#"<div id="host">
              <div id="flag" style="float:right;display:flex">
                <div style="display:inline-block"><div><img alt="This is a good article" width="19" height="20"></div></div>
              </div>
              <div id="tag">From Wikipedia, the free encyclopedia</div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 800.0, 600.0, &NaiveMetrics, &HashMap::new());
        let flag = find(&tree.root, doc.get_element_by_id("flag").unwrap()).unwrap();
        let tag = find(&tree.root, doc.get_element_by_id("tag").unwrap()).unwrap();
        assert!(flag.rect.width < 80.0, "float stretched to {}", flag.rect.width);
        assert!(tag.rect.width > 400.0, "tagline squeezed to {}", tag.rect.width);
    }

    #[test]
    fn mdn_article_column_sits_between_the_sidebars() {
        let doc = parse_html(
            r#"<div class="layout">
              <nav id="left" class="layout__left-sidebar">Sidebar</nav>
              <div id="article" class="layout__body">Window</div>
              <aside id="right" class="layout__right-sidebar">In this article</aside>
            </div>"#,
        );
        let styles = style_document(
            &doc,
            r#"
            :root {
              --layout-sidebar-min: 15rem;
              --layout-sidebar-gap: 2rem;
              --layout-content-max: 48rem;
              --layout-2-sidebars: [full-start left-sidebar-start] minmax(var(--layout-sidebar-min), 1fr) [left-sidebar-end] var(--layout-sidebar-gap) [content-start] minmax(0, var(--layout-content-max)) [content-end] var(--layout-sidebar-gap) [right-sidebar-start] minmax(var(--layout-sidebar-min), 1fr) [full-end right-sidebar-end];
            }
            .layout { display: grid; grid-template-columns: var(--layout-2-sidebars);
              grid-template-areas: "left-sidebar . header . right-sidebar" "left-sidebar . body . right-sidebar"; }
            .layout__left-sidebar { grid-area: left-sidebar; }
            .layout__body { grid-area: body; }
            .layout__right-sidebar { grid-area: right-sidebar; }
            "#,
        );
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        let left = find(&tree.root, doc.get_element_by_id("left").unwrap()).unwrap();
        let article = find(&tree.root, doc.get_element_by_id("article").unwrap()).unwrap();
        let right = find(&tree.root, doc.get_element_by_id("right").unwrap()).unwrap();
        assert!(
            article.rect.x > left.rect.right() - 1.0,
            "article x {} left right {}",
            article.rect.x,
            left.rect.right()
        );
        assert!(
            right.rect.x > article.rect.right() - 1.0,
            "right x {} article right {}",
            right.rect.x,
            article.rect.right()
        );
        assert!(
            article.rect.width > 400.0,
            "article width {}",
            article.rect.width
        );
    }

    #[test]
    fn a_spanning_grid_item_does_not_stretch_the_first_row() {
        let doc = parse_html(
            r#"<div id="page" style="display:grid;width:400px;grid-template-columns:1fr 40px;grid-template-areas:'title side' 'tools side' 'body side'">
              <div id="title" style="grid-area:title;height:20px"></div>
              <div id="tools" style="grid-area:tools;height:16px"></div>
              <div id="side" style="grid-area:side;height:120px"></div>
              <div id="body" style="grid-area:body;height:30px"></div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let title = find(&tree.root, doc.get_element_by_id("title").unwrap()).unwrap();
        let tools = find(&tree.root, doc.get_element_by_id("tools").unwrap()).unwrap();
        let body = find(&tree.root, doc.get_element_by_id("body").unwrap()).unwrap();
        assert!(
            (tools.rect.y - (title.rect.bottom())).abs() < 2.0,
            "toolbar y {} title bottom {}",
            tools.rect.y,
            title.rect.bottom()
        );
        assert!(
            body.rect.y > tools.rect.bottom() - 2.0 && body.rect.y < tools.rect.bottom() + 8.0,
            "article y {} toolbar bottom {}",
            body.rect.y,
            tools.rect.bottom()
        );
    }

    #[test]
    fn display_contents_keeps_the_article_beside_the_sidebar() {
        let doc = parse_html(
            r#"<div id="grid" style="display:grid;width:400px;grid-template-columns:120px 1fr;grid-template-areas:'side title' 'side body'">
              <div id="side" style="grid-area:side;height:200px"></div>
              <div style="display:contents">
                <h1 id="title" style="grid-area:title;height:24px">Window</h1>
                <div id="body" style="grid-area:body;height:40px">Article</div>
              </div>
            </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let title = find(&tree.root, doc.get_element_by_id("title").unwrap()).unwrap();
        let body = find(&tree.root, doc.get_element_by_id("body").unwrap()).unwrap();
        let side = find(&tree.root, doc.get_element_by_id("side").unwrap()).unwrap();
        assert!(
            title.rect.x > side.rect.right() - 2.0,
            "title x {}",
            title.rect.x
        );
        assert!(
            body.rect.y < title.rect.bottom() + 40.0 && body.rect.y < side.rect.height,
            "article dropped below the sidebar: title {} body {}",
            title.rect.bottom(),
            body.rect.y
        );
    }

    #[test]
    fn specified_height_does_not_grow_with_overflow() {
        let doc = parse_html(
            r#"<div id="host"><div id="menu" style="height:0;overflow:hidden"><div style="height:200px"></div></div><div id="next" style="height:10px"></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let next = find(&tree.root, doc.get_element_by_id("next").unwrap()).unwrap();
        assert!(next.rect.y < 20.0, "next y {}", next.rect.y);
    }

    #[test]
    fn nowrap_label_grows_the_border_box() {
        let doc = parse_html(
            r#"<a id="go" style="display:inline-block;white-space:nowrap;border:1px solid #00f">View services</a>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 40.0, 200.0, &NaiveMetrics, &HashMap::new());
        let go = find(&tree.root, doc.get_element_by_id("go").unwrap()).unwrap();
        assert!(
            go.rect.width > 80.0,
            "border follows the label, width {}",
            go.rect.width
        );
    }

    #[test]
    fn an_inline_image_lifts_the_line() {
        let doc = parse_html(
            r#"<p id="p">Hi <img id="pic" width="40" height="80" alt=""> tail</p>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 200.0, &NaiveMetrics, &HashMap::new());
        let pic = find(&tree.root, doc.get_element_by_id("pic").unwrap()).unwrap();
        let paragraph = find(&tree.root, doc.get_element_by_id("p").unwrap()).unwrap();
        assert!(
            pic.rect.height >= 80.0 && paragraph.rect.height >= 80.0,
            "image {} paragraph {}",
            pic.rect.height,
            paragraph.rect.height
        );
    }

    #[test]
    fn flex_items_shrink_to_the_line() {
        let doc = parse_html(
            r#"<div id="row" style="display:flex;width:100px"><div id="a" style="width:80px;height:10px"></div><div id="b" style="width:80px;height:10px"></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let a = find(&tree.root, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(&tree.root, doc.get_element_by_id("b").unwrap()).unwrap();
        assert!(a.rect.width < 79.0, "a {}", a.rect.width);
        assert!(
            (a.rect.width + b.rect.width - 100.0).abs() < 1.0,
            "{} {}",
            a.rect.width,
            b.rect.width
        );
    }

    #[test]
    fn a_flex_button_keeps_its_label() {
        let doc = parse_html(
            r#"<div id="row" style="display:flex;width:40px"><button id="go">View project</button></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let go = find(&tree.root, doc.get_element_by_id("go").unwrap()).unwrap();
        assert!(go.rect.width > 80.0, "button width {}", go.rect.width);
    }

    #[test]
    fn inline_svg_builds_a_path() {
        let doc = parse_html(
            r##"<svg id="icon" viewBox="0 0 10 10" width="20" height="20"><path d="M0 0 L10 0 L10 10 Z" fill="#ff0000"/></svg>"##,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let icon = find(&tree.root, doc.get_element_by_id("icon").unwrap()).unwrap();
        assert!(!icon.shapes.is_empty());
        assert_eq!(icon.background.a, 0);
        assert!(icon.shapes[0].fill.is_some());
        assert!(icon.rect.width >= 20.0, "w {}", icon.rect.width);
    }

    #[test]
    fn table_cells_share_rows_and_columns() {
        let doc = parse_html(
            r#"<table id="t"><tr><th id="a">Name</th><th id="b">Value</th></tr><tr><td id="c">One</td><td id="d">Two</td></tr></table>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 600.0, &NaiveMetrics, &HashMap::new());
        let a = find(&tree.root, doc.get_element_by_id("a").unwrap()).unwrap();
        let b = find(&tree.root, doc.get_element_by_id("b").unwrap()).unwrap();
        let c = find(&tree.root, doc.get_element_by_id("c").unwrap()).unwrap();
        assert!(
            b.rect.x + 0.5 >= a.rect.right(),
            "b {} a {}",
            b.rect.x,
            a.rect.right()
        );
        assert!(
            (c.rect.x - a.rect.x).abs() < 1.0,
            "c x {} a x {}",
            c.rect.x,
            a.rect.x
        );
        assert!(c.rect.y > a.rect.y, "c y {} a y {}", c.rect.y, a.rect.y);
    }

    #[test]
    fn wpt_css_subset_reports_a_pass_rate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/css");
        let mut files: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
            .collect();
        files.sort();
        assert!(!files.is_empty());
        let mut passed = 0usize;
        let mut notes = Vec::new();
        for path in &files {
            let html = std::fs::read_to_string(path).unwrap();
            let doc = parse_html(&html);
            let css = doc.stylesheets.join("\n");
            let styles = style_document(&doc, &css);
            let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
            let name = path.file_stem().unwrap().to_string_lossy();
            let ok = match name.as_ref() {
                "float-beside-tagline" => {
                    let flag = find(&tree.root, doc.get_element_by_id("flag").unwrap()).unwrap();
                    let tag = find(&tree.root, doc.get_element_by_id("tag").unwrap()).unwrap();
                    flag.rect.width < 80.0 && tag.rect.width > 400.0
                }
                "two-sidebars" => {
                    let left = find(&tree.root, doc.get_element_by_id("left").unwrap()).unwrap();
                    let article = find(&tree.root, doc.get_element_by_id("article").unwrap()).unwrap();
                    let right = find(&tree.root, doc.get_element_by_id("right").unwrap()).unwrap();
                    article.rect.x + 1.0 >= left.rect.right()
                        && right.rect.x + 1.0 >= article.rect.right()
                        && article.rect.width > 400.0
                }
                _ => false,
            };
            if ok {
                passed += 1;
            } else {
                notes.push(name.to_string());
            }
        }
        let report = format!("wpt css subset {passed}/{} {notes:?}", files.len());
        eprintln!("{report}");
        assert_eq!(passed, files.len(), "{notes:?}");
    }
}
