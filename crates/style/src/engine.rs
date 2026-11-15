//! Drive Servo's Stylo and Blitz's Taffy/Parley layout from a Mithu document.
//!
//! Each Mithu node is mirrored into a Blitz document. `BaseDocument::resolve`
//! runs Stylo and then Taffy (block, flex, grid, floats) plus Parley.
//! An unchanged document, sheet, and viewport reuses the previous frame so a
//! style read followed by layout does not build that tree twice.

use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex, Once};

use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
use blitz_dom::{Attribute, BaseDocument, DocumentConfig, UserAgentStylesheet};
use blitz_traits::shell::{ColorScheme, Viewport};
use mithu_base::{Color, Edges, Rect};
use mithu_dom::{Document, NodeData, NodeId};
use parley::FontContext;
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use style::invalidation::element::restyle_hints::RestyleHint;
use style::properties::ComputedValues;
use stylo_taffy::CopyStyleGetters;
use style_traits::{CssWriter, ToCss};

use crate::{
    AlignItems, BoxSizing, Clear, ComputedStyle, Display, FlexDirection, FloatSide, Length,
    LineHeight, Position, TextAlign, TextTransform, WhiteSpace,
};

static RESOLVE_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    static FRAME_CACHE: RefCell<Option<(u64, Frame)>> = const { RefCell::new(None) };
    /// The last Blitz document. A later resolve with the same element tree
    /// writes text and attribute changes through `DocumentMutator` and resolves
    /// again, instead of building a second document.
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}

struct Live {
    structure: u64,
    width_bits: u32,
    height_bits: u32,
    fonts: u64,
    images: u64,
    hover: Option<usize>,
    css: String,
    style_text: Option<blitz_dom::NodeId>,
    mirror: Vec<Option<blitz_dom::NodeId>>,
    snapshot: Vec<Vec<(String, String)>>,
    blitz: BaseDocument,
}

thread_local! {
    static REMEMBERED: std::cell::RefCell<(String, Option<NodeId>)> =
        std::cell::RefCell::new((String::new(), None));
}

pub fn remember(author_css: &str, state: crate::StyleState) {
    REMEMBERED.with(|slot| {
        *slot.borrow_mut() = (author_css.to_string(), state.hover);
    });
}

pub fn remembered() -> (String, Option<NodeId>) {
    REMEMBERED.with(|slot| slot.borrow().clone())
}

#[derive(Clone)]
pub struct Frame {
    pub styles: Vec<ComputedStyle>,
    pub placed: Vec<Placed>,
    pub background: Color,
}

#[derive(Clone)]
pub struct Placed {
    pub has_box: bool,
    pub rect: Rect,
    pub margin: Edges,
    pub borders: [f32; 4],
    pub lines: Vec<PlacedLine>,
}

#[derive(Clone)]
pub struct PlacedLine {
    pub baseline: f32,
    pub ascent: f32,
    pub height: f32,
    pub fragments: Vec<PlacedFragment>,
}

#[derive(Clone)]
pub struct PlacedFragment {
    pub x: f32,
    pub text: String,
    pub node: NodeId,
    pub width: f32,
    pub color: Color,
    pub size: f32,
    pub bold: bool,
    pub mono: bool,
    pub family: String,
    pub underline: bool,
}

pub fn resolve(
    doc: &Document,
    author_css: &str,
    state: crate::StyleState,
    viewport_height: f32,
    images: &[(NodeId, u32, u32)],
    fonts: &[(String, Vec<u8>)],
) -> Frame {
    let _guard = RESOLVE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    // Stylo parses `mask` and not the prefixed form. Vector icons set
    // `-webkit-mask-image` (and the shorthand) on `::before`.
    let aliased = author_css
        .contains("-webkit-mask")
        .then(|| author_css.replace("-webkit-mask", "mask"));
    let author_css = aliased.as_deref().unwrap_or(author_css);
    let key = frame_key(doc, author_css, state, viewport_height, images, fonts);
    if let Some(frame) = FRAME_CACHE.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|(cached, _)| *cached == key)
            .map(|(_, frame)| frame.clone())
    }) {
        return frame;
    }
    init_prefs();

    let width = state.viewport_width.max(1.0);
    let height = viewport_height.max(1.0);
    if let Some(frame) = reuse_live(doc, author_css, state, width, height, images, fonts) {
        FRAME_CACHE.with(|slot| *slot.borrow_mut() = Some((key, frame.clone())));
        return frame;
    }
    let mut blitz = BaseDocument::new(DocumentConfig {
        viewport: Some(Viewport {
            color_scheme: ColorScheme::Light,
            window_size: (width.ceil() as u32, height.ceil() as u32),
            hidpi_scale: 1.0,
            zoom: 1.0,
        }),
        base_url: Some("https://mithu.invalid/".to_string()),
        ua_stylesheets: vec![
            UserAgentStylesheet::default_sheet(),
            form_controls_sheet(),
        ],
        font_ctx: font_context(fonts),
        incremental: Some(false),
        ..DocumentConfig::default()
    });

    let mut mirror = vec![None; doc.nodes.len()];
    let mut style_text = None;
    let root = blitz.root_node().id;
    {
        let mut editor = Editor { doc: &mut blitz };
        build_children(doc, doc.root, false, root, &mut editor, &mut mirror);
        if !author_css.is_empty()
            && let Some(parent) = style_parent(&editor, &mirror, doc)
        {
            let mut mutator = editor.doc.mutate();
            let style_id = mutator.create_element(html_name("style"), Vec::new());
            let text_id = mutator.create_text_node(author_css);
            mutator.append_children(style_id, &[text_id]);
            mutator.append_children(parent, &[style_id]);
            style_text = Some(text_id);
        }
    }

    for (id, image_w, image_h) in images {
        let Some(blitz_id) = mirror.get(id.0).copied().flatten() else {
            continue;
        };
        let Some(node) = blitz.get_node_mut(blitz_id) else {
            continue;
        };
        let Some(element) = node.element_data_mut() else {
            continue;
        };
        element.special_data = SpecialElementData::Image(Box::new(ImageData::Raster(
            RasterImageData::new(*image_w, *image_h, Arc::new(Vec::new())),
        )));
    }

    if let Some(hover) = state.hover {
        let mut current = Some(hover);
        while let Some(id) = current {
            if let Some(blitz_id) = mirror.get(id.0).copied().flatten()
                && let Some(node) = blitz.get_node_mut(blitz_id)
            {
                node.hover();
            }
            current = doc.nodes.get(id.0).and_then(|node| node.parent);
        }
    }

    for blitz_id in mirror.iter().flatten().copied() {
        if let Some(node) = blitz.get_node_mut(blitz_id) {
            node.set_restyle_hint(RestyleHint::restyle_subtree());
        }
    }

    blitz.resolve(0.0);
    blitz.set_incremental_layout(true);
    let frame = read_frame(&blitz, &mirror, doc);
    LIVE.with(|slot| {
        *slot.borrow_mut() = Some(Live {
            structure: structure_key(doc),
            width_bits: width.to_bits(),
            height_bits: height.to_bits(),
            fonts: resource_key(fonts),
            images: image_key(images),
            hover: state.hover.map(|id| id.0),
            css: author_css.to_string(),
            style_text,
            mirror,
            snapshot: attr_snapshot(doc),
            blitz,
        });
    });
    FRAME_CACHE.with(|slot| *slot.borrow_mut() = Some((key, frame.clone())));
    frame
}

