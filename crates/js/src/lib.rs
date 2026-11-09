//! In-process JavaScript virtual machine.
//!
//! Scripts run inside the browser process. Boa is the ECMAScript machine.
//! Host objects mutate the Mithu document, and layout runs after they return.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, Finalize, JsData, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    Source, Trace, js_string,
};
use boa_engine::object::builtins::JsArray;
use mithu_css::{parse_css, parse_declarations, Declaration};
use mithu_dom::{Document, NodeData, NodeId};
use mithu_html::parse_html;
use mithu_style::matches_element;

const ATTR: Attribute = Attribute::WRITABLE.union(Attribute::ENUMERABLE).union(Attribute::CONFIGURABLE);

#[derive(Clone)]
struct Host {
    doc: Rc<RefCell<Document>>,
    log: Rc<RefCell<Vec<String>>>,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct NodeKey {
    #[unsafe_ignore_trace]
    id: usize,
}

pub fn engine_name() -> &'static str {
    "boa"
}

pub fn run_scripts(doc: &mut Document, mut fetch: impl FnMut(&str) -> Option<String>) -> Vec<String> {
    if doc.scripts.is_empty() {
        return Vec::new();
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::new(RefCell::new(std::mem::take(doc)));
    let mut context = Context::default();
    context.insert_data(Host {
        doc: Rc::clone(&shared),
        log: Rc::clone(&log),
    });
    if let Err(error) = install_host(&mut context) {
        log.borrow_mut().push(format!("JavaScript error: {error}"));
    } else {
        let scripts = shared.borrow().scripts.clone();
        for script in scripts {
            if script.module {
                log.borrow_mut().push("JavaScript error: modules are not run".to_string());
                continue;
            }
            let code = if let Some(src) = &script.src {
                fetch(src).unwrap_or_default()
            } else {
                script.code.clone()
            };
            if code.trim().is_empty() {
                continue;
            }
            if let Err(error) = context.eval(Source::from_bytes(code.as_str())) {
                log.borrow_mut().push(format!("JavaScript error: {error}"));
            }
        }
    }
    *doc = shared.borrow().clone();
    log.borrow().clone()
}

fn install_host(context: &mut Context) -> JsResult<()> {
    let document = document_object(context)?;
    let console = console_object(context)?;
    let flags = ATTR;
    context.register_global_property(js_string!("document"), document, flags)?;
    context.register_global_property(js_string!("console"), console, flags)?;
    Ok(())
}

fn console_object(context: &mut Context) -> JsResult<JsObject> {
    let mut object = ObjectInitializer::new(context);
    object.function(NativeFunction::from_fn_ptr(console_log), js_string!("log"), 1);
    Ok(object.build())
}

fn document_object(context: &mut Context) -> JsResult<JsObject> {
    let title_get = native(context, "get title", 0, get_title);
    let title_set = native(context, "set title", 1, set_title);
    let body_get = native(context, "get body", 0, get_body);
    let mut object = ObjectInitializer::new(context);
    object
        .function(NativeFunction::from_fn_ptr(get_element_by_id), js_string!("getElementById"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector), js_string!("querySelector"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector_all), js_string!("querySelectorAll"), 1)
        .function(NativeFunction::from_fn_ptr(create_element), js_string!("createElement"), 1)
        .function(NativeFunction::from_fn_ptr(get_elements_by_tag_name), js_string!("getElementsByTagName"), 1)
        .accessor(js_string!("title"), Some(title_get), Some(title_set), ATTR)
        .accessor(js_string!("body"), Some(body_get), None, ATTR);
    Ok(object.build())
}

fn native(
    context: &mut Context,
    name: &str,
    length: usize,
    body: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
) -> boa_engine::object::builtins::JsFunction {
    FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(body))
        .name(JsString::from(name))
        .length(length)
        .constructor(false)
        .build()
}

fn console_log(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let mut parts = Vec::new();
    for arg in args {
        parts.push(arg.to_string(context)?.to_std_string_lossy());
    }
    host.log.borrow_mut().push(parts.join(" "));
    Ok(JsValue::undefined())
}

fn get_title(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(js_string!(host.doc.borrow().title.clone()).into())
}

