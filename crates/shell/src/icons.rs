//! Rasterizer for Chromium's BSD-licensed `.icon` path format.
//!
//! Commands follow the subset used by `chrome/app/vector_icons` and
//! `components/vector_icons`: move, line, cubic, quadratic, and SVG-style arcs.
//! Filled contours are scan-converted by the paint crate.

use std::sync::OnceLock;

use mithu_base::{Color, Rect};
use mithu_paint::Canvas;

#[derive(Clone)]
pub struct Icon {
    pub dim: f32,
    pub contours: Vec<Vec<(f32, f32)>>,
}

pub fn draw(canvas: &mut Canvas<'_>, kind: Kind, dest: Rect, color: Color, clip: Rect) {
    let icon = kind.icon();
    if icon.contours.is_empty() || dest.width <= 0.0 || dest.height <= 0.0 {
        return;
    }
    let scale = (dest.width / icon.dim).min(dest.height / icon.dim);
    let ox = dest.x + (dest.width - icon.dim * scale) * 0.5;
    let oy = dest.y + (dest.height - icon.dim * scale) * 0.5;
    let flip = kind == Kind::Forward;
    let contours: Vec<Vec<(f32, f32)>> = icon
        .contours
        .iter()
        .map(|contour| {
            contour
                .iter()
                .map(|(x, y)| {
                    let sx = if flip { icon.dim - x } else { *x };
                    (ox + sx * scale, oy + y * scale)
                })
                .collect()
        })
        .collect();
    canvas.fill_contours(&contours, color, clip, false);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Back,
    Forward,
    Reload,
    Home,
    Close,
    CloseTab,
    Star,
    StarFilled,
    Lock,
    Add,
    Download,
    History,
    Find,
    Settings,
    Search,
    Minimize,
    Maximize,
}

impl Kind {
    fn icon(self) -> &'static Icon {
        static BACK: OnceLock<Icon> = OnceLock::new();
        static RELOAD: OnceLock<Icon> = OnceLock::new();
        static HOME: OnceLock<Icon> = OnceLock::new();
        static CLOSE: OnceLock<Icon> = OnceLock::new();
        static CLOSE_TAB: OnceLock<Icon> = OnceLock::new();
        static STAR: OnceLock<Icon> = OnceLock::new();
        static STAR_FILLED: OnceLock<Icon> = OnceLock::new();
        static LOCK: OnceLock<Icon> = OnceLock::new();
        static ADD: OnceLock<Icon> = OnceLock::new();
        static DOWNLOAD: OnceLock<Icon> = OnceLock::new();
        static HISTORY: OnceLock<Icon> = OnceLock::new();
        static FIND: OnceLock<Icon> = OnceLock::new();
        static SETTINGS: OnceLock<Icon> = OnceLock::new();
        static SEARCH: OnceLock<Icon> = OnceLock::new();
        static MINIMIZE: OnceLock<Icon> = OnceLock::new();
        static MAXIMIZE: OnceLock<Icon> = OnceLock::new();
        match self {
            Self::Back | Self::Forward => {
                BACK.get_or_init(|| parse(include_str!("../icons/back_arrow.icon")))
            }
            Self::Reload => RELOAD.get_or_init(|| parse(include_str!("../icons/refresh.icon"))),
            Self::Home => HOME.get_or_init(|| parse(include_str!("../icons/home.icon"))),
            Self::Close => CLOSE.get_or_init(|| parse(include_str!("../icons/close.icon"))),
            Self::CloseTab => {
                CLOSE_TAB.get_or_init(|| parse(include_str!("../icons/close_tab.icon")))
            }
            Self::Star => STAR.get_or_init(|| parse(include_str!("../icons/star.icon"))),
            Self::StarFilled => {
                STAR_FILLED.get_or_init(|| parse(include_str!("../icons/star_filled.icon")))
            }
            Self::Lock => LOCK.get_or_init(|| parse(include_str!("../icons/lock.icon"))),
            Self::Add => ADD.get_or_init(|| parse(include_str!("../icons/add.icon"))),
            Self::Download => {
                DOWNLOAD.get_or_init(|| parse(include_str!("../icons/download.icon")))
            }
            Self::History => HISTORY.get_or_init(|| parse(include_str!("../icons/history.icon"))),
            Self::Find => FIND.get_or_init(|| parse(include_str!("../icons/find_in_page.icon"))),
            Self::Settings => {
                SETTINGS.get_or_init(|| parse(include_str!("../icons/settings.icon")))
            }
            Self::Search => SEARCH.get_or_init(|| parse(include_str!("../icons/tab_search.icon"))),
            Self::Minimize => {
                MINIMIZE.get_or_init(|| parse(include_str!("../icons/minimize.icon")))
            }
            Self::Maximize => {
                MAXIMIZE.get_or_init(|| parse(include_str!("../icons/maximize.icon")))
            }
        }
    }
}