fn reuse_live(
    doc: &Document,
    author_css: &str,
    state: crate::StyleState,
    width: f32,
    height: f32,
    images: &[(NodeId, u32, u32)],
    fonts: &[(String, Vec<u8>)],
) -> Option<Frame> {
    LIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let live = slot.as_mut()?;
        if live.structure != structure_key(doc)
            || live.width_bits != width.to_bits()
            || live.height_bits != height.to_bits()
            || live.fonts != resource_key(fonts)
            || live.images != image_key(images)
            || live.hover != state.hover.map(|id| id.0)
        {
            return None;
        }
        if author_css != live.css && live.style_text.is_none() {
            return None;
        }
        apply_live_edits(live, doc, author_css);
        live.blitz.resolve(0.0);
        Some(read_frame(&live.blitz, &live.mirror, doc))
    })
}

fn apply_live_edits(live: &mut Live, doc: &Document, css: &str) {
    let mirror = live.mirror.clone();
    let previous = live.snapshot.clone();
    let mut text_updates = Vec::new();
    let mut attr_sets = Vec::new();
    let mut attr_clears = Vec::new();
    let mut next_snapshot = previous.clone();
    for (index, node) in doc.nodes.iter().enumerate() {
        let Some(blitz_id) = mirror.get(index).copied().flatten() else {
            continue;
        };
        match &node.data {
            NodeData::Text(text) => {
                let current = live
                    .blitz
                    .get_node(blitz_id)
                    .map(|node| node.text_content())
                    .unwrap_or_default();
                if current != *text {
                    text_updates.push((blitz_id, text.clone()));
                }
            }
            NodeData::Element(element) => {
                let mut next = element.attrs.clone();
                next.sort();
                let old = previous.get(index).cloned().unwrap_or_default();
                if old != next {
                    for (key, value) in &next {
                        let unchanged = old.iter().any(|(name, current)| name == key && current == value);
                        if !unchanged {
                            attr_sets.push((blitz_id, key.clone(), value.clone()));
                        }
                    }
                    for (key, _) in &old {
                        if !next.iter().any(|(name, _)| name == key) {
                            attr_clears.push((blitz_id, key.clone()));
                        }
                    }
                    if let Some(slot) = next_snapshot.get_mut(index) {
                        *slot = next;
                    }
                }
            }
            NodeData::Document | NodeData::Comment(_) => {}
        }
    }
    let css_changed = css != live.css;
    {
        let mut mutator = live.blitz.mutate();
        for (id, text) in &text_updates {
            mutator.set_node_text(*id, text);
        }
        for (id, key, value) in &attr_sets {
            mutator.set_attribute(*id, attr_name(key), value);
        }
        for (id, key) in &attr_clears {
            mutator.clear_attribute(*id, attr_name(key));
        }
        if css_changed && let Some(id) = live.style_text {
            mutator.set_node_text(id, css);
        }
    }
    live.snapshot = next_snapshot;
    if css_changed {
        live.css = css.to_string();
    }
}