fn set_title(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let value = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    doc.title = value.clone();
    if let Some(title) = find_tag(&doc, "title") {
        doc.set_text_content(title, &value);
    }
    Ok(JsValue::undefined())
}

fn get_body(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = find_tag(&host.doc.borrow(), "body");
    match id {
        Some(id) => Ok(element_value(id.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn get_element_by_id(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = arg_string(args, 0, context)?;
    let found = host.doc.borrow().get_element_by_id(&id);
    match found {
        Some(node) => Ok(element_value(node.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn query_selector(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let scope = node_of(this).unwrap_or(NodeId(0));
    let found = {
        let doc = host.doc.borrow();
        query_first(&doc, scope, &selector, node_of(this).is_some())
    };
    match found {
        Some(node) => Ok(element_value(node.0, context)),
        None => Ok(JsValue::null()),
    }
}

fn query_selector_all(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let selector = arg_string(args, 0, context)?;
    let scope = node_of(this).unwrap_or(NodeId(0));
    let ids = {
        let doc = host.doc.borrow();
        query_all(&doc, scope, &selector, node_of(this).is_some())
    };
    let values = ids.into_iter().map(|id| element_value(id.0, context)).collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn get_elements_by_tag_name(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let name = arg_string(args, 0, context)?.to_ascii_lowercase();
    let ids = {
        let doc = host.doc.borrow();
        (0..doc.nodes.len())
            .map(NodeId)
            .filter(|id| doc.element_name(*id) == Some(name.as_str()))
            .collect::<Vec<_>>()
    };
    let values = ids.into_iter().map(|id| element_value(id.0, context)).collect::<Vec<_>>();
    Ok(JsObject::from(JsArray::from_iter(values, context)).into())
}

fn create_element(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let name = arg_string(args, 0, context)?;
    let id = host.doc.borrow_mut().create_element_detached(&name);
    Ok(element_value(id.0, context))
}

fn element_value(id: usize, context: &mut Context) -> JsValue {
    let text_get = native(context, "get textContent", 0, get_text);
    let text_set = native(context, "set textContent", 1, set_text);
    let inner_get = native(context, "get innerText", 0, get_text);
    let inner_set = native(context, "set innerText", 1, set_text);
    let html_get = native(context, "get innerHTML", 0, get_html);
    let html_set = native(context, "set innerHTML", 1, set_html);
    let id_get = native(context, "get id", 0, get_id);
    let id_set = native(context, "set id", 1, set_id);
    let class_get = native(context, "get className", 0, get_class);
    let class_set = native(context, "set className", 1, set_class);
    let style = style_object(id, context);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .function(NativeFunction::from_fn_ptr(get_attribute), js_string!("getAttribute"), 1)
        .function(NativeFunction::from_fn_ptr(set_attribute), js_string!("setAttribute"), 2)
        .function(NativeFunction::from_fn_ptr(query_selector), js_string!("querySelector"), 1)
        .function(NativeFunction::from_fn_ptr(query_selector_all), js_string!("querySelectorAll"), 1)
        .function(NativeFunction::from_fn_ptr(append_child), js_string!("appendChild"), 1)
        .accessor(js_string!("textContent"), Some(text_get), Some(text_set), ATTR)
        .accessor(js_string!("innerText"), Some(inner_get), Some(inner_set), ATTR)
        .accessor(js_string!("innerHTML"), Some(html_get), Some(html_set), ATTR)
        .accessor(js_string!("id"), Some(id_get), Some(id_set), ATTR)
        .accessor(js_string!("className"), Some(class_get), Some(class_set), ATTR)
        .property(js_string!("style"), style, ATTR);
    object.build().into()
}

fn style_object(id: usize, context: &mut Context) -> JsObject {
    let css_get = native(context, "get cssText", 0, get_css_text);
    let css_set = native(context, "set cssText", 1, set_css_text);
    let color_get = native(context, "get color", 0, get_style_color);
    let color_set = native(context, "set color", 1, set_style_color);
    let background_get = native(context, "get backgroundColor", 0, get_style_background);
    let background_set = native(context, "set backgroundColor", 1, set_style_background);
    let display_get = native(context, "get display", 0, get_style_display);
    let display_set = native(context, "set display", 1, set_style_display);
    let width_get = native(context, "get width", 0, get_style_width);
    let width_set = native(context, "set width", 1, set_style_width);
    let height_get = native(context, "get height", 0, get_style_height);
    let height_set = native(context, "set height", 1, set_style_height);
    let font_get = native(context, "get fontSize", 0, get_style_font_size);
    let font_set = native(context, "set fontSize", 1, set_style_font_size);
    let margin_get = native(context, "get margin", 0, get_style_margin);
    let margin_set = native(context, "set margin", 1, set_style_margin);
    let padding_get = native(context, "get padding", 0, get_style_padding);
    let padding_set = native(context, "set padding", 1, set_style_padding);
    let align_get = native(context, "get textAlign", 0, get_style_align);
    let align_set = native(context, "set textAlign", 1, set_style_align);
    let position_get = native(context, "get position", 0, get_style_position);
    let position_set = native(context, "set position", 1, set_style_position);
    let top_get = native(context, "get top", 0, get_style_top);
    let top_set = native(context, "set top", 1, set_style_top);
    let left_get = native(context, "get left", 0, get_style_left);
    let left_set = native(context, "set left", 1, set_style_left);
    let opacity_get = native(context, "get opacity", 0, get_style_opacity);
    let opacity_set = native(context, "set opacity", 1, set_style_opacity);
    let flex_get = native(context, "get flexDirection", 0, get_style_flex_direction);
    let flex_set = native(context, "set flexDirection", 1, set_style_flex_direction);
    let gap_get = native(context, "get gap", 0, get_style_gap);
    let gap_set = native(context, "set gap", 1, set_style_gap);
    let mut object = ObjectInitializer::with_native_data(NodeKey { id }, context);
    object
        .accessor(js_string!("cssText"), Some(css_get), Some(css_set), ATTR)
        .function(NativeFunction::from_fn_ptr(set_property), js_string!("setProperty"), 2)
        .accessor(js_string!("color"), Some(color_get), Some(color_set), ATTR)
        .accessor(js_string!("backgroundColor"), Some(background_get), Some(background_set), ATTR)
        .accessor(js_string!("display"), Some(display_get), Some(display_set), ATTR)
        .accessor(js_string!("width"), Some(width_get), Some(width_set), ATTR)
        .accessor(js_string!("height"), Some(height_get), Some(height_set), ATTR)
        .accessor(js_string!("fontSize"), Some(font_get), Some(font_set), ATTR)
        .accessor(js_string!("margin"), Some(margin_get), Some(margin_set), ATTR)
        .accessor(js_string!("padding"), Some(padding_get), Some(padding_set), ATTR)
        .accessor(js_string!("textAlign"), Some(align_get), Some(align_set), ATTR)
        .accessor(js_string!("position"), Some(position_get), Some(position_set), ATTR)
        .accessor(js_string!("top"), Some(top_get), Some(top_set), ATTR)
        .accessor(js_string!("left"), Some(left_get), Some(left_set), ATTR)
        .accessor(js_string!("opacity"), Some(opacity_get), Some(opacity_set), ATTR)
        .accessor(js_string!("flexDirection"), Some(flex_get), Some(flex_set), ATTR)
        .accessor(js_string!("gap"), Some(gap_get), Some(gap_set), ATTR);
    object.build()
}

macro_rules! style_fns {
    ($get:ident, $set:ident, $css:literal) => {
        fn $get(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            read_style(this, $css, context)
        }
        fn $set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            write_style(this, $css, args, context)
        }
    };
}

style_fns!(get_style_color, set_style_color, "color");
style_fns!(get_style_background, set_style_background, "background-color");
style_fns!(get_style_display, set_style_display, "display");
style_fns!(get_style_width, set_style_width, "width");
style_fns!(get_style_height, set_style_height, "height");
style_fns!(get_style_font_size, set_style_font_size, "font-size");
style_fns!(get_style_margin, set_style_margin, "margin");
style_fns!(get_style_padding, set_style_padding, "padding");
style_fns!(get_style_align, set_style_align, "text-align");
style_fns!(get_style_position, set_style_position, "position");
style_fns!(get_style_top, set_style_top, "top");
style_fns!(get_style_left, set_style_left, "left");
style_fns!(get_style_opacity, set_style_opacity, "opacity");
style_fns!(get_style_flex_direction, set_style_flex_direction, "flex-direction");
style_fns!(get_style_gap, set_style_gap, "gap");

fn get_css_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let text = host.doc.borrow().attr(id, "style").unwrap_or("").to_string();
    Ok(js_string!(text).into())
}

fn set_css_text(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "style", &value);
    Ok(JsValue::undefined())
}

fn set_property(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    write_style(this, &name, &args[1..], context)
}

fn read_style(this: &JsValue, name: &str, context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let text = host.doc.borrow().attr(id, "style").unwrap_or("").to_string();
    let value = parse_declarations(&text)
        .into_iter()
        .rev()
        .find(|declaration| declaration.name == name)
        .map(|declaration| declaration.value)
        .unwrap_or_default();
    Ok(js_string!(value).into())
}

fn write_style(this: &JsValue, name: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    let current = doc.attr(id, "style").unwrap_or("").to_string();
    let mut declarations = parse_declarations(&current);
    if let Some(existing) = declarations.iter_mut().find(|declaration| declaration.name == name) {
        existing.value = value;
        existing.important = false;
    } else {
        declarations.push(Declaration {
            name: name.to_string(),
            value,
            important: false,
        });
    }
    let text = declarations
        .iter()
        .map(|declaration| format!("{}: {}", declaration.name, declaration.value))
        .collect::<Vec<_>>()
        .join("; ");
    doc.set_attr(id, "style", &text);
    Ok(JsValue::undefined())
}

fn get_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().text_content(id)).into())
}

fn set_text(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_text_content(id, &value);
    Ok(JsValue::undefined())
}

fn get_html(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(serialize(&host.doc.borrow(), id)).into())
}

fn set_html(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let html = arg_string(args, 0, context)?;
    let mut doc = host.doc.borrow_mut();
    doc.clear_children(id);
    if html.contains('<') {
        let parsed = parse_html(&format!("<div id=\"mithu-root\">{html}</div>"));
        if let Some(root) = parsed.get_element_by_id("mithu-root") {
            copy_children(&parsed, root, &mut doc, id);
        }
    } else if !html.is_empty() {
        doc.append_text(id, &html);
    }
    Ok(JsValue::undefined())
}

fn get_id(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, "id").unwrap_or("").to_string()).into())
}