pub fn parse(src: &str) -> Icon {
    let tokens = tokenize(src);
    let mut index = 0;
    let mut dim = 16.0;
    let mut pen = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut current = Vec::new();
    let mut contours = Vec::new();
    while index < tokens.len() {
        let command = tokens[index].as_str();
        index += 1;
        let take = |index: &mut usize, tokens: &[String]| -> f32 {
            let value = tokens.get(*index).map(|token| number(token)).unwrap_or(0.0);
            *index += 1;
            value
        };
        match command {
            "CANVAS_DIMENSIONS" => dim = take(&mut index, &tokens),
            "FILL_RULE_NONZERO" | "FILL_RULE_EVENODD" => {}
            "NEW_PATH" => finish(&mut current, &mut contours),
            "MOVE_TO" => {
                finish(&mut current, &mut contours);
                let x = take(&mut index, &tokens);
                let y = take(&mut index, &tokens);
                pen = (x, y);
                start = pen;
                current.push(pen);
            }
            "R_MOVE_TO" => {
                finish(&mut current, &mut contours);
                pen.0 += take(&mut index, &tokens);
                pen.1 += take(&mut index, &tokens);
                start = pen;
                current.push(pen);
            }
            "LINE_TO" => {
                pen.0 = take(&mut index, &tokens);
                pen.1 = take(&mut index, &tokens);
                current.push(pen);
            }
            "R_LINE_TO" => {
                pen.0 += take(&mut index, &tokens);
                pen.1 += take(&mut index, &tokens);
                current.push(pen);
            }
            "H_LINE_TO" => {
                pen.0 = take(&mut index, &tokens);
                current.push(pen);
            }
            "V_LINE_TO" => {
                pen.1 = take(&mut index, &tokens);
                current.push(pen);
            }
            "R_H_LINE_TO" => {
                pen.0 += take(&mut index, &tokens);
                current.push(pen);
            }
            "R_V_LINE_TO" => {
                pen.1 += take(&mut index, &tokens);
                current.push(pen);
            }
            "CUBIC_TO" => {
                let c1 = (take(&mut index, &tokens), take(&mut index, &tokens));
                let c2 = (take(&mut index, &tokens), take(&mut index, &tokens));
                let end = (take(&mut index, &tokens), take(&mut index, &tokens));
                cubic(pen, c1, c2, end, &mut current);
                pen = end;
            }
            "R_CUBIC_TO" => {
                let c1 = (
                    pen.0 + take(&mut index, &tokens),
                    pen.1 + take(&mut index, &tokens),
                );
                let c2 = (
                    pen.0 + take(&mut index, &tokens),
                    pen.1 + take(&mut index, &tokens),
                );
                let end = (
                    pen.0 + take(&mut index, &tokens),
                    pen.1 + take(&mut index, &tokens),
                );
                cubic(pen, c1, c2, end, &mut current);
                pen = end;
            }
            "QUADRATIC_TO" => {
                let control = (take(&mut index, &tokens), take(&mut index, &tokens));
                let end = (take(&mut index, &tokens), take(&mut index, &tokens));
                quadratic(pen, control, end, &mut current);
                pen = end;
            }
            "R_QUADRATIC_TO" => {
                let control = (
                    pen.0 + take(&mut index, &tokens),
                    pen.1 + take(&mut index, &tokens),
                );
                let end = (
                    pen.0 + take(&mut index, &tokens),
                    pen.1 + take(&mut index, &tokens),
                );
                quadratic(pen, control, end, &mut current);
                pen = end;
            }
            "ARC_TO" => {
                let end = arc_command(&mut index, &tokens, pen, false);
                let next = end.end;
                extend_arc(pen, end, &mut current);
                pen = next;
            }
            "R_ARC_TO" => {
                let end = arc_command(&mut index, &tokens, pen, true);
                let next = end.end;
                extend_arc(pen, end, &mut current);
                pen = next;
            }
            "CLOSE" => {
                if current.last().is_some_and(|point| *point != start) {
                    current.push(start);
                }
                finish(&mut current, &mut contours);
                pen = start;
            }
            _ => {}
        }
    }
    finish(&mut current, &mut contours);
    Icon { dim, contours }
}

struct ArcEnd {
    points: Vec<(f32, f32)>,
    end: (f32, f32),
}

