//! Inline SVG, mapped into the element's border box.
//!
//! Paths, basic shapes, and `viewBox` are turned into contours. Curves are
//! flattened. `preserveAspectRatio` defaults to `xMidYMid meet`.

use mithu_base::Color;
use mithu_dom::{Document, NodeData, NodeId};
use mithu_style::{ComputedStyle, Length};

use crate::VectorShape;

pub(crate) fn apply_size(style: &mut ComputedStyle, doc: &Document, id: NodeId) {
    let (intrinsic_w, intrinsic_h) = intrinsic(doc, id);
    let width_attr = length_attr(doc, id, "width");
    let height_attr = length_attr(doc, id, "height");
    if matches!(style.width, Length::Auto)
        && let Some(width) = width_attr
    {
        style.width = Length::Px(width);
    }
    if matches!(style.height, Length::Auto)
        && let Some(height) = height_attr
    {
        style.height = Length::Px(height);
    }
    if matches!(style.width, Length::Auto) && matches!(style.height, Length::Auto) {
        style.width = Length::Px(intrinsic_w);
        style.height = Length::Px(intrinsic_h);
    } else if matches!(style.height, Length::Auto)
        && let Length::Px(width) = style.width
        && intrinsic_w > 0.0
    {
        style.height = Length::Px(intrinsic_h * width / intrinsic_w);
    } else if matches!(style.width, Length::Auto)
        && let Length::Px(height) = style.height
        && intrinsic_h > 0.0
    {
        style.width = Length::Px(intrinsic_w * height / intrinsic_h);
    }
}

pub(crate) fn shapes(
    doc: &Document,
    styles: &[ComputedStyle],
    id: NodeId,
    width: f32,
    height: f32,
    color: Color,
) -> Vec<VectorShape> {
    let view = view_box(doc, id).unwrap_or(ViewBox {
        x: 0.0,
        y: 0.0,
        width: width.max(1.0),
        height: height.max(1.0),
    });
    let mut out = Vec::new();
    walk(
        doc,
        styles,
        id,
        &view,
        width,
        height,
        0.0,
        0.0,
        false,
        &mut Vec::new(),
        Paint {
            fill: Some(Color::BLACK),
            stroke: None,
            stroke_width: 1.0,
            color,
            even_odd: false,
        },
        &mut out,
    );
    out
}

fn intrinsic(doc: &Document, id: NodeId) -> (f32, f32) {
    if let Some(view) = view_box(doc, id) {
        return (view.width.max(1.0), view.height.max(1.0));
    }
    (
        length_attr(doc, id, "width").unwrap_or(300.0),
        length_attr(doc, id, "height").unwrap_or(150.0),
    )
}

struct ViewBox {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy)]
struct Paint {
    fill: Option<Color>,
    stroke: Option<Color>,
    stroke_width: f32,
    color: Color,
    even_odd: bool,
}