fn read_frame(blitz: &BaseDocument, mirror: &[Option<blitz_dom::NodeId>], doc: &Document) -> Frame {
    let mut styles = vec![ComputedStyle::initial(); doc.nodes.len()];
    let mut placed = (0..doc.nodes.len())
        .map(|_| Placed {
            has_box: false,
            rect: Rect::default(),
            margin: Edges::ZERO,
            borders: [0.0; 4],
            lines: Vec::new(),
        })
        .collect::<Vec<_>>();

    for (index, blitz_id) in mirror.iter().enumerate() {
        let Some(blitz_id) = *blitz_id else {
            continue;
        };
        let Some(node) = blitz.get_node(blitz_id) else {
            continue;
        };
        if let Some(values) = node.primary_styles() {
            styles[index] = project(&values);
        }
        // A mask painted on `::before` (Wikipedia's search icon) has no Mithu
        // node of its own. The element's box is the one the painter fills.
        if styles[index].mask_image.is_none()
            && let Some(before_id) = node.before()
            && let Some(before) = blitz.get_node(before_id)
            && let Some(values) = before.primary_styles()
        {
            let pseudo = project(&values);
            if pseudo.mask_image.is_some() {
                styles[index].mask_image = pseudo.mask_image;
                if styles[index].background.a == 0 {
                    styles[index].background = pseudo.background;
                }
            }
        }
        let Some(element) = node.element_data() else {
            continue;
        };
        if let Some(layout) = element.layout_data.as_ref() {
            let origin = node.absolute_position(0.0, 0.0);
            let box_layout = &layout.final_layout;
            placed[index].has_box = true;
            placed[index].rect = Rect {
                x: origin.x,
                y: origin.y,
                width: box_layout.size.width,
                height: box_layout.size.height,
            };
            placed[index].margin = Edges::new(
                box_layout.margin.top,
                box_layout.margin.right,
                box_layout.margin.bottom,
                box_layout.margin.left,
            );
            placed[index].borders = [
                box_layout.border.top,
                box_layout.border.right,
                box_layout.border.bottom,
                box_layout.border.left,
            ];
        }
        if node.flags.is_inline_root()
            && let Some(inline) = element.inline_layout_data.as_ref()
        {
            placed[index].lines = inline_lines(blitz, mirror, index, node, inline);
        }
    }
    // Mixed block/inline parents wrap stray inlines in anonymous boxes. Those
    // boxes are not Mithu nodes, so their line layout has to be copied onto
    // the parent that owns them.
    let mirrored: HashSet<_> = mirror.iter().flatten().copied().collect();
    for (index, blitz_id) in mirror.iter().enumerate() {
        let Some(blitz_id) = *blitz_id else {
            continue;
        };
        let Some(node) = blitz.get_node(blitz_id) else {
            continue;
        };
        let child_ids: Vec<_> = node
            .layout_children
            .borrow()
            .as_ref()
            .map(|children| children.iter().copied().collect())
            .unwrap_or_default();
        for child_id in child_ids {
            if mirrored.contains(&child_id) {
                continue;
            }
            let Some(child) = blitz.get_node(child_id) else {
                continue;
            };
            if !child.flags.is_inline_root() {
                continue;
            }
            let Some(inline) = child
                .element_data()
                .and_then(|element| element.inline_layout_data.as_ref())
            else {
                continue;
            };
            let extra = inline_lines(blitz, mirror, index, child, inline);
            placed[index].lines.extend(extra);
        }
    }
    paint_fragments(doc, &styles, &mut placed);
    Frame {
        background: page_background(doc, &styles),
        styles,
        placed,
    }
}