fn arc_command(index: &mut usize, tokens: &[String], pen: (f32, f32), relative: bool) -> ArcEnd {
    let rx = tokens.get(*index).map(|token| number(token)).unwrap_or(0.0);
    let ry = tokens
        .get(*index + 1)
        .map(|token| number(token))
        .unwrap_or(0.0);
    let rotation = tokens
        .get(*index + 2)
        .map(|token| number(token))
        .unwrap_or(0.0);
    let large = tokens
        .get(*index + 3)
        .map(|token| number(token))
        .unwrap_or(0.0)
        > 0.5;
    let sweep = tokens
        .get(*index + 4)
        .map(|token| number(token))
        .unwrap_or(0.0)
        > 0.5;
    let mut x = tokens
        .get(*index + 5)
        .map(|token| number(token))
        .unwrap_or(0.0);
    let mut y = tokens
        .get(*index + 6)
        .map(|token| number(token))
        .unwrap_or(0.0);
    *index += 7;
    if relative {
        x += pen.0;
        y += pen.1;
    }
    ArcEnd {
        points: elliptical_arc(pen.0, pen.1, rx, ry, rotation, large, sweep, x, y),
        end: (x, y),
    }
}

fn extend_arc(pen: (f32, f32), arc: ArcEnd, current: &mut Vec<(f32, f32)>) {
    if arc.points.is_empty() {
        if arc.end != pen {
            current.push(arc.end);
        }
        return;
    }
    current.extend(arc.points);
}

fn finish(current: &mut Vec<(f32, f32)>, contours: &mut Vec<Vec<(f32, f32)>>) {
    if current.len() >= 3 {
        contours.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn tokenize(src: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        for part in line.split(',') {
            let part = part.trim().trim_end_matches(['f', 'F']);
            if !part.is_empty() {
                tokens.push(part.to_string());
            }
        }
    }
    tokens
}

fn number(token: &str) -> f32 {
    token.trim_end_matches(['f', 'F']).parse().unwrap_or(0.0)
}

fn cubic(
    start: (f32, f32),
    control1: (f32, f32),
    control2: (f32, f32),
    end: (f32, f32),
    out: &mut Vec<(f32, f32)>,
) {
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let u = 1.0 - t;
        let x = u * u * u * start.0
            + 3.0 * u * u * t * control1.0
            + 3.0 * u * t * t * control2.0
            + t * t * t * end.0;
        let y = u * u * u * start.1
            + 3.0 * u * u * t * control1.1
            + 3.0 * u * t * t * control2.1
            + t * t * t * end.1;
        out.push((x, y));
    }
}

fn quadratic(start: (f32, f32), control: (f32, f32), end: (f32, f32), out: &mut Vec<(f32, f32)>) {
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        let u = 1.0 - t;
        let x = u * u * start.0 + 2.0 * u * t * control.0 + t * t * end.0;
        let y = u * u * start.1 + 2.0 * u * t * control.1 + t * t * end.1;
        out.push((x, y));
    }
}

fn elliptical_arc(
    x1: f32,
    y1: f32,
    mut rx: f32,
    mut ry: f32,
    angle: f32,
    large: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
) -> Vec<(f32, f32)> {
    if (x1 - x2).abs() < 1e-4 && (y1 - y2).abs() < 1e-4 {
        return Vec::new();
    }
    rx = rx.abs();
    ry = ry.abs();
    if rx < 1e-4 || ry < 1e-4 {
        return vec![(x2, y2)];
    }
    let phi = angle.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    let mut rx2 = rx * rx;
    let mut ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let lambda = x1p2 / rx2 + y1p2 / ry2;
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
        rx2 = rx * rx;
        ry2 = ry * ry;
    }
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let numer = (rx2 * ry2 - rx2 * y1p2 - ry2 * x1p2).max(0.0);
    let denom = (rx2 * y1p2 + ry2 * x1p2).max(1e-8);
    let coef = sign * (numer / denom).sqrt();
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
    let steps = ((delta.abs() / 0.4).ceil() as i32).clamp(2, 16);
    let mut points = Vec::with_capacity(steps as usize);
    for step in 1..=steps {
        let theta = theta1 + delta * (step as f32 / steps as f32);
        let (sin_t, cos_t) = theta.sin_cos();
        let px = rx * cos_t;
        let py = ry * sin_t;
        points.push((
            cos_phi * px - sin_phi * py + cx,
            sin_phi * px + cos_phi * py + cy,
        ));
    }
    points
}

fn vector_angle(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let denom = ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt()).max(1e-8);
    let mut angle = ((ux * vx + uy * vy) / denom).clamp(-1.0, 1.0).acos();
    if ux * vy - uy * vx < 0.0 {
        angle = -angle;
    }
    angle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_arrow_parses_a_left_pointing_contour() {
        let icon = parse(include_str!("../icons/back_arrow.icon"));
        assert!((icon.dim - 20.0).abs() < 0.01);
        let contour = icon.contours.first().expect("contour");
        assert!(contour.len() >= 4);
        let min_x = contour.iter().map(|point| point.0).fold(f32::MAX, f32::min);
        let max_x = contour.iter().map(|point| point.0).fold(f32::MIN, f32::max);
        assert!(min_x < 8.0 && max_x > 14.0);
    }

    #[test]
    fn settings_gear_keeps_several_contours() {
        let icon = parse(include_str!("../icons/settings.icon"));
        assert!(icon.contours.len() >= 2);
    }
}