fn walk(
    doc: &Document,
    styles: &[ComputedStyle],
    id: NodeId,
    view: &ViewBox,
    width: f32,
    height: f32,
    dx: f32,
    dy: f32,
    paint_hidden: bool,
    seen: &mut Vec<NodeId>,
    mut paint: Paint,
    out: &mut Vec<VectorShape>,
) {
    let name = doc.element_name(id).unwrap_or("");
    if !paint_hidden && matches!(name, "defs" | "symbol" | "clippath" | "mask") {
        return;
    }
    if let Some(style) = styles.get(id.0) {
        paint.color = style.color;
        if let Some(fill) = style.fill {
            paint.fill = Some(fill);
        }
    }
    paint = paint_of(doc, id, paint);
    if name == "use" {
        let href = doc
            .attr(id, "href")
            .or_else(|| doc.attr(id, "xlink:href"))
            .unwrap_or("");
        let Some(target) = href
            .strip_prefix('#')
            .and_then(|id| doc.get_element_by_id(id))
        else {
            return;
        };
        if seen.contains(&target) {
            return;
        }
        let ox = doc
            .attr(id, "x")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0);
        let oy = doc
            .attr(id, "y")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0);
        seen.push(target);
        walk(
            doc,
            styles,
            target,
            view,
            width,
            height,
            dx + ox,
            dy + oy,
            true,
            seen,
            paint,
            out,
        );
        seen.pop();
        return;
    }
    let contours = match name {
        "path" => doc.attr(id, "d").map(parse_path).unwrap_or_default(),
        "rect" => rect_contour(doc, id),
        "circle" => circle_contour(doc, id),
        "ellipse" => ellipse_contour(doc, id),
        "line" => line_contour(doc, id),
        "polyline" | "polygon" => poly_contour(doc, id, name == "polygon"),
        _ => Vec::new(),
    };
    let mapped: Vec<Vec<(f32, f32)>> = contours
        .into_iter()
        .filter(|points| points.len() >= 2)
        .map(|points| {
            points
                .into_iter()
                .map(|(x, y)| map_point(x + dx, y + dy, view, width, height))
                .collect()
        })
        .collect();
    if !mapped.is_empty() {
        let scale = uniform_scale(view, width, height);
        // One path's subpaths share a fill so holes punch out of the outline.
        // Basic shapes stay one contour each.
        let groups: Vec<Vec<Vec<(f32, f32)>>> = if name == "path" {
            vec![mapped]
        } else {
            mapped.into_iter().map(|points| vec![points]).collect()
        };
        for group in groups {
            out.push(VectorShape {
                fill: paint.fill,
                stroke: paint.stroke,
                stroke_width: paint.stroke_width * scale,
                points: group[0].clone(),
                contours: group,
                even_odd: paint.even_odd,
            });
        }
    }
    for child in doc.node(id).children.clone() {
        if matches!(doc.node(child).data, NodeData::Element(_)) {
            walk(
                doc,
                styles,
                child,
                view,
                width,
                height,
                dx,
                dy,
                paint_hidden,
                seen,
                paint,
                out,
            );
        }
    }
}

fn paint_of(doc: &Document, id: NodeId, parent: Paint) -> Paint {
    let mut paint = parent;
    if let Some(value) = doc.attr(id, "fill") {
        paint.fill = parse_paint(value, parent.color);
    }
    if let Some(value) = doc.attr(id, "stroke") {
        paint.stroke = parse_paint(value, parent.color);
    }
    if let Some(value) = doc.attr(id, "stroke-width")
        && let Some(width) = value.parse::<f32>().ok()
    {
        paint.stroke_width = width.max(0.0);
    }
    if let Some(value) = doc.attr(id, "fill-rule") {
        paint.even_odd = value.eq_ignore_ascii_case("evenodd");
    }
    if let Some(style) = doc.attr(id, "style") {
        for part in style.split(';') {
            let mut pair = part.splitn(2, ':');
            let key = pair.next().unwrap_or("").trim();
            let value = pair.next().unwrap_or("").trim();
            match key {
                "fill" => paint.fill = parse_paint(value, parent.color),
                "stroke" => paint.stroke = parse_paint(value, parent.color),
                "stroke-width" => {
                    if let Ok(width) = value.parse::<f32>() {
                        paint.stroke_width = width.max(0.0);
                    }
                }
                "fill-rule" => paint.even_odd = value.eq_ignore_ascii_case("evenodd"),
                _ => {}
            }
        }
    }
    paint
}