fn set_id(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "id", &value);
    Ok(JsValue::undefined())
}

fn get_class(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    Ok(js_string!(host.doc.borrow().attr(id, "class").unwrap_or("").to_string()).into())
}

fn set_class(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let value = arg_string(args, 0, context)?;
    host.doc.borrow_mut().set_attr(id, "class", &value);
    Ok(JsValue::undefined())
}

fn get_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    match host.doc.borrow().attr(id, &name) {
        Some(value) => Ok(js_string!(value.to_string()).into()),
        None => Ok(JsValue::null()),
    }
}

fn set_attribute(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = node_of(this).ok_or_else(type_error)?;
    let name = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    host.doc.borrow_mut().set_attr(id, &name, &value);
    Ok(JsValue::undefined())
}

fn append_child(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let parent = node_of(this).ok_or_else(type_error)?;
    let child_value = args.first().cloned().unwrap_or(JsValue::undefined());
    let child = node_of(&child_value).ok_or_else(type_error)?;
    host.doc.borrow_mut().append_child(parent, child);
    Ok(child_value)
}

fn host(context: &mut Context) -> JsResult<Host> {
    context.get_data::<Host>().cloned().ok_or_else(type_error)
}

fn type_error() -> JsError {
    JsNativeError::typ().with_message("DOM object is missing").into()
}