fn structure_key(doc: &Document) -> u64 {
    let mut hasher = DefaultHasher::new();
    doc.nodes.len().hash(&mut hasher);
    for node in &doc.nodes {
        node.parent.map(|id| id.0).hash(&mut hasher);
        for child in &node.children {
            child.0.hash(&mut hasher);
        }
        match &node.data {
            NodeData::Document => 0u8.hash(&mut hasher),
            NodeData::Element(element) => {
                1u8.hash(&mut hasher);
                element.name.hash(&mut hasher);
            }
            NodeData::Text(_) => 2u8.hash(&mut hasher),
            NodeData::Comment(_) => 3u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

fn attr_snapshot(doc: &Document) -> Vec<Vec<(String, String)>> {
    doc.nodes
        .iter()
        .map(|node| match &node.data {
            NodeData::Element(element) => {
                let mut attrs = element.attrs.clone();
                attrs.sort();
                attrs
            }
            _ => Vec::new(),
        })
        .collect()
}

fn image_key(images: &[(NodeId, u32, u32)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (id, width, height) in images {
        id.0.hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
    }
    hasher.finish()
}

fn resource_key(fonts: &[(String, Vec<u8>)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (family, bytes) in fonts {
        family.hash(&mut hasher);
        bytes.len().hash(&mut hasher);
        bytes.first().hash(&mut hasher);
        bytes.last().hash(&mut hasher);
    }
    hasher.finish()
}

fn frame_key(
    doc: &Document,
    author_css: &str,
    state: crate::StyleState,
    viewport_height: f32,
    images: &[(NodeId, u32, u32)],
    fonts: &[(String, Vec<u8>)],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    doc.nodes.len().hash(&mut hasher);
    for node in &doc.nodes {
        node.parent.map(|id| id.0).hash(&mut hasher);
        for child in &node.children {
            child.0.hash(&mut hasher);
        }
        match &node.data {
            NodeData::Document => 0u8.hash(&mut hasher),
            NodeData::Element(element) => {
                1u8.hash(&mut hasher);
                element.name.hash(&mut hasher);
                element.id.hash(&mut hasher);
                element.classes.hash(&mut hasher);
                element.attrs.hash(&mut hasher);
            }
            NodeData::Text(text) => {
                2u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            NodeData::Comment(text) => {
                3u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
        }
    }
    author_css.hash(&mut hasher);
    state.hover.map(|id| id.0).hash(&mut hasher);
    state.viewport_width.to_bits().hash(&mut hasher);
    viewport_height.to_bits().hash(&mut hasher);
    for (id, width, height) in images {
        id.0.hash(&mut hasher);
        width.hash(&mut hasher);
        height.hash(&mut hasher);
    }
    for (family, bytes) in fonts {
        family.hash(&mut hasher);
        bytes.len().hash(&mut hasher);
        bytes.first().hash(&mut hasher);
        bytes.last().hash(&mut hasher);
    }
    hasher.finish()
}

fn form_controls_sheet() -> UserAgentStylesheet {
    static SHEET: LazyLock<UserAgentStylesheet> = LazyLock::new(|| {
        UserAgentStylesheet::parse(
            "button, input[type=button], input[type=submit], input[type=reset] { white-space: nowrap; }
             noscript, noembed, noframes { display: none !important; }
             slot { display: contents; }",
        )
    });
    SHEET.clone()
}

fn page_background(doc: &Document, styles: &[ComputedStyle]) -> Color {
    let mut color = Color::WHITE;
    for (index, style) in styles.iter().enumerate() {
        let name = doc.element_name(NodeId(index));
        if matches!(name, Some("html" | "body")) && style.background.a > 0 {
            color = style.background;
        }
    }
    color
}

fn paint_fragments(doc: &Document, styles: &[ComputedStyle], placed: &mut [Placed]) {
    for lines in placed.iter_mut().map(|item| &mut item.lines) {
        for fragment in lines.iter_mut().flat_map(|line| &mut line.fragments) {
            let style = element_style(doc, styles, fragment.node);
            fragment.color = style.color;
            fragment.size = style.font_size;
            fragment.bold = style.bold;
            fragment.mono = style.mono;
            fragment.family = style.font_family.clone();
            fragment.underline = style.underline;
        }
    }
}

fn element_style<'a>(doc: &Document, styles: &'a [ComputedStyle], id: NodeId) -> &'a ComputedStyle {
    let mut current = Some(id);
    while let Some(node) = current {
        if doc.element_name(node).is_some() {
            return &styles[node.0];
        }
        current = doc.nodes.get(node.0).and_then(|item| item.parent);
    }
    &styles[0]
}

fn inline_lines(
    _blitz: &BaseDocument,
    mirror: &[Option<blitz_dom::NodeId>],
    host: usize,
    node: &blitz_dom::Node,
    inline: &blitz_dom::node::TextLayout,
) -> Vec<PlacedLine> {
    let Some(layout_data) = node.element_data().and_then(|element| element.layout_data.as_ref())
    else {
        return Vec::new();
    };
    let origin = node.absolute_position(0.0, 0.0);
    let box_layout = &layout_data.final_layout;
    let scale = inline.layout.scale().max(0.01);
    let content_x = origin.x + box_layout.border.left + box_layout.padding.left;
    let content_y =
        origin.y + box_layout.border.top + box_layout.padding.top + inline.block_offset;
    let text = inline.text.as_str();
    let mut lines = Vec::new();
    for line in inline.layout.lines() {
        let metrics = line.metrics();
        let mut fragments = Vec::new();
        // Glyph runs of one Parley run all report that run's full text range.
        // Walk each run's clusters once, in logical order, and split whenever
        // the style changes so a space between two spans stays a space.
        let items: Vec<_> = line.items().collect();
        let mut seen_runs = HashSet::new();
        for item in &items {
            let parley::PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let range = run.run().text_range();
            if !seen_runs.insert((range.start, range.end)) {
                continue;
            }
            let min_offset = items
                .iter()
                .filter_map(|other| {
                    let parley::PositionedLayoutItem::GlyphRun(other) = other else {
                        return None;
                    };
                    (other.run().text_range() == range).then_some(other.offset())
                })
                .fold(f32::MAX, f32::min);
            let mut cursor = if min_offset.is_finite() {
                min_offset
            } else {
                run.offset()
            };
            let mut group_style: Option<u16> = None;
            let mut group_brush = run.style().brush.id;
            let mut group_text = String::new();
            let mut group_advance = 0.0;
            let mut group_x = cursor;
            let mut flush = |style: &mut Option<u16>,
                             brush: &mut blitz_dom::NodeId,
                             slice: &mut String,
                             advance: &mut f32,
                             at: &mut f32| {
                if slice.is_empty() {
                    return;
                }
                let mithu_id = mithu_for(mirror, *brush).unwrap_or(NodeId(host));
                fragments.push(PlacedFragment {
                    x: content_x + *at / scale,
                    text: std::mem::take(slice),
                    node: mithu_id,
                    width: *advance / scale,
                    color: Color::BLACK,
                    size: 16.0,
                    bold: false,
                    mono: false,
                    family: String::new(),
                    underline: false,
                });
                *style = None;
                *advance = 0.0;
            };
            for cluster in run.run().clusters() {
                let style_index = cluster.style_index();
                if group_style != Some(style_index) {
                    flush(
                        &mut group_style,
                        &mut group_brush,
                        &mut group_text,
                        &mut group_advance,
                        &mut group_x,
                    );
                    group_style = Some(style_index);
                    group_brush = cluster.style().brush.id;
                    group_x = cursor;
                }
                let part = text
                    .get(cluster.text_range())
                    .unwrap_or("")
                    .replace('\u{FFFC}', "");
                if !part.is_empty() {
                    group_text.push_str(&part);
                    group_advance += cluster.advance();
                }
                cursor += cluster.advance();
            }
            flush(
                &mut group_style,
                &mut group_brush,
                &mut group_text,
                &mut group_advance,
                &mut group_x,
            );
        }
        if fragments.is_empty() {
            continue;
        }
        lines.push(PlacedLine {
            baseline: content_y + metrics.baseline / scale,
            ascent: (metrics.baseline - metrics.block_min_coord).max(0.0) / scale,
            height: (metrics.block_max_coord - metrics.block_min_coord).max(1.0) / scale,
            fragments,
        });
    }
    lines
}

fn mithu_for(mirror: &[Option<blitz_dom::NodeId>], blitz_id: blitz_dom::NodeId) -> Option<NodeId> {
    mirror.iter().enumerate().find_map(|(index, id)| {
        (*id == Some(blitz_id)).then_some(NodeId(index))
    })
}

fn project(values: &ComputedValues) -> ComputedStyle {
    let mut style = ComputedStyle::initial();
    style.display = project_display(values);
    style.position = project_position(values);
    style.float = project_float(values);
    style.clear = project_clear(values);
    style.color = project_color(&css_of(&values.clone_color()));
    style.opacity = values.clone_opacity();
    style.visible = !matches!(
        values.clone_visibility(),
        style::computed_values::visibility::T::Hidden
            | style::computed_values::visibility::T::Collapse
    );
    style.z_index = match values.clone_z_index() {
        style::computed_values::z_index::T::Integer(value) => value,
        _ => 0,
    };
    style.text_transform = match css_of_prop(values, "text-transform").as_str() {
        value if value.contains("uppercase") => TextTransform::Uppercase,
        value if value.contains("lowercase") => TextTransform::Lowercase,
        value if value.contains("capitalize") => TextTransform::Capitalize,
        _ => TextTransform::None,
    };
    style.text_align = match values.clone_text_align() {
        style::computed_values::text_align::T::Center
        | style::computed_values::text_align::T::MozCenter => TextAlign::Center,
        style::computed_values::text_align::T::Right
        | style::computed_values::text_align::T::End => TextAlign::Right,
        _ => TextAlign::Left,
    };
    style.white_space = project_white_space(values);
    style.overflow_hidden = !matches!(
        values.clone_overflow_x(),
        style::computed_values::overflow_x::T::Visible
    ) || !matches!(
        values.clone_overflow_y(),
        style::computed_values::overflow_y::T::Visible
    );
    style.box_sizing = match values.clone_box_sizing() {
        style::computed_values::box_sizing::T::BorderBox => BoxSizing::BorderBox,
        _ => BoxSizing::ContentBox,
    };
    style.font_size = font_px(values);
    style.bold = font_weight(values) >= 600.0;
    style.mono = font_is_mono(values);
    style.font_family = font_family(values);
    style.underline = text_underline(values);
    style.background = background_color(values);
    style.background_image = css_image_url(&css_of_prop(values, "background-image"));
    style.mask_image = css_image_url(&css_of_prop(values, "mask-image"));
    style.fill = fill_color(values);
    style.width = length_of(&css_of_prop(values, "width"));
    style.height = length_of(&css_of_prop(values, "height"));
    style.min_width = length_of(&css_of_prop(values, "min-width"));
    style.min_height = length_of(&css_of_prop(values, "min-height"));
    style.max_width = length_of(&css_of_prop(values, "max-width"));
    style.max_height = length_of(&css_of_prop(values, "max-height"));
    style.margin = [
        length_of(&css_of_prop(values, "margin-top")),
        length_of(&css_of_prop(values, "margin-right")),
        length_of(&css_of_prop(values, "margin-bottom")),
        length_of(&css_of_prop(values, "margin-left")),
    ];
    style.padding = [
        length_of(&css_of_prop(values, "padding-top")),
        length_of(&css_of_prop(values, "padding-right")),
        length_of(&css_of_prop(values, "padding-bottom")),
        length_of(&css_of_prop(values, "padding-left")),
    ];
    style.insets = [
        length_of(&css_of_prop(values, "top")),
        length_of(&css_of_prop(values, "right")),
        length_of(&css_of_prop(values, "bottom")),
        length_of(&css_of_prop(values, "left")),
    ];
    style.gap = px_of(&css_of_prop(values, "column-gap"));
    style.row_gap = px_of(&css_of_prop(values, "row-gap"));
    style.flex_wrap = css_of_prop(values, "flex-wrap").contains("wrap");
    style.flex_direction = if css_of_prop(values, "flex-direction").contains("column") {
        FlexDirection::Column
    } else {
        FlexDirection::Row
    };
    style.flex_grow = px_of(&css_of_prop(values, "flex-grow"));
    style.flex_shrink = {
        let value = px_of(&css_of_prop(values, "flex-shrink"));
        if css_of_prop(values, "flex-shrink").is_empty() {
            1.0
        } else {
            value
        }
    };
    style.justify = match css_of_prop(values, "justify-content").as_str() {
        "center" => crate::Justify::Center,
        "flex-end" | "end" => crate::Justify::End,
        "space-between" => crate::Justify::SpaceBetween,
        _ => crate::Justify::Start,
    };
    style.align_items = match css_of_prop(values, "align-items").as_str() {
        "center" => AlignItems::Center,
        "flex-end" | "end" => AlignItems::End,
        "flex-start" | "start" => AlignItems::Start,
        _ => AlignItems::Stretch,
    };
    style.aspect_ratio = aspect_ratio(&css_of_prop(values, "aspect-ratio"));
    style.border_radius = px_of(&css_of_prop(values, "border-top-left-radius"));
    style.line_height = line_height(&css_of_prop(values, "line-height"));
    style
}

fn project_display(values: &ComputedValues) -> Display {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let display = values.clone_display();
    if display.is_none() {
        return Display::None;
    }
    if display.is_contents() {
        return Display::Contents;
    }
    let outside = display.outside();
    let inside = display.inside();
    if inside == DisplayInside::Flex {
        return if outside == DisplayOutside::Inline {
            Display::InlineFlex
        } else {
            Display::Flex
        };
    }
    if inside == DisplayInside::Grid {
        return Display::Grid;
    }
    if display.is_list_item() {
        return Display::ListItem;
    }
    match outside {
        DisplayOutside::Inline => {
            if inside == DisplayInside::Flow {
                Display::Inline
            } else {
                Display::InlineBlock
            }
        }
        _ => Display::Block,
    }
}

fn project_position(values: &ComputedValues) -> Position {
    use style::computed_values::position::T;
    match values.clone_position() {
        T::Relative => Position::Relative,
        T::Absolute => Position::Absolute,
        T::Fixed => Position::Fixed,
        T::Sticky => Position::Sticky,
        _ => Position::Static,
    }
}

fn project_float(values: &ComputedValues) -> FloatSide {
    use style::computed_values::float::T;
    match values.clone_float() {
        T::Left => FloatSide::Left,
        T::Right => FloatSide::Right,
        _ => FloatSide::None,
    }
}

fn project_clear(values: &ComputedValues) -> Clear {
    use style::computed_values::clear::T;
    match values.clone_clear() {
        T::Left => Clear::Left,
        T::Right => Clear::Right,
        T::Both => Clear::Both,
        _ => Clear::None,
    }
}

fn project_white_space(values: &ComputedValues) -> WhiteSpace {
    use style::computed_values::white_space_collapse::T;
    let collapse = values.clone_white_space_collapse();
    let wrap = css_of_prop(values, "text-wrap-mode");
    if wrap.contains("nowrap") {
        return WhiteSpace::Nowrap;
    }
    match collapse {
        T::Preserve | T::PreserveSpaces | T::BreakSpaces => WhiteSpace::Pre,
        _ => WhiteSpace::Normal,
    }
}

fn font_px(values: &ComputedValues) -> f32 {
    let css = css_of_prop(values, "font-size");
    px_of(&css).max(0.0)
}

fn font_weight(values: &ComputedValues) -> f32 {
    let css = css_of_prop(values, "font-weight");
    css.parse::<f32>().unwrap_or(if css == "bold" { 700.0 } else { 400.0 })
}

fn font_is_mono(values: &ComputedValues) -> bool {
    let family = font_family(values).to_ascii_lowercase();
    family.contains("mono") || family.contains("courier") || family.contains("consolas")
}

fn font_family(values: &ComputedValues) -> String {
    let css = css_of_prop(values, "font-family");
    css.split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn text_underline(values: &ComputedValues) -> bool {
    css_of_prop(values, "text-decoration-line").contains("underline")
}

fn background_color(values: &ComputedValues) -> Color {
    project_color(&css_of_prop(values, "background-color"))
}

fn fill_color(values: &ComputedValues) -> Option<Color> {
    let css = css_of_prop(values, "fill");
    if css.is_empty() || css == "none" || css == "currentcolor" {
        return None;
    }
    Some(project_color(&css))
}

fn css_of_prop(values: &ComputedValues, name: &str) -> String {
    let Ok(id) = style::properties::PropertyId::parse_enabled_for_all_content(name) else {
        return String::new();
    };
    let Some(longhand) = id.longhand_id() else {
        return String::new();
    };
    values.computed_value_to_string(style::properties::PropertyDeclarationId::Longhand(longhand))
}

fn css_image_url(value: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let rest = value[start..].trim_start();
    let body = if let Some(rest) = rest.strip_prefix('"') {
        rest.split('"').next()?
    } else if let Some(rest) = rest.strip_prefix('\'') {
        rest.split('\'').next()?
    } else {
        rest.split(')').next()?.trim()
    };
    let url = body.trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

fn css_of(value: &impl ToCss) -> String {
    let mut out = String::new();
    let _ = value.to_css(&mut CssWriter::new(&mut out));
    out
}

fn project_color(css: &str) -> Color {
    let css = css.trim();
    if let Some(hex) = css.strip_prefix('#') {
        return hex_color(hex);
    }
    if let Some(inner) = css
        .strip_prefix("rgba(")
        .or_else(|| css.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts = split_components(inner);
        let r = component(parts.first().map(String::as_str).unwrap_or("0"));
        let g = component(parts.get(1).map(String::as_str).unwrap_or("0"));
        let b = component(parts.get(2).map(String::as_str).unwrap_or("0"));
        let a = parts
            .get(3)
            .map(|part| alpha_byte(part))
            .unwrap_or(255);
        return Color::rgba(r, g, b, a);
    }
    if let Some(inner) = css
        .strip_prefix("color(srgb ")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts = split_components(inner);
        let r = channel(parts.first().map(String::as_str).unwrap_or("0"));
        let g = channel(parts.get(1).map(String::as_str).unwrap_or("0"));
        let b = channel(parts.get(2).map(String::as_str).unwrap_or("0"));
        let a = parts
            .get(3)
            .map(|part| alpha_byte(part.trim_start_matches('/').trim()))
            .unwrap_or(255);
        return Color::rgba(r, g, b, a);
    }
    Color::BLACK
}

fn hex_color(hex: &str) -> Color {
    let hex = hex.trim();
    let byte = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).unwrap_or(0);
    match hex.len() {
        6 => Color::rgb(byte(0), byte(2), byte(4)),
        8 => Color::rgba(byte(0), byte(2), byte(4), byte(6)),
        3 => {
            let nibble = |index: usize| {
                let c = u8::from_str_radix(&hex[index..index + 1], 16).unwrap_or(0);
                c * 16 + c
            };
            Color::rgb(nibble(0), nibble(1), nibble(2))
        }
        _ => Color::BLACK,
    }
}

fn split_components(inner: &str) -> Vec<String> {
    let inner = inner.replace('/', " / ");
    if inner.contains(',') {
        inner
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect()
    } else {
        inner
            .split_whitespace()
            .filter(|part| *part != "/")
            .map(|part| part.to_string())
            .collect()
    }
}

fn component(text: &str) -> u8 {
    let text = text.trim().trim_end_matches('%');
    if text.contains('.') {
        (text.parse::<f32>().unwrap_or(0.0) * 255.0).round().clamp(0.0, 255.0) as u8
    } else {
        text.parse::<f32>().unwrap_or(0.0).round().clamp(0.0, 255.0) as u8
    }
}

fn channel(text: &str) -> u8 {
    let value = text.trim().trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
    if text.contains('%') {
        (value / 100.0 * 255.0).round() as u8
    } else if value <= 1.0 {
        (value * 255.0).round() as u8
    } else {
        value.round() as u8
    }
}

fn alpha_byte(text: &str) -> u8 {
    let text = text.trim();
    if text.ends_with('%') {
        let value = text.trim_end_matches('%').parse::<f32>().unwrap_or(100.0);
        (value / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8
    } else {
        let value = text.parse::<f32>().unwrap_or(1.0);
        if value <= 1.0 {
            (value * 255.0).round().clamp(0.0, 255.0) as u8
        } else {
            value.round().clamp(0.0, 255.0) as u8
        }
    }
}

fn length_of(css: &str) -> Length {
    let css = css.trim();
    if css.is_empty() || css == "auto" || css == "none" || css == "normal" {
        return Length::Auto;
    }
    if let Some(number) = css.strip_suffix('%') {
        return Length::Percent(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(number) = css.strip_suffix("px") {
        return Length::Px(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(number) = css.strip_suffix("rem") {
        return Length::Rem(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(number) = css.strip_suffix("em") {
        return Length::Em(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(number) = css.strip_suffix("vw") {
        return Length::Vw(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(number) = css.strip_suffix("vh") {
        return Length::Vh(number.trim().parse().unwrap_or(0.0));
    }
    if let Some(inner) = css
        .strip_prefix("calc(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return parse_calc(inner);
    }
    Length::Auto
}

fn parse_calc(inner: &str) -> Length {
    let mut percent = 0.0;
    let mut px = 0.0;
    let mut em = 0.0;
    let mut rem = 0.0;
    let mut vw = 0.0;
    let mut vh = 0.0;
    let mut sign = 1.0;
    let mut number = String::new();
    let mut chars = inner.chars().peekable();
    let finish = |number: &str, sign: f32, percent: &mut f32, px: &mut f32, em: &mut f32, rem: &mut f32, vw: &mut f32, vh: &mut f32| {
        let number = number.trim();
        if number.is_empty() {
            return;
        }
        let (value, unit) = split_unit(number);
        let value = sign * value;
        match unit {
            "%" => *percent += value,
            "em" => *em += value,
            "rem" => *rem += value,
            "vw" => *vw += value,
            "vh" => *vh += value,
            _ => *px += value,
        }
    };
    while let Some(ch) = chars.next() {
        match ch {
            '+' => {
                finish(&number, sign, &mut percent, &mut px, &mut em, &mut rem, &mut vw, &mut vh);
                number.clear();
                sign = 1.0;
            }
            '-' => {
                finish(&number, sign, &mut percent, &mut px, &mut em, &mut rem, &mut vw, &mut vh);
                number.clear();
                sign = -1.0;
            }
            _ => number.push(ch),
        }
    }
    finish(&number, sign, &mut percent, &mut px, &mut em, &mut rem, &mut vw, &mut vh);
    if em == 0.0 && rem == 0.0 && vw == 0.0 && vh == 0.0 && percent == 0.0 {
        Length::Px(px)
    } else if px == 0.0 && em == 0.0 && rem == 0.0 && vw == 0.0 && vh == 0.0 {
        Length::Percent(percent)
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

fn split_unit(token: &str) -> (f32, &str) {
    let token = token.trim();
    for unit in ["rem", "em", "vw", "vh", "px", "%"] {
        if let Some(number) = token.strip_suffix(unit) {
            return (number.trim().parse().unwrap_or(0.0), unit);
        }
    }
    (token.parse().unwrap_or(0.0), "px")
}

fn px_of(css: &str) -> f32 {
    match length_of(css) {
        Length::Px(value) | Length::Percent(value) | Length::Em(value) | Length::Rem(value) => value,
        _ => css
            .split_whitespace()
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or(0.0),
    }
}

fn aspect_ratio(css: &str) -> Option<(f32, f32)> {
    let css = css.trim();
    if css.is_empty() || css == "auto" {
        return None;
    }
    let mut parts = css.split(['/', ' ']).filter(|part| !part.is_empty());
    let width = parts.next()?.parse().ok()?;
    let height = parts.next().and_then(|part| part.parse().ok()).unwrap_or(1.0);
    Some((width, height))
}

fn line_height(css: &str) -> LineHeight {
    match length_of(css) {
        Length::Auto => LineHeight::Normal,
        Length::Px(value) => LineHeight::Px(value),
        Length::Percent(value) => LineHeight::Percent(value),
        Length::Em(value) | Length::Rem(value) => LineHeight::Number(value),
        _ => LineHeight::Normal,
    }
}

fn font_context(fonts: &[(String, Vec<u8>)]) -> Option<FontContext> {
    if fonts.is_empty() {
        return None;
    }
    let mut context = FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: true,
        }),
    };
    for (family, bytes) in fonts {
        let override_info = parley::fontique::FontInfoOverride {
            family_name: Some(family.as_str()),
            ..parley::fontique::FontInfoOverride::default()
        };
        context
            .collection
            .register_fonts(Blob::new(Arc::new(bytes.clone())), Some(override_info));
    }
    Some(context)
}

fn init_prefs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        style_config::set_pref!("layout.css.flexbox-balance.enabled", true);
        style_config::set_pref!("layout.css.background-clip.text.enabled", true);
        style_config::set_pref!("layout.css.webkit-text-fill-color.enabled", true);
        style_config::set_pref!("layout.css.columns.enabled", true);
        style_config::set_pref!("layout.css.font-variations.enabled", true);
        style_config::set_pref!("layout.css.word-break.break-word.enabled", true);
        style_config::set_pref!("layout.css.writing-mode.enabled", true);
    });
}

struct Editor<'a> {
    doc: &'a mut BaseDocument,
}

impl<'a> Editor<'a> {
    fn doc_root(&self) -> blitz_dom::NodeId {
        self.doc.root_node().id
    }
}

fn build_children(
    doc: &Document,
    parent: NodeId,
    parent_svg: bool,
    blitz_parent: blitz_dom::NodeId,
    editor: &mut Editor<'_>,
    mirror: &mut [Option<blitz_dom::NodeId>],
) {
    let children = doc.rendered_children(parent);
    let mut ids = Vec::new();
    for child in children {
        let Some(node) = doc.nodes.get(child.0) else {
            continue;
        };
        match &node.data {
            NodeData::Element(element) => {
                if element.name == "mithu-shadow" {
                    continue;
                }
                let svg = parent_svg || element.name == "svg";
                let name = if svg && element.name != "svg" && !svg_tag(&element.name) && parent_svg {
                    html_name(&element.name)
                } else if svg {
                    svg_name(&element.name)
                } else {
                    html_name(&element.name)
                };
                let attrs = element
                    .attrs
                    .iter()
                    .map(|(key, value)| Attribute {
                        name: attr_name(key),
                        value: value.clone(),
                    })
                    .collect();
                let id = {
                    let mut mutator = editor.doc.mutate();
                    let id = mutator.create_element(name, attrs);
                    mutator.append_children(blitz_parent, &[id]);
                    id
                };
                mirror[child.0] = Some(id);
                ids.push(id);
                build_children(doc, child, svg || element.name == "svg", id, editor, mirror);
            }
            NodeData::Text(text) => {
                let mut mutator = editor.doc.mutate();
                let id = mutator.create_text_node(text);
                mutator.append_children(blitz_parent, &[id]);
                mirror[child.0] = Some(id);
                ids.push(id);
            }
            NodeData::Comment(_) | NodeData::Document => {}
        }
    }
    let _ = ids;
}

fn style_parent(
    editor: &Editor<'_>,
    mirror: &[Option<blitz_dom::NodeId>],
    doc: &Document,
) -> Option<blitz_dom::NodeId> {
    for (index, node) in doc.nodes.iter().enumerate() {
        if doc.element_name(NodeId(index)) == Some("head") {
            return mirror[index];
        }
        let _ = node;
    }
    for (index, _) in doc.nodes.iter().enumerate() {
        if doc.element_name(NodeId(index)) == Some("html") {
            return mirror[index];
        }
    }
    Some(editor.doc_root())
}

fn html_name(name: &str) -> markup5ever::QualName {
    markup5ever::QualName::new(None, markup5ever::ns!(html), markup5ever::LocalName::from(name))
}

fn svg_name(name: &str) -> markup5ever::QualName {
    markup5ever::QualName::new(None, markup5ever::ns!(svg), markup5ever::LocalName::from(name))
}

fn attr_name(name: &str) -> markup5ever::QualName {
    markup5ever::QualName::new(None, markup5ever::ns!(), markup5ever::LocalName::from(name))
}

fn svg_tag(name: &str) -> bool {
    matches!(
        name,
        "svg" | "path" | "use" | "defs" | "g" | "circle" | "rect" | "line" | "polyline"
            | "polygon" | "ellipse" | "text" | "tspan" | "symbol" | "clippath" | "mask"
            | "lineargradient" | "radialgradient" | "stop" | "image"
    )
}

// Silence the unused import if the editor wrapper is the only BaseDocument user.
#[allow(dead_code)]
fn _touch(doc: &BaseDocument) {
    let _ = doc.id();
}