fn parse_paint(value: &str, current: Color) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    if value.eq_ignore_ascii_case("currentcolor") {
        return Some(current);
    }
    // Gradient and pattern paints fall back to a solid silhouette.
    if value.starts_with("url(") {
        return Some(Color::BLACK);
    }
    parse_color(value)
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => Some(Color::rgb(
                hex_byte(hex.as_bytes()[0])? * 17,
                hex_byte(hex.as_bytes()[1])? * 17,
                hex_byte(hex.as_bytes()[2])? * 17,
            )),
            6 | 8 => Some(Color::rgb(
                hex_pair(&hex.as_bytes()[0..2])?,
                hex_pair(&hex.as_bytes()[2..4])?,
                hex_pair(&hex.as_bytes()[4..6])?,
            )),
            _ => None,
        };
    }
    Some(match value.to_ascii_lowercase().as_str() {
        "black" => Color::BLACK,
        "white" => Color::WHITE,
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "currentcolor" => return None,
        _ => return None,
    })
}

fn hex_byte(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_pair(bytes: &[u8]) -> Option<u8> {
    Some(hex_byte(bytes[0])? * 16 + hex_byte(bytes[1])?)
}

fn map_point(x: f32, y: f32, view: &ViewBox, width: f32, height: f32) -> (f32, f32) {
    let scale = uniform_scale(view, width, height);
    let used_w = view.width * scale;
    let used_h = view.height * scale;
    let dx = (width - used_w) / 2.0;
    let dy = (height - used_h) / 2.0;
    ((x - view.x) * scale + dx, (y - view.y) * scale + dy)
}

fn uniform_scale(view: &ViewBox, width: f32, height: f32) -> f32 {
    if view.width <= 0.0 || view.height <= 0.0 {
        return 1.0;
    }
    (width / view.width).min(height / view.height)
}

fn view_box(doc: &Document, id: NodeId) -> Option<ViewBox> {
    let raw = doc
        .attr(id, "viewBox")
        .or_else(|| doc.attr(id, "viewbox"))?;
    let nums = numbers(raw);
    if nums.len() < 4 || nums[2] <= 0.0 || nums[3] <= 0.0 {
        return None;
    }
    Some(ViewBox {
        x: nums[0],
        y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

fn length_attr(doc: &Document, id: NodeId, name: &str) -> Option<f32> {
    let raw = doc.attr(id, name)?;
    let raw = raw.trim().trim_end_matches("px");
    raw.parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn rect_contour(doc: &Document, id: NodeId) -> Vec<Vec<(f32, f32)>> {
    let x = attr_f32(doc, id, "x");
    let y = attr_f32(doc, id, "y");
    let w = attr_f32(doc, id, "width");
    let h = attr_f32(doc, id, "height");
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    vec![vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]]
}

fn circle_contour(doc: &Document, id: NodeId) -> Vec<Vec<(f32, f32)>> {
    let cx = attr_f32(doc, id, "cx");
    let cy = attr_f32(doc, id, "cy");
    let r = attr_f32(doc, id, "r");
    if r <= 0.0 {
        return Vec::new();
    }
    vec![ellipse(cx, cy, r, r)]
}

fn ellipse_contour(doc: &Document, id: NodeId) -> Vec<Vec<(f32, f32)>> {
    let rx = attr_f32(doc, id, "rx");
    let ry = attr_f32(doc, id, "ry");
    if rx <= 0.0 || ry <= 0.0 {
        return Vec::new();
    }
    vec![ellipse(
        attr_f32(doc, id, "cx"),
        attr_f32(doc, id, "cy"),
        rx,
        ry,
    )]
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    for step in 0..=24 {
        let t = step as f32 / 24.0 * std::f32::consts::TAU;
        points.push((cx + t.cos() * rx, cy + t.sin() * ry));
    }
    points
}

fn line_contour(doc: &Document, id: NodeId) -> Vec<Vec<(f32, f32)>> {
    vec![vec![
        (attr_f32(doc, id, "x1"), attr_f32(doc, id, "y1")),
        (attr_f32(doc, id, "x2"), attr_f32(doc, id, "y2")),
    ]]
}

fn poly_contour(doc: &Document, id: NodeId, close: bool) -> Vec<Vec<(f32, f32)>> {
    let nums = doc.attr(id, "points").map(numbers).unwrap_or_default();
    let mut points = Vec::new();
    for pair in nums.chunks(2) {
        if pair.len() == 2 {
            points.push((pair[0], pair[1]));
        }
    }
    if close && let Some(first) = points.first().copied() {
        points.push(first);
    }
    if points.is_empty() {
        Vec::new()
    } else {
        vec![points]
    }
}

fn attr_f32(doc: &Document, id: NodeId, name: &str) -> f32 {
    doc.attr(id, name)
        .and_then(|value| value.trim().trim_end_matches("px").parse().ok())
        .unwrap_or(0.0)
}

fn parse_path(d: &str) -> Vec<Vec<(f32, f32)>> {
    let bytes = d.as_bytes();
    let mut index = 0;
    let mut contours = Vec::new();
    let mut current = Vec::new();
    let mut x = 0.0;
    let mut y = 0.0;
    let mut start_x = 0.0;
    let mut start_y = 0.0;
    let mut prev_ctrl_x = 0.0;
    let mut prev_ctrl_y = 0.0;
    let mut prev = ' ';
    while index < bytes.len() {
        skip_seps(bytes, &mut index);
        if index >= bytes.len() {
            break;
        }
        let mut command = bytes[index] as char;
        if command.is_ascii_alphabetic() {
            index += 1;
        } else if matches!(
            prev,
            'M' | 'm'
                | 'L'
                | 'l'
                | 'C'
                | 'c'
                | 'Q'
                | 'q'
                | 'S'
                | 's'
                | 'H'
                | 'h'
                | 'V'
                | 'v'
                | 'A'
                | 'a'
        ) {
            command = if prev == 'M' {
                'L'
            } else if prev == 'm' {
                'l'
            } else {
                prev
            };
        } else {
            break;
        }
        let relative = command.is_ascii_lowercase();
        match command.to_ascii_uppercase() {
            'M' => {
                if let Some((nx, ny)) = take_pair(bytes, &mut index) {
                    if !current.is_empty() {
                        contours.push(std::mem::take(&mut current));
                    }
                    x = if relative { x + nx } else { nx };
                    y = if relative { y + ny } else { ny };
                    start_x = x;
                    start_y = y;
                    current.push((x, y));
                    prev = if relative { 'l' } else { 'L' };
                }
            }
            'L' => {
                if let Some((nx, ny)) = take_pair(bytes, &mut index) {
                    x = if relative { x + nx } else { nx };
                    y = if relative { y + ny } else { ny };
                    current.push((x, y));
                    prev = command;
                }
            }
            'H' => {
                if let Some(nx) = take_one(bytes, &mut index) {
                    x = if relative { x + nx } else { nx };
                    current.push((x, y));
                    prev = command;
                }
            }
            'V' => {
                if let Some(ny) = take_one(bytes, &mut index) {
                    y = if relative { y + ny } else { ny };
                    current.push((x, y));
                    prev = command;
                }
            }
            'C' => {
                if let (Some((x1, y1)), Some((x2, y2)), Some((x3, y3))) = (
                    take_pair(bytes, &mut index),
                    take_pair(bytes, &mut index),
                    take_pair(bytes, &mut index),
                ) {
                    let (c1x, c1y) = abs(relative, x, y, x1, y1);
                    let (c2x, c2y) = abs(relative, x, y, x2, y2);
                    let (nx, ny) = abs(relative, x, y, x3, y3);
                    cubic(&mut current, x, y, c1x, c1y, c2x, c2y, nx, ny);
                    prev_ctrl_x = c2x;
                    prev_ctrl_y = c2y;
                    x = nx;
                    y = ny;
                    prev = command;
                }
            }
            'S' => {
                if let (Some((x2, y2)), Some((x3, y3))) =
                    (take_pair(bytes, &mut index), take_pair(bytes, &mut index))
                {
                    let (c1x, c1y) = if matches!(prev, 'C' | 'c' | 'S' | 's') {
                        (x * 2.0 - prev_ctrl_x, y * 2.0 - prev_ctrl_y)
                    } else {
                        (x, y)
                    };
                    let (c2x, c2y) = abs(relative, x, y, x2, y2);
                    let (nx, ny) = abs(relative, x, y, x3, y3);
                    cubic(&mut current, x, y, c1x, c1y, c2x, c2y, nx, ny);
                    prev_ctrl_x = c2x;
                    prev_ctrl_y = c2y;
                    x = nx;
                    y = ny;
                    prev = command;
                }
            }
            'Q' => {
                if let (Some((x1, y1)), Some((x2, y2))) =
                    (take_pair(bytes, &mut index), take_pair(bytes, &mut index))
                {
                    let (c1x, c1y) = abs(relative, x, y, x1, y1);
                    let (nx, ny) = abs(relative, x, y, x2, y2);
                    quadratic(&mut current, x, y, c1x, c1y, nx, ny);
                    prev_ctrl_x = c1x;
                    prev_ctrl_y = c1y;
                    x = nx;
                    y = ny;
                    prev = command;
                }
            }
            'Z' => {
                current.push((start_x, start_y));
                x = start_x;
                y = start_y;
                prev = command;
            }
            'A' => {
                let Some(rx) = take_one(bytes, &mut index) else {
                    break;
                };
                let Some(ry) = take_one(bytes, &mut index) else {
                    break;
                };
                let Some(rotation) = take_one(bytes, &mut index) else {
                    break;
                };
                let Some(large) = take_one(bytes, &mut index) else {
                    break;
                };
                let Some(sweep) = take_one(bytes, &mut index) else {
                    break;
                };
                let Some((nx, ny)) = take_pair(bytes, &mut index) else {
                    break;
                };
                let (nx, ny) = abs(relative, x, y, nx, ny);
                arc(
                    &mut current,
                    x,
                    y,
                    rx,
                    ry,
                    rotation,
                    large != 0.0,
                    sweep != 0.0,
                    nx,
                    ny,
                );
                x = nx;
                y = ny;
                prev = command;
            }
            _ => break,
        }
    }
    if !current.is_empty() {
        contours.push(current);
    }
    contours
}

fn arc(
    points: &mut Vec<(f32, f32)>,
    x1: f32,
    y1: f32,
    mut rx: f32,
    mut ry: f32,
    rotation: f32,
    large: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
) {
    if (x1 - x2).abs() < f32::EPSILON && (y1 - y2).abs() < f32::EPSILON {
        return;
    }
    if rx.abs() < f32::EPSILON || ry.abs() < f32::EPSILON {
        points.push((x2, y2));
        return;
    }
    let phi = rotation.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    rx = rx.abs();
    ry = ry.abs();
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let numer = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let denom = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let coef = if denom <= f32::EPSILON {
        0.0
    } else {
        sign * (numer / denom).sqrt()
    };
    let cxp = coef * rx * y1p / ry;
    let cyp = coef * -ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (x1 + x2) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (y1 + y2) / 2.0;
    let theta1 = vector_angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = vector_angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    }
    if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs() / (std::f32::consts::PI / 8.0)).ceil() as i32).max(1);
    for step in 1..=steps {
        let angle = theta1 + delta * step as f32 / steps as f32;
        let (sin_t, cos_t) = angle.sin_cos();
        points.push((
            cx + rx * cos_t * cos_phi - ry * sin_t * sin_phi,
            cy + rx * cos_t * sin_phi + ry * sin_t * cos_phi,
        ));
    }
}

fn vector_angle(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let dot = ux * vx + uy * vy;
    let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt();
    if len <= f32::EPSILON {
        return 0.0;
    }
    let mut angle = (dot / len).clamp(-1.0, 1.0).acos();
    if ux * vy - uy * vx < 0.0 {
        angle = -angle;
    }
    angle
}

fn abs(relative: bool, x: f32, y: f32, nx: f32, ny: f32) -> (f32, f32) {
    if relative { (x + nx, y + ny) } else { (nx, ny) }
}

fn cubic(
    points: &mut Vec<(f32, f32)>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    x3: f32,
    y3: f32,
) {
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let u = 1.0 - t;
        let x = u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x3;
        let y = u * u * u * y0 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y3;
        points.push((x, y));
    }
}

