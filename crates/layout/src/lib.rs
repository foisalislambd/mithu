//! Block, flex, grid, and inline layout.
//!
//! Geometry comes from Taffy (block, flex, grid, floats) and Parley line
//! breaking, driven by Stylo computed values. See `from_engine`.

use std::collections::HashMap;
use std::sync::Arc;

use mithu_base::{Edges, Rect};
use mithu_dom::{Document, NodeId};
use mithu_image::Bitmap;
use mithu_style::ComputedStyle;
use mithu_text::MeasureText;

mod from_engine;
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
    /// Every subpath of this shape. A fill uses them together so a hole
    /// subtracts from the outer contour. Empty means `points` is the only one.
    pub contours: Vec<Vec<(f32, f32)>>,
    /// SVG `fill-rule="evenodd"`. Otherwise the fill is nonzero winding.
    pub even_odd: bool,
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
    return from_engine::layout(
        doc,
        styles,
        viewport_width,
        viewport_height,
        measure,
        images,
    );
}

/// Contours for an inline `<svg>` painted into a box of `width` by `height`.
pub fn svg_shapes(
    doc: &Document,
    id: NodeId,
    width: f32,
    height: f32,
    color: mithu_base::Color,
) -> Vec<VectorShape> {
    svg::shapes(doc, &[], id, width, height, color)
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
        assert_eq!(text, "AB CD");
    }

    #[test]
    fn a_space_between_inline_spans_is_kept() {
        let doc = parse_html(r#"<h1 id="h"><span>The</span> <span>Progressive</span></h1>"#);
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 800.0, 400.0, &NaiveMetrics, &HashMap::new());
        let heading = find(&tree.root, doc.get_element_by_id("h").unwrap()).unwrap();
        let text = all_text(heading);
        assert!(
            text.contains("The") && text.contains("Progressive") && text.contains(' '),
            "collapsed to {text:?}"
        );
        let mut pieces: Vec<_> = heading
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .filter(|fragment| !fragment.text.trim().is_empty())
            .collect();
        pieces.sort_by(|left, right| left.x.partial_cmp(&right.x).unwrap());
        for pair in pieces.windows(2) {
            assert!(
                pair[1].x + 1.0 >= pair[0].x + pair[0].width * 0.85,
                "{:?} overlaps {:?}",
                pair[0].text,
                pair[1].text
            );
        }
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
        // A following block's border box stays full width; it shares the float's band
        // and `clear` moves the next box below it.
        assert!(
            main.rect.y < side.rect.bottom(),
            "main y {} dropped below the float {}",
            main.rect.y,
            side.rect.bottom()
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
        assert!(
            flag.rect.width < 80.0,
            "float stretched to {}",
            flag.rect.width
        );
        assert!(
            tag.rect.width > 400.0,
            "tagline squeezed to {}",
            tag.rect.width
        );
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
            tools.rect.y + 1.0 >= title.rect.bottom() && tools.rect.y < title.rect.bottom() + 40.0,
            "toolbar y {} title bottom {}",
            tools.rect.y,
            title.rect.bottom()
        );
        assert!(
            body.rect.y + 1.0 >= tools.rect.bottom() && body.rect.y < tools.rect.bottom() + 40.0,
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
            body.rect.x > side.rect.right() - 2.0,
            "article x {}",
            body.rect.x
        );
        assert!(
            body.rect.y + 1.0 >= title.rect.bottom() && body.rect.y < side.rect.bottom(),
            "article dropped below the sidebar: title {} body {} side {}",
            title.rect.bottom(),
            body.rect.y,
            side.rect.bottom()
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
    fn a_shadow_root_replaces_the_light_dom() {
        let mut doc = parse_html(r#"<div id="host">light</div>"#);
        let host = doc.get_element_by_id("host").unwrap();
        let shadow = doc.create_element_detached("mithu-shadow");
        let paragraph = doc.create_element_detached("p");
        doc.append_child(shadow, paragraph);
        doc.set_text_content(paragraph, "shadow text");
        doc.set_attr(host, "mithu-shadow", &shadow.0.to_string());
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 200.0, &NaiveMetrics, &HashMap::new());
        let host_box = find(&tree.root, host).unwrap();
        fn texts(box_: &LayoutBox) -> String {
            let mut out = String::new();
            for line in &box_.lines {
                for fragment in &line.fragments {
                    out.push_str(&fragment.text);
                }
            }
            for child in &box_.children {
                out.push_str(&texts(child));
            }
            out
        }
        let text = texts(host_box);
        assert!(
            text.contains("shadow") && !text.contains("light"),
            "laid out {text:?}"
        );
    }

    #[test]
    fn vector_rows_stack_and_the_infobox_floats() {
        let doc = parse_html(
            r#"<style>
            @media (min-width: 1120px) {
              .mw-body { display: grid; grid-template: min-content min-content min-content 1fr / minmax(0, 59.25rem) min-content;
                grid-template-areas: 'titlebar-cx .' 'titlebar columnEnd' 'toolbar columnEnd' 'content columnEnd'; }
              .vector-page-titlebar { grid-area: titlebar; }
              .vector-page-toolbar { grid-area: toolbar; }
              #bodyContent { grid-area: content; }
            }
            @media (min-width: 640px) { .infobox { float: right; width: 160px; } }
            </style>
            <div class="mw-body"><div class="vector-page-titlebar"><h1 id="title">Rust</h1></div>
            <div class="vector-page-toolbar" id="tools">Read Edit</div>
            <div id="bodyContent"><table class="infobox"><tr><td><img id="pic" width="128" height="128" alt=""></td></tr></table>
            <p id="lead">Rust is a general-purpose programming language that emphasizes performance and safety.</p></div></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        let title = find(&tree.root, doc.get_element_by_id("title").unwrap()).unwrap();
        let tools = find(&tree.root, doc.get_element_by_id("tools").unwrap()).unwrap();
        let pic = find(&tree.root, doc.get_element_by_id("pic").unwrap()).unwrap();
        let lead = find(&tree.root, doc.get_element_by_id("lead").unwrap()).unwrap();
        assert!(
            tools.rect.y + 1.0 >= title.rect.bottom(),
            "toolbar y {} overlaps title bottom {}",
            tools.rect.y,
            title.rect.bottom()
        );
        let text_right = text_edge(lead);
        assert!(
            text_right < pic.rect.x + 1.0,
            "lead text ends at {text_right}, logo starts at {}",
            pic.rect.x
        );
    }

    #[test]
    fn a_floated_infobox_does_not_cover_the_lead() {
        let doc = parse_html(
            r#"<div id="out"><table id="box" style="float:right;width:160px"><tr><td><img id="pic" width="128" height="128" alt=""></td></tr></table><p id="lead">Rust is a general-purpose programming language that emphasizes performance and safety.</p></div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 800.0, 600.0, &NaiveMetrics, &HashMap::new());
        let pic = find(&tree.root, doc.get_element_by_id("pic").unwrap()).unwrap();
        let lead = find(&tree.root, doc.get_element_by_id("lead").unwrap()).unwrap();
        let text_right = text_edge(lead);
        assert!(
            text_right < pic.rect.x + 1.0,
            "lead text ends at {text_right}, logo starts at {} lead {:?}",
            pic.rect.x, lead.rect
        );
    }

    fn text_edge(box_: &LayoutBox) -> f32 {
        let mut right = 0.0_f32;
        for line in &box_.lines {
            for fragment in &line.fragments {
                right = right.max(fragment.x + fragment.width);
            }
        }
        for child in &box_.children {
            right = right.max(text_edge(child));
        }
        right
    }

    #[test]
    fn an_inline_image_lifts_the_line() {
        let doc =
            parse_html(r#"<p id="p">Hi <img id="pic" width="40" height="80" alt=""> tail</p>"#);
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
    fn negative_margin_block_lines_up_the_nav_labels() {
        let doc = parse_html(
            r#"<header data-marketing-header="true"><ul class="list"><li><div class="box"><button id="products" class="btn">Products</button></div></li><li><a id="pricing" class="link">Pricing</a></li></ul></header>"#,
        );
        let styles = style_document(
            &doc,
            r#"
            :root { --pad: 1.25rem; --h: 1.75rem; }
            .list { display: flex; align-items: flex-start; }
            [data-marketing-header=true] .box {
              margin-block: calc(-1 * var(--pad));
              padding-block: var(--pad);
            }
            .btn, .link { display: flex; height: var(--h); padding: 0 8px; }
            "#,
        );
        let tree = layout_tree(&doc, &styles, 1280.0, 200.0, &NaiveMetrics, &HashMap::new());
        let products = find(&tree.root, doc.get_element_by_id("products").unwrap()).unwrap();
        let pricing = find(&tree.root, doc.get_element_by_id("pricing").unwrap()).unwrap();
        assert!(
            (products.rect.y - pricing.rect.y).abs() < 1.0,
            "products y {} pricing y {}",
            products.rect.y,
            pricing.rect.y
        );
    }

    #[test]
    fn a_missing_grid_area_does_not_overflow() {
        let doc = parse_html(
            r#"<div id="g" style="display:grid;width:200px;grid-template-areas:'a'">
                <div id="b" style="grid-area:missing;height:12px">x</div>
              </div>"#,
        );
        let styles = style_document(&doc, "");
        let tree = layout_tree(&doc, &styles, 400.0, 300.0, &NaiveMetrics, &HashMap::new());
        assert!(find(&tree.root, doc.get_element_by_id("b").unwrap()).is_some());
    }

    #[test]
    fn aspect_ratio_gives_an_empty_video_a_height() {
        let doc = parse_html(r#"<div id="hero"><video id="clip"></video></div>"#);
        let styles = style_document(
            &doc,
            "#hero { width: 800px } video { width: 100%; height: auto; aspect-ratio: 1600/1078; display: block }",
        );
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        let clip = find(&tree.root, doc.get_element_by_id("clip").unwrap()).unwrap();
        let expected = 800.0 * 1078.0 / 1600.0;
        assert!(
            (clip.rect.height - expected).abs() < 4.0,
            "video height {} expected {expected}",
            clip.rect.height
        );
    }

    #[test]
    fn github_nav_labels_share_the_bar_with_search() {
        // Reduced from the live marketing-header sheet: the desktop bar is a
        // row, the menu list is a row, and Pricing is a link beside the buttons.
        let css = r#"
            .link { display: flex; align-items: center; height: 68px; padding: 24px; }
            .bar { display: flex; flex-direction: column; width: 100%; }
            .menu { display: flex; flex-direction: column; width: 100%; padding-top: 68px; }
            .wrap { display: flex; flex-direction: column; width: 100%; }
            .cta { display: flex; width: 100%; }
            .search { width: 100%; height: 36px; }
            @media (width >= 1012px) {
                .link { height: 28px; padding: 0 8px; }
                .list { display: flex; margin: 0; padding: 0; }
                .bar { flex-direction: row; align-items: center; min-height: 68px; }
                .menu { width: auto; padding-top: 0; }
                .wrap { flex-direction: row; align-items: center; }
                .cta { width: auto; margin-left: 16px; flex-direction: row; }
                .search { width: 200px; }
            }
        "#;
        let doc = parse_html(
            r#"<header>
                <div id="bar" class="bar">
                  <div id="logo">logo</div>
                  <div id="menu" class="menu">
                    <div id="wrap" class="wrap">
                      <ul id="list" class="list">
                        <li><button id="products" class="link">Products</button></li>
                        <li><button id="dev" class="link">Developers</button></li>
                        <li><a id="pricing" class="link">Pricing</a></li>
                      </ul>
                      <div id="cta" class="cta"><div id="search" class="search">Search</div></div>
                    </div>
                  </div>
                </div>
              </header>"#,
        );
        let styles = style_document(&doc, css);
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        let products = find(&tree.root, doc.get_element_by_id("products").unwrap()).unwrap();
        let pricing = find(&tree.root, doc.get_element_by_id("pricing").unwrap()).unwrap();
        let search = find(&tree.root, doc.get_element_by_id("search").unwrap()).unwrap();
        assert!(
            (products.rect.y - pricing.rect.y).abs() < 8.0,
            "labels should share a baseline: products {:?} pricing {:?}",
            products.rect,
            pricing.rect
        );
        assert!(
            search.rect.x + 1.0 >= pricing.rect.right(),
            "search {:?} should start after pricing {:?}",
            search.rect,
            pricing.rect
        );
    }

    #[test]
    fn github_header_hides_the_menu_button_at_desktop() {
        // Reduced from github.githubassets.com marketing-header.module.css.
        let css = r#"
            .visuallyHidden__TIbnmb { position: absolute; width: 1px; height: 1px; margin: 0; padding: 0; overflow: hidden; }
            .toggle__kPg75- { display: inline-block; }
            @media (width>=1012px) { .toggle__kPg75- { display: none; } .toggleSlot__-ODxSX { display: none; } }
            .bar__cF4m1O { display: flex; flex-direction: column; }
            @media (width>=1012px) { .bar__cF4m1O { flex-direction: row; width: 100%; } }
            .topRow__-dqkhM { display: flex; width: 100%; }
            .LogoSlot__mHAmXE, .menu__2EHTkB { display: flex; }
        "#;
        let doc = parse_html(
            r#"<header class="root__q6pgdo">
                <h2 id="label" class="visuallyHidden__TIbnmb">Navigation Menu</h2>
                <div id="bar" class="bar__cF4m1O">
                  <div id="row" class="topRow__-dqkhM">
                    <div id="logo" class="LogoSlot__mHAmXE">logo</div>
                    <div id="menu" class="menu__2EHTkB">Products</div>
                    <div id="toggle" class="toggleSlot__-ODxSX"><button class="toggle__kPg75-">menu</button></div>
                  </div>
                </div>
              </header>"#,
        );
        let styles = style_document(&doc, css);
        let tree = layout_tree(&doc, &styles, 1280.0, 800.0, &NaiveMetrics, &HashMap::new());
        assert!(
            find(&tree.root, doc.get_element_by_id("toggle").unwrap()).is_none(),
            "the desktop media query should drop the hamburger"
        );
        let label = find(&tree.root, doc.get_element_by_id("label").unwrap()).unwrap();
        assert!(label.rect.width < 8.0, "heading width {}", label.rect.width);
        let row = find(&tree.root, doc.get_element_by_id("row").unwrap()).unwrap();
        let logo = find(&tree.root, doc.get_element_by_id("logo").unwrap()).unwrap();
        let menu = find(&tree.root, doc.get_element_by_id("menu").unwrap()).unwrap();
        assert!(
            menu.rect.x + 1.0 >= logo.rect.x && (logo.rect.y - menu.rect.y).abs() < 4.0,
            "logo and menu should share a row: logo {:?} menu {:?}",
            logo.rect,
            menu.rect
        );
        assert!(row.rect.width > 600.0, "row width {}", row.rect.width);
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
                    let article =
                        find(&tree.root, doc.get_element_by_id("article").unwrap()).unwrap();
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