fn arg_string(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<String> {
    let value = args.get(index).cloned().unwrap_or(JsValue::undefined());
    Ok(value.to_string(context)?.to_std_string_lossy())
}

fn node_of(value: &JsValue) -> Option<NodeId> {
    let object = value.as_object()?;
    let key = object.downcast_ref::<NodeKey>()?;
    Some(NodeId(key.id))
}

fn find_tag(doc: &Document, name: &str) -> Option<NodeId> {
    (0..doc.nodes.len()).map(NodeId).find(|id| doc.element_name(*id) == Some(name))
}

fn query_first(doc: &Document, scope: NodeId, selector: &str, descendants_only: bool) -> Option<NodeId> {
    query_all(doc, scope, selector, descendants_only).into_iter().next()
}

fn query_all(doc: &Document, scope: NodeId, selector: &str, descendants_only: bool) -> Vec<NodeId> {
    let sheet = parse_css(&format!("{selector} {{ color: black }}"));
    let Some(parsed) = sheet.rules.first().and_then(|rule| rule.selectors.first()) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    if !descendants_only && doc.element_name(scope).is_some() {
        nodes.push(scope);
    }
    collect_elements(doc, scope, &mut nodes);
    nodes.into_iter().filter(|id| matches_element(doc, *id, parsed)).collect()
}

fn collect_elements(doc: &Document, id: NodeId, out: &mut Vec<NodeId>) {
    let Some(node) = doc.nodes.get(id.0) else {
        return;
    };
    for child in node.children.clone() {
        if doc.element_name(child).is_some() {
            out.push(child);
            collect_elements(doc, child, out);
        }
    }
}

fn copy_children(src: &Document, src_id: NodeId, dst: &mut Document, dst_parent: NodeId) {
    for child in src.node(src_id).children.clone() {
        match &src.node(child).data {
            NodeData::Text(text) => {
                dst.append_text(dst_parent, text);
            }
            NodeData::Element(element) => {
                let id = dst.create_element(dst_parent, &element.name);
                for (name, value) in &element.attrs {
                    dst.set_attr(id, name, value);
                }
                copy_children(src, child, dst, id);
            }
            NodeData::Comment(_) | NodeData::Document => {}
        }
    }
}

fn serialize(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    for child in &doc.node(id).children {
        write_html(doc, *child, &mut out);
    }
    out
}

fn write_html(doc: &Document, id: NodeId, out: &mut String) {
    match &doc.node(id).data {
        NodeData::Text(text) => out.push_str(&escape(text)),
        NodeData::Element(element) => {
            out.push('<');
            out.push_str(&element.name);
            for (name, value) in &element.attrs {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape(value));
                out.push('"');
            }
            out.push('>');
            for child in &doc.node(id).children {
                write_html(doc, *child, out);
            }
            out.push_str("</");
            out.push_str(&element.name);
            out.push('>');
        }
        NodeData::Comment(text) => {
            out.push_str("<!--");
            out.push_str(text);
            out.push_str("-->");
        }
        NodeData::Document => {}
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithu_html::parse_html;

    #[test]
    fn script_updates_text_and_style() {
        let mut doc = parse_html(
            r##"<p id="msg">before</p><div id="host"></div>
            <script>
              document.getElementById("msg").textContent = "after";
              document.title = "Changed";
              var el = document.querySelector("#host");
              el.className = "on";
              el.style.color = "navy";
              el.style.display = "flex";
              console.log("ready");
            </script>"##,
        );
        let log = run_scripts(&mut doc, |_| None);
        assert_eq!(doc.get_element_by_id("msg").map(|id| doc.text_content(id)).as_deref(), Some("after"));
        assert_eq!(doc.title, "Changed");
        let host_node = doc.get_element_by_id("host").unwrap();
        assert!(doc.has_class(host_node, "on"));
        let style = doc.attr(host_node, "style").unwrap_or("");
        assert!(style.contains("color: navy"), "{style}");
        assert!(style.contains("display: flex"), "{style}");
        assert_eq!(log, vec!["ready".to_string()]);
    }

    #[test]
    fn inner_html_builds_elements() {
        let mut doc = parse_html(r##"<div id="host"></div><script>
            document.getElementById("host").innerHTML = "<b id='yes'>Hi</b>";
        </script>"##);
        run_scripts(&mut doc, |_| None);
        let yes = doc.get_element_by_id("yes").unwrap();
        assert_eq!(doc.text_content(yes), "Hi");
        assert_eq!(doc.element_name(yes), Some("b"));
    }
}