fn quadratic(points: &mut Vec<(f32, f32)>, x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32) {
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let u = 1.0 - t;
        let x = u * u * x0 + 2.0 * u * t * x1 + t * t * x2;
        let y = u * u * y0 + 2.0 * u * t * y1 + t * t * y2;
        points.push((x, y));
    }
}

fn take_pair(bytes: &[u8], index: &mut usize) -> Option<(f32, f32)> {
    Some((take_one(bytes, index)?, take_one(bytes, index)?))
}

fn take_one(bytes: &[u8], index: &mut usize) -> Option<f32> {
    skip_seps(bytes, index);
    let start = *index;
    if *index < bytes.len() && (bytes[*index] == b'+' || bytes[*index] == b'-') {
        *index += 1;
    }
    let mut saw = false;
    while *index < bytes.len() && bytes[*index].is_ascii_digit() {
        saw = true;
        *index += 1;
    }
    if *index < bytes.len() && bytes[*index] == b'.' {
        *index += 1;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            saw = true;
            *index += 1;
        }
    }
    if *index < bytes.len() && (bytes[*index] == b'e' || bytes[*index] == b'E') {
        let saved = *index;
        *index += 1;
        if *index < bytes.len() && (bytes[*index] == b'+' || bytes[*index] == b'-') {
            *index += 1;
        }
        let exp = *index;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        if *index == exp {
            *index = saved;
        }
    }
    if !saw {
        return None;
    }
    std::str::from_utf8(&bytes[start..*index])
        .ok()
        .and_then(|text| text.parse().ok())
}

fn skip_seps(bytes: &[u8], index: &mut usize) {
    while *index < bytes.len() {
        let byte = bytes[*index];
        if byte == b',' || byte.is_ascii_whitespace() {
            *index += 1;
        } else {
            break;
        }
    }
}

fn numbers(input: &str) -> Vec<f32> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut out = Vec::new();
    while let Some(value) = take_one(bytes, &mut index) {
        out.push(value);
    }
    out
}

/// Serialize an inline `<svg>` so resvg can paint gradients, clips, and masks.
/// The HTML parser stores names in lowercase; SVG wants the camelCase form.
pub(crate) fn markup(doc: &Document, id: NodeId, styles: &[ComputedStyle], color: Color) -> String {
    let mut out = String::new();
    write_node(doc, id, styles, color, true, &mut out);
    out
}

fn write_node(
    doc: &Document,
    id: NodeId,
    styles: &[ComputedStyle],
    color: Color,
    root: bool,
    out: &mut String,
) {
    let (kind, children) = {
        let node = doc.node(id);
        let children = node.children.clone();
        let kind = match &node.data {
            NodeData::Text(text) => NodeKind::Text(text.clone()),
            NodeData::Element(element) => NodeKind::Element {
                name: svg_ident(&element.name).to_string(),
                attrs: element.attrs.clone(),
            },
            NodeData::Document | NodeData::Comment(_) => NodeKind::Skip,
        };
        (kind, children)
    };
    match kind {
        NodeKind::Skip => {}
        NodeKind::Text(text) => push_escaped(out, &text),
        NodeKind::Element { name, attrs } => {
            out.push('<');
            out.push_str(&name);
            if root && attrs.iter().all(|(key, _)| key != "xmlns") {
                out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
            }
            let mut wrote_color = false;
            for (key, value) in &attrs {
                let key = svg_ident(key);
                out.push(' ');
                out.push_str(key);
                out.push_str("=\"");
                if root && key == "style" && !value.contains("color:") {
                    out.push_str("color:");
                    out.push_str(&hex(color));
                    out.push(';');
                    wrote_color = true;
                }
                push_escaped(out, value);
                out.push('"');
            }
            if root && !wrote_color {
                out.push_str(" style=\"color:");
                out.push_str(&hex(color));
                out.push('"');
            }
            if let Some(style) = styles.get(id.0)
                && let Some(fill) = style.fill
                && attrs.iter().all(|(key, _)| key != "fill")
            {
                out.push_str(" fill=\"");
                out.push_str(&hex(fill));
                out.push('"');
            }
            out.push('>');
            for child in children {
                write_node(doc, child, styles, color, false, out);
            }
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
        }
    }
}

enum NodeKind {
    Skip,
    Text(String),
    Element { name: String, attrs: Vec<(String, String)> },
}

fn svg_ident(name: &str) -> &str {
    match name {
        "lineargradient" => "linearGradient",
        "radialgradient" => "radialGradient",
        "clippath" => "clipPath",
        "textpath" => "textPath",
        "foreignobject" => "foreignObject",
        "animatetransform" => "animateTransform",
        "animatemotion" => "animateMotion",
        "feblend" => "feBlend",
        "fecolormatrix" => "feColorMatrix",
        "fecomponenttransfer" => "feComponentTransfer",
        "fecomposite" => "feComposite",
        "feconvolvematrix" => "feConvolveMatrix",
        "fediffuselighting" => "feDiffuseLighting",
        "fedisplacementmap" => "feDisplacementMap",
        "fedistantlight" => "feDistantLight",
        "fedropshadow" => "feDropShadow",
        "feflood" => "feFlood",
        "fefunca" => "feFuncA",
        "fefuncb" => "feFuncB",
        "fefuncg" => "feFuncG",
        "fefuncr" => "feFuncR",
        "fegaussianblur" => "feGaussianBlur",
        "feimage" => "feImage",
        "femerge" => "feMerge",
        "femergenode" => "feMergeNode",
        "femorphology" => "feMorphology",
        "feoffset" => "feOffset",
        "fepointlight" => "fePointLight",
        "fespecularlighting" => "feSpecularLighting",
        "fespotlight" => "feSpotLight",
        "fetile" => "feTile",
        "feturbulence" => "feTurbulence",
        "viewbox" => "viewBox",
        "preserveaspectratio" => "preserveAspectRatio",
        "gradientunits" => "gradientUnits",
        "gradienttransform" => "gradientTransform",
        "patternunits" => "patternUnits",
        "patterncontentunits" => "patternContentUnits",
        "patterntransform" => "patternTransform",
        "clippathunits" => "clipPathUnits",
        "maskunits" => "maskUnits",
        "maskcontentunits" => "maskContentUnits",
        "filterunits" => "filterUnits",
        "primitiveunits" => "primitiveUnits",
        "stddeviation" => "stdDeviation",
        "basefrequency" => "baseFrequency",
        "numoctaves" => "numOctaves",
        "spreadmethod" => "spreadMethod",
        "attributename" => "attributeName",
        "repeatcount" => "repeatCount",
        "repeatdur" => "repeatDur",
        "keypoints" => "keyPoints",
        "keytimes" => "keyTimes",
        "keysplines" => "keySplines",
        "calcmode" => "calcMode",
        "pathlength" => "pathLength",
        "textlength" => "textLength",
        "lengthadjust" => "lengthAdjust",
        "startoffset" => "startOffset",
        "refx" => "refX",
        "refy" => "refY",
        "markerwidth" => "markerWidth",
        "markerheight" => "markerHeight",
        "markerunits" => "markerUnits",
        "stopcolor" => "stop-color",
        "stopopacity" => "stop-opacity",
        other => other,
    }
}

fn hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn push_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}
