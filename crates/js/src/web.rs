//! Host classes a page script expects to exist: `URL`, `TextEncoder`,
//! `XMLHttpRequest`, `MutationObserver`, and `history`.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, Finalize, JsData, JsNativeError, JsObject, JsResult, JsString, JsValue,
    NativeFunction, Trace, js_string,
};

use crate::{arg_string, cors_allows, cross_origin, host, native, resolve_request};

const ATTR: Attribute = Attribute::WRITABLE
    .union(Attribute::ENUMERABLE)
    .union(Attribute::CONFIGURABLE);

pub(crate) fn register(context: &mut Context) -> JsResult<()> {
    context.register_global_class::<PageUrl>()?;
    context.register_global_class::<SearchParams>()?;
    context.register_global_class::<TextEncoderHost>()?;
    context.register_global_class::<TextDecoderHost>()?;
    context.register_global_class::<Xhr>()?;
    context.register_global_class::<MutationObserverHost>()?;
    context.register_global_class::<HtmlImage>()?;
    context.register_global_class::<EventTargetHost>()?;
    context.register_global_class::<NodeHost>()?;
    context.register_global_class::<ElementHost>()?;
    context.register_global_class::<ShadowRootHost>()?;
    context.register_global_class::<DocumentFragmentHost>()?;
    context.register_global_class::<HtmlElementHost>()?;
    context.register_global_class::<DocumentHost>()?;
    context.register_global_class::<DomEvent>()?;
    context.register_global_class::<CustomEventHost>()?;
    context.register_global_class::<MessageChannelHost>()?;
    context.register_global_class::<MessagePortHost>()?;
    context.register_global_class::<AbortSignalHost>()?;
    context.register_global_class::<AbortControllerHost>()?;
    context.register_global_class::<HtmlTemplateHost>()?;
    context.register_global_class::<FontFaceHost>()?;
    context.register_global_class::<RangeHost>()?;
    context.register_global_class::<IntersectionObserverHost>()?;
    let style_sheet = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(crate::construct_style_sheet),
    )
    .name(js_string!("CSSStyleSheet"))
    .length(0)
    .constructor(true)
    .build();
    context.register_global_property(
        js_string!("CSSStyleSheet"),
        style_sheet,
        Attribute::WRITABLE.union(Attribute::CONFIGURABLE),
    )?;
    context.register_global_class::<ResizeObserverHost>()?;
    register_html_elements(context)?;
    let image = context.global_object().get(js_string!("Image"), context)?;
    context.register_global_property(js_string!("HTMLImageElement"), image, Attribute::all())?;
    for name in [
        "MouseEvent",
        "WheelEvent",
        "KeyboardEvent",
        "FocusEvent",
        "PointerEvent",
        "InputEvent",
        "UIEvent",
        "TouchEvent",
        "ErrorEvent",
    ] {
        let constructor = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_fn_ptr(construct_event),
        )
        .name(JsString::from(name))
        .length(1)
        .constructor(true)
        .build();
        context.register_global_property(
            js_string!(name),
            constructor,
            Attribute::WRITABLE.union(Attribute::CONFIGURABLE),
        )?;
    }
    for name in [
        "CharacterData",
        "Comment",
        "CDATASection",
        "Text",
        "ProcessingInstruction",
        "Window",
        "SVGElement",
        "CustomElementRegistry",
        "HTMLUnknownElement",
        "NodeFilter",
        "TreeWalker",
    ] {
        let prototype = ObjectInitializer::new(context).build();
        let constructor = ObjectInitializer::new(context)
            .property(js_string!("prototype"), prototype, ATTR)
            .build();
        if name == "NodeFilter" {
            constructor.set(js_string!("SHOW_ALL"), JsValue::from(-1), false, context)?;
            constructor.set(js_string!("SHOW_ELEMENT"), JsValue::from(1), false, context)?;
            constructor.set(js_string!("SHOW_TEXT"), JsValue::from(4), false, context)?;
            constructor.set(
                js_string!("SHOW_COMMENT"),
                JsValue::from(128),
                false,
                context,
            )?;
        }
        context.register_global_property(js_string!(name), constructor, Attribute::all())?;
    }
    let crypto = crypto_object(context)?;
    context.register_global_property(js_string!("crypto"), crypto, Attribute::all())?;
    let observers = JsArray::from_iter([], context);
    context.register_global_property(
        js_string!("__mithuObservers"),
        observers,
        Attribute::all(),
    )?;
    let ports = ObjectInitializer::new(context).build();
    context.register_global_property(js_string!("__mithuPorts"), ports, Attribute::all())?;
    let history = history_object(context)?;
    context.register_global_property(js_string!("history"), history, Attribute::all())?;
    context.register_global_callable(js_string!("btoa"), 1, NativeFunction::from_fn_ptr(btoa))?;
    context.register_global_callable(js_string!("atob"), 1, NativeFunction::from_fn_ptr(atob))?;
    Ok(())
}

#[derive(Debug, Trace, Finalize, JsData)]
struct PageUrl {
    #[unsafe_ignore_trace]
    url: mithu_url::Url,
}

impl Class for PageUrl {
    const NAME: &'static str = "URL";
    const LENGTH: usize = 1;

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(url_to_string),
        );
        class.static_method(
            js_string!("canParse"),
            1,
            NativeFunction::from_fn_ptr(url_can_parse),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        let spec = arg_string(args, 0, context)?;
        let base = match args.get(1) {
            Some(value) if !value.is_undefined() && !value.is_null() => {
                Some(value.to_string(context)?.to_std_string_lossy())
            }
            _ => None,
        };
        let page = host(context)?.location.borrow().clone();
        let base_url = mithu_url::Url::parse(base.as_deref().unwrap_or(page.as_str()))
            .or_else(|_| mithu_url::Url::parse(&page))
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        let url = if spec.is_empty() {
            base_url
        } else {
            base_url
                .resolve(&spec)
                .or_else(|_| mithu_url::Url::parse(&spec))
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
        };
        Ok(Self { url })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let url = instance.as_ref().borrow().data().url.clone();
        let search = url
            .query
            .as_ref()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let params = SearchParams::from_data(
            SearchParams {
                query: url.query.clone().unwrap_or_default(),
            },
            context,
        )?;
        let object = instance.clone().upcast();
        object.set(js_string!("href"), js_string!(href_of(&url)), true, context)?;
        object.set(
            js_string!("origin"),
            js_string!(origin_of(&url)),
            true,
            context,
        )?;
        object.set(
            js_string!("protocol"),
            js_string!(format!("{}:", url.scheme)),
            true,
            context,
        )?;
        object.set(js_string!("username"), js_string!(""), true, context)?;
        object.set(js_string!("password"), js_string!(""), true, context)?;
        object.set(js_string!("host"), js_string!(host_of(&url)), true, context)?;
        object.set(
            js_string!("hostname"),
            js_string!(url.host.clone()),
            true,
            context,
        )?;
        object.set(js_string!("port"), js_string!(port_of(&url)), true, context)?;
        object.set(
            js_string!("pathname"),
            js_string!(url.path.clone()),
            true,
            context,
        )?;
        object.set(js_string!("search"), js_string!(search), true, context)?;
        object.set(js_string!("hash"), js_string!(hash_of(&url)), true, context)?;
        object.set(js_string!("searchParams"), params, true, context)?;
        Ok(())
    }
}

fn url_to_string(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this
        .as_object()
        .and_then(|object| {
            object
                .downcast_ref::<PageUrl>()
                .map(|data| data.url.clone())
        })
        .ok_or_else(|| {
            JsNativeError::typ().with_message("URL method called on the wrong object")
        })?;
    Ok(js_string!(href_of(&url)).into())
}

fn url_can_parse(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let spec = arg_string(args, 0, context).unwrap_or_default();
    let base = args
        .get(1)
        .filter(|value| !value.is_undefined() && !value.is_null())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_lossy());
    let page = host(context)
        .ok()
        .map(|host| host.location.borrow().clone())
        .unwrap_or_else(|| "about:blank".to_string());
    let Ok(base_url) = mithu_url::Url::parse(base.as_deref().unwrap_or(page.as_str())) else {
        return Ok(JsValue::from(false));
    };
    let parsed = if spec.is_empty() {
        true
    } else {
        base_url.resolve(&spec).is_ok() || mithu_url::Url::parse(&spec).is_ok()
    };
    Ok(JsValue::from(parsed))
}

fn href_of(url: &mithu_url::Url) -> String {
    if url.scheme == "data" || url.scheme == "blob" {
        return format!("{}:{}", url.scheme, url.path);
    }
    url.to_display()
}

fn origin_of(url: &mithu_url::Url) -> String {
    if !url.is_network() {
        return "null".to_string();
    }
    match url.port {
        Some(port) if mithu_url::Url::default_port(&url.scheme) != Some(port) => {
            format!("{}://{}:{port}", url.scheme, url.host)
        }
        _ => format!("{}://{}", url.scheme, url.host),
    }
}

fn host_of(url: &mithu_url::Url) -> String {
    match url.port {
        Some(port) if mithu_url::Url::default_port(&url.scheme) != Some(port) => {
            format!("{}:{port}", url.host)
        }
        _ => url.host.clone(),
    }
}

fn port_of(url: &mithu_url::Url) -> String {
    match url.port {
        Some(port) if mithu_url::Url::default_port(&url.scheme) != Some(port) => port.to_string(),
        _ => String::new(),
    }
}

fn hash_of(url: &mithu_url::Url) -> String {
    url.fragment
        .as_ref()
        .map(|fragment| format!("#{fragment}"))
        .unwrap_or_default()
}

#[derive(Debug, Trace, Finalize, JsData)]
struct SearchParams {
    #[unsafe_ignore_trace]
    query: String,
}

impl Class for SearchParams {
    const NAME: &'static str = "URLSearchParams";
    const LENGTH: usize = 0;

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(params_get),
        );
        class.method(
            js_string!("has"),
            1,
            NativeFunction::from_fn_ptr(params_has),
        );
        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(params_to_string),
        );
        class.method(
            js_string!("set"),
            2,
            NativeFunction::from_fn_ptr(params_set),
        );
        class.method(
            js_string!("append"),
            2,
            NativeFunction::from_fn_ptr(params_append),
        );
        class.method(
            js_string!("delete"),
            1,
            NativeFunction::from_fn_ptr(params_delete),
        );
        class.method(
            js_string!("forEach"),
            1,
            NativeFunction::from_fn_ptr(params_for_each),
        );
        class.method(
            js_string!("keys"),
            0,
            NativeFunction::from_fn_ptr(params_keys),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        let query = if args.first().is_some_and(|value| !value.is_undefined()) {
            arg_string(args, 0, context)?
        } else {
            String::new()
        };
        let query = query.strip_prefix('?').unwrap_or(&query).to_string();
        Ok(Self { query })
    }
}

fn params_of(this: &JsValue) -> JsResult<String> {
    this.as_object()
        .and_then(|object| {
            object
                .downcast_ref::<SearchParams>()
                .map(|data| data.query.clone())
        })
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("URLSearchParams method called on the wrong object")
                .into()
        })
}

fn params_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = params_of(this)?;
    let name = arg_string(args, 0, context)?;
    Ok(match query_get(&query, &name) {
        Some(value) => js_string!(value).into(),
        None => JsValue::null(),
    })
}

fn params_has(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = params_of(this)?;
    let name = arg_string(args, 0, context)?;
    Ok(JsValue::from(query_get(&query, &name).is_some()))
}

fn params_to_string(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(js_string!(params_of(this)?).into())
}

fn params_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("URLSearchParams method called on the wrong object")
    })?;
    let Some(mut params) = object.downcast_mut::<SearchParams>() else {
        return Err(JsNativeError::typ()
            .with_message("URLSearchParams method called on the wrong object")
            .into());
    };
    let mut pairs: Vec<(String, String)> = query_pairs(&params.query)
        .into_iter()
        .filter(|(key, _)| key != &name)
        .collect();
    pairs.push((name, value));
    params.query = join_pairs(&pairs);
    Ok(JsValue::undefined())
}

fn params_append(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("URLSearchParams method called on the wrong object")
    })?;
    let Some(mut params) = object.downcast_mut::<SearchParams>() else {
        return Err(JsNativeError::typ()
            .with_message("URLSearchParams method called on the wrong object")
            .into());
    };
    if !params.query.is_empty() {
        params.query.push('&');
    }
    params.query.push_str(&form_escape(&name));
    params.query.push('=');
    params.query.push_str(&form_escape(&value));
    Ok(JsValue::undefined())
}

fn params_for_each(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = params_of(this)?;
    let Some(callback) = args.first().and_then(JsValue::as_object) else {
        return Ok(JsValue::undefined());
    };
    if !callback.is_callable() {
        return Ok(JsValue::undefined());
    }
    let this_arg = args.get(1).cloned().unwrap_or(JsValue::undefined());
    for (name, value) in query_pairs(&query) {
        callback.call(
            &this_arg,
            &[
                js_string!(value).into(),
                js_string!(name).into(),
                this.clone(),
            ],
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn params_keys(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let query = params_of(this)?;
    let keys = query_pairs(&query)
        .into_iter()
        .map(|(name, _)| JsValue::from(js_string!(name)))
        .collect::<Vec<_>>();
    Ok(JsArray::from_iter(keys, context).into())
}

fn params_delete(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = arg_string(args, 0, context)?;
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("URLSearchParams method called on the wrong object")
    })?;
    let Some(mut params) = object.downcast_mut::<SearchParams>() else {
        return Err(JsNativeError::typ()
            .with_message("URLSearchParams method called on the wrong object")
            .into());
    };
    let pairs: Vec<(String, String)> = query_pairs(&params.query)
        .into_iter()
        .filter(|(key, _)| key != &name)
        .collect();
    params.query = join_pairs(&pairs);
    Ok(JsValue::undefined())
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            (form_unescape(name), form_unescape(value))
        })
        .collect()
}

fn query_get(query: &str, name: &str) -> Option<String> {
    query_pairs(query)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn join_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", form_escape(name), form_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn form_unescape(input: &str) -> String {
    let mut bytes = Vec::new();
    let raw = input.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'+' {
            bytes.push(b' ');
            index += 1;
        } else if raw[index] == b'%' && index + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                bytes.push(value);
                index += 3;
            } else {
                bytes.push(raw[index]);
                index += 1;
            }
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn form_escape(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[derive(Debug, Trace, Finalize, JsData)]
struct TextEncoderHost {
    #[unsafe_ignore_trace]
    _private: u8,
}

impl Class for TextEncoderHost {
    const NAME: &'static str = "TextEncoder";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("encode"),
            1,
            NativeFunction::from_fn_ptr(text_encode),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self { _private: 0 })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        instance.clone().upcast().set(
            js_string!("encoding"),
            js_string!("utf-8"),
            true,
            context,
        )?;
        Ok(())
    }
}

fn text_encode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = if args.first().is_some_and(|value| !value.is_undefined()) {
        arg_string(args, 0, context)?
    } else {
        String::new()
    };
    uint8_array(text.as_bytes(), context)
}

#[derive(Debug, Trace, Finalize, JsData)]
struct TextDecoderHost {
    #[unsafe_ignore_trace]
    _private: u8,
}

impl Class for TextDecoderHost {
    const NAME: &'static str = "TextDecoder";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("decode"),
            1,
            NativeFunction::from_fn_ptr(text_decode),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self { _private: 0 })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        instance.clone().upcast().set(
            js_string!("encoding"),
            js_string!("utf-8"),
            true,
            context,
        )?;
        Ok(())
    }
}

fn construct_event(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let event = context.global_object().get(js_string!("Event"), context)?;
    let Some(constructor) = event.as_constructor() else {
        return Ok(JsValue::undefined());
    };
    let target = this.as_object();
    Ok(constructor
        .construct(args, target.as_ref(), context)?
        .into())
}

fn text_decode(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(value) = args.first() else {
        return Ok(js_string!("").into());
    };
    if value.is_undefined() || value.is_null() {
        return Ok(js_string!("").into());
    }
    if let Some(text) = value.as_string() {
        return Ok(js_string!(text.to_std_string_lossy()).into());
    }
    let Some(object) = value.as_object() else {
        return Ok(js_string!("").into());
    };
    let length = object
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0) as usize;
    let mut bytes = Vec::with_capacity(length);
    for index in 0..length {
        let byte = object.get(index, context)?.to_u32(context).unwrap_or(0) as u8;
        bytes.push(byte);
    }
    Ok(js_string!(String::from_utf8_lossy(&bytes).into_owned()).into())
}

fn uint8_array(bytes: &[u8], context: &mut Context) -> JsResult<JsValue> {
    let constructor = context
        .intrinsics()
        .constructors()
        .typed_uint8_array()
        .constructor()
        .clone();
    let array = constructor.construct(
        &[JsValue::from(bytes.len() as u32)],
        Some(&constructor),
        context,
    )?;
    for (index, byte) in bytes.iter().enumerate() {
        array.set(index, JsValue::from(u32::from(*byte)), true, context)?;
    }
    Ok(array.into())
}

#[derive(Debug, Trace, Finalize, JsData)]
struct Xhr {
    #[unsafe_ignore_trace]
    method: String,
    #[unsafe_ignore_trace]
    url: String,
    #[unsafe_ignore_trace]
    asynchronous: bool,
    #[unsafe_ignore_trace]
    status: u16,
    #[unsafe_ignore_trace]
    body: String,
    #[unsafe_ignore_trace]
    headers: Vec<(String, String)>,
}

impl Class for Xhr {
    const NAME: &'static str = "XMLHttpRequest";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(js_string!("open"), 2, NativeFunction::from_fn_ptr(xhr_open));
        class.method(js_string!("send"), 0, NativeFunction::from_fn_ptr(xhr_send));
        class.method(
            js_string!("setRequestHeader"),
            2,
            NativeFunction::from_fn_ptr(xhr_header),
        );
        class.method(
            js_string!("getAllResponseHeaders"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| Ok(js_string!("").into())),
        );
        class.property(js_string!("UNSENT"), JsValue::from(0), ATTR);
        class.property(js_string!("OPENED"), JsValue::from(1), ATTR);
        class.property(js_string!("DONE"), JsValue::from(4), ATTR);
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self {
            method: "GET".to_string(),
            url: String::new(),
            asynchronous: true,
            status: 0,
            body: String::new(),
            headers: Vec::new(),
        })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let object = instance.clone().upcast();
        object.set(js_string!("readyState"), JsValue::from(0), true, context)?;
        object.set(js_string!("status"), JsValue::from(0), true, context)?;
        object.set(js_string!("statusText"), js_string!(""), true, context)?;
        object.set(js_string!("responseText"), js_string!(""), true, context)?;
        object.set(js_string!("response"), js_string!(""), true, context)?;
        object.set(js_string!("responseType"), js_string!(""), true, context)?;
        Ok(())
    }
}

fn xhr_open(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let method = arg_string(args, 0, context)?.to_ascii_uppercase();
    let url = arg_string(args, 1, context)?;
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("XMLHttpRequest method called on the wrong object")
    })?;
    {
        let Some(mut xhr) = object.downcast_mut::<Xhr>() else {
            return Err(JsNativeError::typ()
                .with_message("XMLHttpRequest method called on the wrong object")
                .into());
        };
        xhr.method = method;
        xhr.url = url;
        xhr.asynchronous = match args.get(2) {
            Some(value) if !value.is_undefined() => value.to_boolean(),
            _ => true,
        };
    }
    object.set(js_string!("readyState"), JsValue::from(1), true, context)?;
    Ok(JsValue::undefined())
}

fn xhr_header(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let Some(mut xhr) = object.downcast_mut::<Xhr>() else {
        return Ok(JsValue::undefined());
    };
    let name = arg_string(args, 0, context)?;
    let value = arg_string(args, 1, context)?;
    if !name.is_empty() {
        xhr.headers.push((name, value));
    }
    Ok(JsValue::undefined())
}

fn xhr_send(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("XMLHttpRequest method called on the wrong object")
    })?;
    let (method, url, asynchronous, headers) = {
        let Some(xhr) = object.downcast_ref::<Xhr>() else {
            return Err(JsNativeError::typ()
                .with_message("XMLHttpRequest method called on the wrong object")
                .into());
        };
        (
            xhr.method.clone(),
            xhr.url.clone(),
            xhr.asynchronous,
            xhr.headers.clone(),
        )
    };
    if std::env::var_os("MITHU_JS_DEBUG").is_some() {
        eprintln!("xhr {method} {url}");
    }
    let body = args
        .first()
        .filter(|value| !value.is_undefined() && !value.is_null())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_lossy())
        .unwrap_or_default();
    let (status, text) = xhr_roundtrip(&method, &url, &body, &headers, context);
    if asynchronous {
        if let Some(mut xhr) = object.downcast_mut::<Xhr>() {
            xhr.status = status;
            xhr.body = text;
        }
        crate::schedule_callback(context, object.clone().into(), 2, 0)?;
        return Ok(JsValue::undefined());
    }
    apply_xhr(&object, status, &text, context)?;
    Ok(JsValue::undefined())
}

pub(crate) fn finish_xhr(value: &JsValue, context: &mut Context) -> JsResult<()> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let (status, text) = {
        let Some(xhr) = object.downcast_ref::<Xhr>() else {
            return Ok(());
        };
        (xhr.status, xhr.body.clone())
    };
    apply_xhr(&object, status, &text, context)
}

fn apply_xhr(object: &JsObject, status: u16, text: &str, context: &mut Context) -> JsResult<()> {
    object.set(
        js_string!("status"),
        JsValue::from(u32::from(status)),
        true,
        context,
    )?;
    object.set(
        js_string!("statusText"),
        js_string!(if (200..300).contains(&status) {
            "OK"
        } else {
            ""
        }),
        true,
        context,
    )?;
    object.set(
        js_string!("responseText"),
        js_string!(text.to_string()),
        true,
        context,
    )?;
    let response_type = object
        .get(js_string!("responseType"), context)?
        .to_string(context)?
        .to_std_string_lossy();
    let response = if response_type == "json" {
        let json = context.global_object().get(js_string!("JSON"), context)?;
        match json.as_object().and_then(|value| {
            value
                .get(js_string!("parse"), context)
                .ok()
                .and_then(|parse| parse.as_object())
        }) {
            Some(parse) if parse.is_callable() => parse
                .call(&json, &[js_string!(text).into()], context)
                .unwrap_or(JsValue::null()),
            _ => JsValue::null(),
        }
    } else {
        js_string!(text).into()
    };
    object.set(js_string!("response"), response, true, context)?;
    object.set(js_string!("readyState"), JsValue::from(4), true, context)?;
    call_handler(object, "onreadystatechange", context)?;
    if (200..300).contains(&status) || status == 0 {
        call_handler(object, "onload", context)?;
    } else {
        call_handler(object, "onerror", context)?;
    }
    Ok(())
}

fn call_handler(object: &JsObject, name: &str, context: &mut Context) -> JsResult<()> {
    let handler = object.get(js_string!(name), context)?;
    if let Some(function) = handler
        .as_object()
        .filter(|function| function.is_callable())
    {
        let _ = function.call(&object.clone().into(), &[], context);
    }
    Ok(())
}

fn xhr_roundtrip(
    method: &str,
    spec: &str,
    body: &str,
    headers: &[(String, String)],
    context: &mut Context,
) -> (u16, String) {
    let Ok(host) = host(context) else {
        return (0, String::new());
    };
    let location = host.location.borrow().clone();
    let Ok(url) = resolve_request(&location, spec) else {
        return (0, String::new());
    };
    if !url.is_network() {
        return (0, String::new());
    }
    let initiator = mithu_url::Url::parse(&location).ok();
    let origin = initiator.as_ref().map(super::serialized_origin);
    let content_type = headers.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            .then_some(value.as_str())
    });
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("content-type"))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let fallback = if method.eq_ignore_ascii_case("POST") && !body.is_empty() {
        let trimmed = body.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            Some("application/json")
        } else {
            Some("application/x-www-form-urlencoded")
        }
    } else {
        None
    };
    let result = mithu_net::send(
        method,
        &url,
        content_type.or(fallback),
        body.as_bytes(),
        initiator.as_ref().map(|page| page.host.as_str()),
        origin.as_deref(),
        &header_refs,
    );
    match result {
        Ok(response) => {
            if let Some(page) = initiator.as_ref()
                && cross_origin(page, &response.url)
                && !cors_allows(page, response.header("access-control-allow-origin"))
            {
                return (0, String::new());
            }
            (response.status, response.text())
        }
        Err(_) => (0, String::new()),
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct MutationObserverHost {
    #[unsafe_ignore_trace]
    _private: u8,
}

impl Class for MutationObserverHost {
    const NAME: &'static str = "MutationObserver";
    const LENGTH: usize = 1;

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("observe"),
            1,
            NativeFunction::from_fn_ptr(observer_observe),
        );
        class.method(
            js_string!("disconnect"),
            0,
            NativeFunction::from_fn_ptr(observer_disconnect),
        );
        class.method(
            js_string!("takeRecords"),
            0,
            NativeFunction::from_fn_ptr(observer_take),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self { _private: 0 })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        if let Some(callback) = args.first() {
            instance.clone().upcast().set(
                js_string!("callback"),
                callback.clone(),
                true,
                context,
            )?;
        }
        Ok(())
    }
}

fn history_object(context: &mut Context) -> JsResult<JsObject> {
    let length = native(context, "get length", 0, history_length);
    let state = native(context, "get state", 0, history_state);
    let mut object = ObjectInitializer::new(context);
    object
        .accessor(js_string!("length"), Some(length), None, ATTR)
        .accessor(js_string!("state"), Some(state), None, ATTR)
        .function(
            NativeFunction::from_fn_ptr(history_push),
            js_string!("pushState"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(history_replace),
            js_string!("replaceState"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(history_back),
            js_string!("back"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(history_forward),
            js_string!("forward"),
            0,
        )
        .function(NativeFunction::from_fn_ptr(history_go), js_string!("go"), 1);
    Ok(object.build())
}

fn history_length(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    Ok(JsValue::from(host.history.borrow().len() as u32))
}

fn history_url(args: &[JsValue], context: &mut Context) -> JsResult<Option<String>> {
    let Some(value) = args.get(2) else {
        return Ok(None);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    let spec = value.to_string(context)?.to_std_string_lossy();
    if spec.is_empty() {
        return Ok(None);
    }
    let host = host(context)?;
    let location = host.location.borrow().clone();
    let next = resolve_request(&location, &spec)
        .map_err(|error| JsNativeError::typ().with_message(error))?;
    Ok(Some(next.to_display()))
}

fn history_state(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::null());
    };
    let value = object.get(js_string!("__mithuState"), context)?;
    if value.is_undefined() {
        Ok(JsValue::null())
    } else {
        Ok(value)
    }
}

fn remember_history_state(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<()> {
    let Some(object) = this.as_object() else {
        return Ok(());
    };
    let state = args.first().cloned().unwrap_or(JsValue::null());
    let state = if state.is_undefined() {
        JsValue::null()
    } else {
        state
    };
    object.set(js_string!("__mithuState"), state, false, context)?;
    Ok(())
}

fn history_push(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    remember_history_state(this, args, context)?;
    let Some(next) = history_url(args, context)? else {
        return Ok(JsValue::undefined());
    };
    let host = host(context)?;
    if host.location.borrow().as_str() == next {
        return Ok(JsValue::undefined());
    }
    *host.location.borrow_mut() = next.clone();
    let mut history = host.history.borrow_mut();
    let mut index = host.history_index.get();
    if index + 1 < history.len() {
        history.truncate(index + 1);
    }
    history.push(next);
    index = history.len() - 1;
    host.history_index.set(index);
    Ok(JsValue::undefined())
}

fn history_replace(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    remember_history_state(this, args, context)?;
    let Some(next) = history_url(args, context)? else {
        return Ok(JsValue::undefined());
    };
    let host = host(context)?;
    if host.location.borrow().as_str() == next {
        return Ok(JsValue::undefined());
    }
    *host.location.borrow_mut() = next.clone();
    let mut history = host.history.borrow_mut();
    let index = host.history_index.get();
    if let Some(slot) = history.get_mut(index) {
        *slot = next;
    }
    Ok(JsValue::undefined())
}

fn history_back(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    history_go_delta(-1, context)
}

fn history_forward(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    history_go_delta(1, context)
}

fn history_go(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let delta = args
        .first()
        .map(|value| value.to_number(context))
        .transpose()?
        .unwrap_or(0.0) as i32;
    history_go_delta(delta, context)
}

fn history_go_delta(delta: i32, context: &mut Context) -> JsResult<JsValue> {
    if delta == 0 {
        return Ok(JsValue::undefined());
    }
    let host = host(context)?;
    let history = host.history.borrow();
    if history.is_empty() {
        return Ok(JsValue::undefined());
    }
    let current = host.history_index.get() as i32;
    let next = (current + delta).clamp(0, history.len() as i32 - 1) as usize;
    if next == host.history_index.get() {
        return Ok(JsValue::undefined());
    }
    let url = history[next].clone();
    drop(history);
    host.history_index.set(next);
    *host.location.borrow_mut() = url.clone();
    *host.navigation.borrow_mut() = Some(url);
    Ok(JsValue::undefined())
}

fn btoa(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, context)?;
    if !text.is_ascii() {
        return Err(JsNativeError::typ()
            .with_message("btoa expects a Latin-1 string")
            .into());
    }
    Ok(js_string!(base64_encode(text.as_bytes())).into())
}

fn atob(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, context)?;
    let bytes = base64_decode(&text).ok_or_else(|| {
        JsNativeError::typ().with_message("atob received a string that is not base64")
    })?;
    let text: String = bytes.iter().map(|byte| char::from(*byte)).collect();
    Ok(js_string!(text).into())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut index = 0;
    while index + 2 < bytes.len() {
        let n = (u32::from(bytes[index]) << 16)
            | (u32::from(bytes[index + 1]) << 8)
            | u32::from(bytes[index + 2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        index += 3;
    }
    if index < bytes.len() {
        let rest = bytes.len() - index;
        let n = if rest == 1 {
            u32::from(bytes[index]) << 16
        } else {
            (u32::from(bytes[index]) << 16) | (u32::from(bytes[index + 1]) << 8)
        };
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if rest == 1 {
            out.push('=');
        } else {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        }
        out.push('=');
    }
    out
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'=' {
            break;
        }
        let a = value(bytes[index])?;
        let b = value(bytes[index + 1])?;
        out.push((a << 2) | (b >> 4));
        if index + 2 >= bytes.len() || bytes[index + 2] == b'=' {
            break;
        }
        let c = value(bytes[index + 2])?;
        out.push((b << 4) | (c >> 2));
        if index + 3 >= bytes.len() || bytes[index + 3] == b'=' {
            break;
        }
        let d = value(bytes[index + 3])?;
        out.push((c << 6) | d);
        index += 4;
    }
    Some(out)
}

fn observed_node(value: &JsValue, context: &mut Context) -> Option<usize> {
    if let Some(id) = crate::node_of(value) {
        return Some(id.0);
    }
    let object = value.as_object()?;
    let kind = object
        .get(js_string!("nodeType"), context)
        .ok()?
        .as_number()?;
    if kind == 9.0 {
        let host = crate::host(context).ok()?;
        let doc = host.doc.borrow();
        return (0..doc.nodes.len())
            .find(|index| doc.element_name(mithu_dom::NodeId(*index)) == Some("html"));
    }
    if kind == 11.0 {
        return object
            .get(js_string!("__host"), context)
            .ok()?
            .as_number()
            .map(|id| id as usize);
    }
    None
}

fn observer_observe(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(id) = args.first().and_then(|value| observed_node(value, context)) else {
        return Err(JsNativeError::typ()
            .with_message("MutationObserver.observe expects a node")
            .into());
    };
    let subtree = args
        .get(1)
        .and_then(JsValue::as_object)
        .and_then(|options| options.get(js_string!("subtree"), context).ok())
        .and_then(|value| value.as_boolean())
        .unwrap_or(false);
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("MutationObserver method called on the wrong object")
            .into());
    };
    object.set(
        js_string!("__target"),
        JsValue::from(id as u32),
        true,
        context,
    )?;
    object.set(
        js_string!("__subtree"),
        JsValue::from(subtree),
        true,
        context,
    )?;
    let list = context
        .global_object()
        .get(js_string!("__mithuObservers"), context)?;
    if let Some(list) = list.as_object() {
        let len = list
            .get(js_string!("length"), context)?
            .to_u32(context)
            .unwrap_or(0);
        list.set(len, this.clone(), true, context)?;
    }
    Ok(JsValue::undefined())
}

fn observer_disconnect(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if let Some(object) = this.as_object() {
        object.set(js_string!("__target"), JsValue::from(-1), true, context)?;
    }
    Ok(JsValue::undefined())
}

fn observer_take(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let target = this
        .as_object()
        .and_then(|object| object.get(js_string!("__target"), context).ok())
        .and_then(|value| value.as_number())
        .unwrap_or(-1.0);
    let mine = {
        let host = host(context)?;
        let mut doc = host.doc.borrow_mut();
        let mut mine = Vec::new();
        let mut rest = Vec::new();
        for record in doc.mutations.drain(..) {
            if target >= 0.0 && record.target.0 == target as usize {
                mine.push(record);
            } else {
                rest.push(record);
            }
        }
        doc.mutations = rest;
        mine
    };
    record_array(&mine, context)
}

pub(crate) fn deliver_mutations(context: &mut Context) -> JsResult<()> {
    for _ in 0..16 {
        let records = {
            let host = host(context)?;
            std::mem::take(&mut host.doc.borrow_mut().mutations)
        };
        if records.is_empty() {
            return Ok(());
        }
        deliver_one_batch(&records, context)?;
    }
    Ok(())
}

fn deliver_one_batch(records: &[mithu_dom::DomMutation], context: &mut Context) -> JsResult<()> {
    let list = context
        .global_object()
        .get(js_string!("__mithuObservers"), context)?;
    let Some(list) = list.as_object() else {
        return Ok(());
    };
    let len = list
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    for index in 0..len {
        let observer = list.get(index, context)?;
        let Some(object) = observer.as_object() else {
            continue;
        };
        let target = object
            .get(js_string!("__target"), context)?
            .as_number()
            .unwrap_or(-1.0);
        if target < 0.0 {
            continue;
        }
        let subtree = object
            .get(js_string!("__subtree"), context)?
            .as_boolean()
            .unwrap_or(false);
        let mine: Vec<_> = {
            let host = host(context)?;
            let doc = host.doc.borrow();
            let observed = mithu_dom::NodeId(target as usize);
            records
                .iter()
                .filter(|record| {
                    record.target == observed
                        || (subtree && doc.contains_node(observed, record.target))
                })
                .cloned()
                .collect()
        };
        if mine.is_empty() {
            continue;
        }
        let array = record_array(&mine, context)?;
        let callback = object.get(js_string!("callback"), context)?;
        if let Some(callback) = callback
            .as_object()
            .filter(|function| function.is_callable())
        {
            if let Err(error) = callback.call(&observer, &[array, observer.clone()], context)
                && std::env::var_os("MITHU_JS_DEBUG").is_some()
            {
                eprintln!("mutation observer: {error}");
            }
        }
    }
    Ok(())
}

fn node_list(ids: &[mithu_dom::NodeId], context: &mut Context) -> JsResult<JsValue> {
    let nodes = ids
        .iter()
        .map(|id| crate::element_value(id.0, context))
        .collect::<Vec<_>>();
    Ok(JsArray::from_iter(nodes, context).into())
}

fn record_array(records: &[mithu_dom::DomMutation], context: &mut Context) -> JsResult<JsValue> {
    let mut values = Vec::new();
    for record in records {
        let attribute = match &record.attribute {
            Some(name) => js_string!(name.clone()).into(),
            None => JsValue::null(),
        };
        let target = crate::element_value(record.target.0, context);
        let added = node_list(&record.added, context)?;
        let removed = node_list(&record.removed, context)?;
        let mut item = ObjectInitializer::new(context);
        item.property(js_string!("type"), js_string!(record.kind.clone()), ATTR)
            .property(js_string!("attributeName"), attribute, ATTR)
            .property(js_string!("target"), target, ATTR)
            .property(js_string!("addedNodes"), added, ATTR)
            .property(js_string!("removedNodes"), removed, ATTR);
        values.push(JsValue::from(item.build()));
    }
    Ok(JsArray::from_iter(values, context).into())
}

#[derive(Debug, Trace, Finalize, JsData)]
struct HtmlImage {
    #[unsafe_ignore_trace]
    src: String,
}

impl Class for HtmlImage {
    const NAME: &'static str = "Image";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let getter = {
            let context = class.context();
            native(context, "get src", 0, image_src)
        };
        let setter = {
            let context = class.context();
            native(context, "set src", 1, image_set_src)
        };
        class.accessor(js_string!("src"), Some(getter), Some(setter), ATTR);
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self { src: String::new() })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let object = instance.clone().upcast();
        object.set(js_string!("width"), JsValue::from(0), true, context)?;
        object.set(js_string!("height"), JsValue::from(0), true, context)?;
        object.set(js_string!("complete"), JsValue::from(false), true, context)?;
        Ok(())
    }
}

fn image_src(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let src = this
        .as_object()
        .and_then(|object| {
            object
                .downcast_ref::<HtmlImage>()
                .map(|image| image.src.clone())
        })
        .unwrap_or_default();
    Ok(js_string!(src).into())
}

fn image_set_src(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let src = arg_string(args, 0, context)?;
    if let Some(object) = this.as_object() {
        if let Some(mut image) = object.downcast_mut::<HtmlImage>() {
            image.src = src;
        }
    }
    if let Some(object) = this.as_object() {
        let handler = object.get(js_string!("onload"), context)?;
        if let Some(function) = handler
            .as_object()
            .filter(|function| function.is_callable())
        {
            let _ = function.call(this, &[], context);
        }
        object.set(js_string!("complete"), JsValue::from(true), true, context)?;
    }
    Ok(JsValue::undefined())
}

#[derive(Debug, Trace, Finalize, JsData)]
struct DomEvent;

impl Class for DomEvent {
    const NAME: &'static str = "Event";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let kind = args
            .first()
            .map(|value| {
                value
                    .to_string(context)
                    .map(|text| text.to_std_string_lossy())
            })
            .transpose()?
            .unwrap_or_default();
        let object = instance.clone().upcast();
        object.set(js_string!("type"), js_string!(kind), true, context)?;
        object.set(js_string!("bubbles"), JsValue::from(false), true, context)?;
        object.set(
            js_string!("cancelable"),
            JsValue::from(false),
            true,
            context,
        )?;
        object.set(js_string!("target"), JsValue::null(), true, context)?;
        object.set(
            js_string!("defaultPrevented"),
            JsValue::from(false),
            true,
            context,
        )?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct CustomEventHost;

impl Class for CustomEventHost {
    const NAME: &'static str = "CustomEvent";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let kind = args
            .first()
            .map(|value| {
                value
                    .to_string(context)
                    .map(|text| text.to_std_string_lossy())
            })
            .transpose()?
            .unwrap_or_default();
        let detail = args
            .get(1)
            .and_then(|value| value.as_object())
            .and_then(|object| object.get(js_string!("detail"), context).ok())
            .unwrap_or(JsValue::null());
        let object = instance.clone().upcast();
        object.set(js_string!("type"), js_string!(kind), true, context)?;
        object.set(js_string!("detail"), detail, true, context)?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct DocumentHost;

impl Class for DocumentHost {
    const NAME: &'static str = "Document";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("querySelector"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector),
        );
        class.method(
            js_string!("querySelectorAll"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector_all),
        );
        class.method(
            js_string!("importNode"),
            1,
            NativeFunction::from_fn_ptr(super::import_node),
        );
        class.method(
            js_string!("prepend"),
            0,
            NativeFunction::from_fn_ptr(super::prepend_nodes),
        );
        class.method(
            js_string!("append"),
            0,
            NativeFunction::from_fn_ptr(super::append_nodes),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct HtmlElementHost;

thread_local! {
    /// The element currently being upgraded. `HTMLElement`'s constructor
    /// returns it, so a subclass `super()` binds `this` to the existing node
    /// instead of a fresh object that is not in the document.
    static CUSTOM_UPGRADE: std::cell::RefCell<Option<JsValue>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn begin_custom_upgrade(element: JsValue) {
    CUSTOM_UPGRADE.with(|slot| *slot.borrow_mut() = Some(element));
}

pub(crate) fn end_custom_upgrade() {
    CUSTOM_UPGRADE.with(|slot| *slot.borrow_mut() = None);
}

impl Class for HtmlElementHost {
    const NAME: &'static str = "HTMLElement";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn construct(
        new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsObject> {
        let upgrading = CUSTOM_UPGRADE.with(|slot| slot.borrow_mut().take());
        if let Some(existing) = upgrading
            && let Some(object) = existing.as_object()
        {
            return Ok(object);
        }
        if new_target.is_undefined() {
            return Err(JsNativeError::typ()
                .with_message("cannot call constructor of native class `HTMLElement` without new")
                .into());
        }
        let data = Self::data_constructor(new_target, args, context)?;
        let object = Self::from_data(data, context)?;
        if let Some(constructor) = new_target.as_object()
            && let Ok(proto) = constructor.get(js_string!("prototype"), context)
            && let Some(proto) = proto.as_object()
        {
            object.set_prototype(Some(proto));
        }
        Ok(object)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct MessagePortHost {
    #[unsafe_ignore_trace]
    id: u32,
    #[unsafe_ignore_trace]
    peer: u32,
}

#[derive(Debug, Trace, Finalize, JsData)]
struct MessageChannelHost {
    #[unsafe_ignore_trace]
    _private: u8,
}

fn next_port_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Class for MessagePortHost {
    const NAME: &'static str = "MessagePort";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("postMessage"),
            1,
            NativeFunction::from_fn_ptr(port_post),
        );
        class.method(
            js_string!("start"),
            0,
            NativeFunction::from_fn_ptr(port_start),
        );
        class.method(
            js_string!("close"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
        );
        class.method(
            js_string!("addEventListener"),
            2,
            NativeFunction::from_fn_ptr(port_listen),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self {
            id: next_port_id(),
            peer: 0,
        })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let id = instance
            .clone()
            .upcast()
            .downcast_ref::<MessagePortHost>()
            .map(|port| port.id)
            .unwrap_or(0);
        remember_port(context, id, instance.clone().upcast().into())?;
        let object = instance.clone().upcast();
        let get = crate::native(context, "get onmessage", 0, port_onmessage_get);
        let set = crate::native(context, "set onmessage", 1, port_onmessage_set);
        let _ = crate::define_accessor(&object, context, "onmessage", get, Some(set));
        Ok(())
    }
}

impl Class for MessageChannelHost {
    const NAME: &'static str = "MessageChannel";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self { _private: 0 })
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let constructor = context
            .global_object()
            .get(js_string!("MessagePort"), context)?;
        let Some(constructor) = constructor.as_object() else {
            return Ok(());
        };
        let first = constructor.construct(&[], Some(&constructor), context)?;
        let second = constructor.construct(&[], Some(&constructor), context)?;
        let first_id = first
            .downcast_ref::<MessagePortHost>()
            .map(|port| port.id)
            .unwrap_or(0);
        let second_id = second
            .downcast_ref::<MessagePortHost>()
            .map(|port| port.id)
            .unwrap_or(0);
        if let Some(mut port) = first.downcast_mut::<MessagePortHost>() {
            port.peer = second_id;
        }
        if let Some(mut port) = second.downcast_mut::<MessagePortHost>() {
            port.peer = first_id;
        }
        let object = instance.clone().upcast();
        object.set(js_string!("port1"), first, true, context)?;
        object.set(js_string!("port2"), second, true, context)?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct AbortSignalHost;

impl Class for AbortSignalHost {
    const NAME: &'static str = "AbortSignal";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("addEventListener"),
            2,
            NativeFunction::from_fn_ptr(signal_listen),
        );
        class.method(
            js_string!("removeEventListener"),
            2,
            NativeFunction::from_fn_ptr(signal_ignore),
        );
        class.method(
            js_string!("throwIfAborted"),
            0,
            NativeFunction::from_fn_ptr(signal_throw_if_aborted),
        );
        class.static_method(
            js_string!("abort"),
            0,
            NativeFunction::from_fn_ptr(signal_abort_static),
        );
        class.static_method(
            js_string!("timeout"),
            1,
            NativeFunction::from_fn_ptr(signal_timeout),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let object = instance.clone().upcast();
        object.set(js_string!("aborted"), JsValue::from(false), true, context)?;
        object.set(js_string!("reason"), JsValue::undefined(), true, context)?;
        object.set(js_string!("__n"), JsValue::from(0), true, context)?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct AbortControllerHost;

impl Class for AbortControllerHost {
    const NAME: &'static str = "AbortController";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("abort"),
            0,
            NativeFunction::from_fn_ptr(controller_abort),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        _args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let constructor = context
            .global_object()
            .get(js_string!("AbortSignal"), context)?;
        let Some(constructor) = constructor.as_object() else {
            return Ok(());
        };
        let signal = constructor.construct(&[], Some(&constructor), context)?;
        let object = instance.clone().upcast();
        object.set(js_string!("signal"), signal, true, context)?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct HtmlTemplateHost;

impl Class for HtmlTemplateHost {
    const NAME: &'static str = "HTMLTemplateElement";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct EventTargetHost;

impl Class for EventTargetHost {
    const NAME: &'static str = "EventTarget";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("addEventListener"),
            2,
            NativeFunction::from_fn_ptr(super::add_event_listener),
        );
        class.method(
            js_string!("removeEventListener"),
            2,
            NativeFunction::from_fn_ptr(super::remove_event_listener),
        );
        class.method(
            js_string!("dispatchEvent"),
            1,
            NativeFunction::from_fn_ptr(super::dispatch_event),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct NodeHost;

impl Class for NodeHost {
    const NAME: &'static str = "Node";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.static_property(js_string!("ELEMENT_NODE"), JsValue::from(1), ATTR);
        class.static_property(js_string!("TEXT_NODE"), JsValue::from(3), ATTR);
        class.static_property(js_string!("COMMENT_NODE"), JsValue::from(8), ATTR);
        class.static_property(js_string!("DOCUMENT_NODE"), JsValue::from(9), ATTR);
        class.static_property(
            js_string!("DOCUMENT_FRAGMENT_NODE"),
            JsValue::from(11),
            ATTR,
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct ElementHost;

impl Class for ElementHost {
    const NAME: &'static str = "Element";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("getRootNode"),
            0,
            NativeFunction::from_fn_ptr(element_root),
        );
        class.method(
            js_string!("animate"),
            1,
            NativeFunction::from_fn_ptr(element_animate),
        );
        class.method(
            js_string!("attachShadow"),
            1,
            NativeFunction::from_fn_ptr(super::attach_shadow),
        );
        class.method(
            js_string!("getAttribute"),
            1,
            NativeFunction::from_fn_ptr(super::get_attribute),
        );
        class.method(
            js_string!("setAttribute"),
            2,
            NativeFunction::from_fn_ptr(super::set_attribute),
        );
        class.method(
            js_string!("removeAttribute"),
            1,
            NativeFunction::from_fn_ptr(super::remove_attribute),
        );
        class.method(
            js_string!("toggleAttribute"),
            1,
            NativeFunction::from_fn_ptr(super::toggle_attribute),
        );
        class.method(
            js_string!("getAttributeNS"),
            2,
            NativeFunction::from_fn_ptr(super::attribute_ns),
        );
        class.method(
            js_string!("setAttributeNS"),
            3,
            NativeFunction::from_fn_ptr(super::set_attribute_ns),
        );
        class.method(
            js_string!("removeAttributeNS"),
            2,
            NativeFunction::from_fn_ptr(super::remove_attribute_ns),
        );
        class.method(
            js_string!("insertAdjacentHTML"),
            2,
            NativeFunction::from_fn_ptr(super::insert_adjacent_html),
        );
        class.method(
            js_string!("insertAdjacentElement"),
            2,
            NativeFunction::from_fn_ptr(super::insert_adjacent_element),
        );
        class.method(
            js_string!("querySelector"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector),
        );
        class.method(
            js_string!("querySelectorAll"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector_all),
        );
        class.method(
            js_string!("closest"),
            1,
            NativeFunction::from_fn_ptr(super::closest_selector),
        );
        class.method(
            js_string!("matches"),
            1,
            NativeFunction::from_fn_ptr(super::matches_selector),
        );
        class.method(
            js_string!("prepend"),
            0,
            NativeFunction::from_fn_ptr(super::prepend_nodes),
        );
        class.method(
            js_string!("append"),
            0,
            NativeFunction::from_fn_ptr(super::append_nodes),
        );
        class.method(
            js_string!("before"),
            0,
            NativeFunction::from_fn_ptr(super::before_node),
        );
        class.method(
            js_string!("after"),
            0,
            NativeFunction::from_fn_ptr(super::after_node),
        );
        class.method(
            js_string!("remove"),
            0,
            NativeFunction::from_fn_ptr(super::remove_self),
        );
        let html_get = {
            let context = class.context();
            native(context, "get innerHTML", 0, super::get_html)
        };
        let html_set = {
            let context = class.context();
            native(context, "set innerHTML", 1, super::set_html)
        };
        class.accessor(
            js_string!("innerHTML"),
            Some(html_get),
            Some(html_set),
            ATTR,
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct DocumentFragmentHost;

impl Class for DocumentFragmentHost {
    const NAME: &'static str = "DocumentFragment";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("prepend"),
            0,
            NativeFunction::from_fn_ptr(super::prepend_nodes),
        );
        class.method(
            js_string!("append"),
            0,
            NativeFunction::from_fn_ptr(super::append_nodes),
        );
        class.method(
            js_string!("querySelector"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector),
        );
        class.method(
            js_string!("querySelectorAll"),
            1,
            NativeFunction::from_fn_ptr(super::query_selector_all),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct ShadowRootHost;

impl Class for ShadowRootHost {
    const NAME: &'static str = "ShadowRoot";

    fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

fn element_root(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    context.global_object().get(js_string!("document"), context)
}

fn element_animate(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let finished = JsPromise::resolve(JsValue::undefined(), context)?;
    let mut animation = ObjectInitializer::new(context);
    animation
        .property(js_string!("finished"), finished, ATTR)
        .property(js_string!("playState"), js_string!("finished"), ATTR)
        .function(
            NativeFunction::from_fn_ptr(signal_ignore),
            js_string!("play"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(signal_ignore),
            js_string!("pause"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(signal_ignore),
            js_string!("cancel"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(signal_ignore),
            js_string!("finish"),
            0,
        );
    Ok(animation.build().into())
}

macro_rules! html_class {
    ($($struct:ident => $name:literal),+ $(,)?) => {
        $(
            #[derive(Debug, Trace, Finalize, JsData)]
            struct $struct;

            impl Class for $struct {
                const NAME: &'static str = $name;

                fn init(_class: &mut ClassBuilder<'_>) -> JsResult<()> {
                    Ok(())
                }

                fn data_constructor(
                    _new_target: &JsValue,
                    _args: &[JsValue],
                    _context: &mut Context,
                ) -> JsResult<Self> {
                    Ok(Self)
                }
            }
        )+

        fn register_html_elements(context: &mut Context) -> JsResult<()> {
            $(context.register_global_class::<$struct>()?;)+
            Ok(())
        }

        pub(crate) const HTML_ELEMENT_CLASSES: &[&str] = &[$($name),+];
    };
}

html_class! {
    HtmlAnchorElement => "HTMLAnchorElement",
    HtmlBodyElement => "HTMLBodyElement",
    HtmlButtonElement => "HTMLButtonElement",
    HtmlCanvasElement => "HTMLCanvasElement",
    HtmlDivElement => "HTMLDivElement",
    HtmlFormElement => "HTMLFormElement",
    HtmlHeadElement => "HTMLHeadElement",
    HtmlHeadingElement => "HTMLHeadingElement",
    HtmlHtmlElement => "HTMLHtmlElement",
    HtmlIFrameElement => "HTMLIFrameElement",
    HtmlInputElement => "HTMLInputElement",
    HtmlLabelElement => "HTMLLabelElement",
    HtmlLiElement => "HTMLLIElement",
    HtmlLinkElement => "HTMLLinkElement",
    HtmlMetaElement => "HTMLMetaElement",
    HtmlOListElement => "HTMLOListElement",
    HtmlOptionElement => "HTMLOptionElement",
    HtmlParagraphElement => "HTMLParagraphElement",
    HtmlPictureElement => "HTMLPictureElement",
    HtmlScriptElement => "HTMLScriptElement",
    HtmlSelectElement => "HTMLSelectElement",
    HtmlSlotElement => "HTMLSlotElement",
    HtmlSourceElement => "HTMLSourceElement",
    HtmlSpanElement => "HTMLSpanElement",
    HtmlStyleElement => "HTMLStyleElement",
    HtmlTableCellElement => "HTMLTableCellElement",
    HtmlTableElement => "HTMLTableElement",
    HtmlTableRowElement => "HTMLTableRowElement",
    HtmlTableSectionElement => "HTMLTableSectionElement",
    HtmlTextAreaElement => "HTMLTextAreaElement",
    HtmlTitleElement => "HTMLTitleElement",
    HtmlUListElement => "HTMLUListElement",
    HtmlVideoElement => "HTMLVideoElement",
    HtmlAudioElement => "HTMLAudioElement",
    HtmlAreaElement => "HTMLAreaElement",
    HtmlBaseElement => "HTMLBaseElement",
    HtmlBrElement => "HTMLBRElement",
    HtmlDataElement => "HTMLDataElement",
    HtmlDataListElement => "HTMLDataListElement",
    HtmlDetailsElement => "HTMLDetailsElement",
    HtmlDialogElement => "HTMLDialogElement",
    HtmlDirectoryElement => "HTMLDirectoryElement",
    HtmlDListElement => "HTMLDListElement",
    HtmlEmbedElement => "HTMLEmbedElement",
    HtmlFieldSetElement => "HTMLFieldSetElement",
    HtmlFontElement => "HTMLFontElement",
    HtmlFrameElement => "HTMLFrameElement",
    HtmlFrameSetElement => "HTMLFrameSetElement",
    HtmlHrElement => "HTMLHRElement",
    HtmlLegendElement => "HTMLLegendElement",
    HtmlMapElement => "HTMLMapElement",
    HtmlMarqueeElement => "HTMLMarqueeElement",
    HtmlMediaElement => "HTMLMediaElement",
    HtmlMenuElement => "HTMLMenuElement",
    HtmlMeterElement => "HTMLMeterElement",
    HtmlModElement => "HTMLModElement",
    HtmlObjectElement => "HTMLObjectElement",
    HtmlOptGroupElement => "HTMLOptGroupElement",
    HtmlOutputElement => "HTMLOutputElement",
    HtmlParamElement => "HTMLParamElement",
    HtmlPreElement => "HTMLPreElement",
    HtmlProgressElement => "HTMLProgressElement",
    HtmlQuoteElement => "HTMLQuoteElement",
    HtmlTableCaptionElement => "HTMLTableCaptionElement",
    HtmlTableColElement => "HTMLTableColElement",
    HtmlTimeElement => "HTMLTimeElement",
    HtmlTrackElement => "HTMLTrackElement",
}

#[derive(Debug, Trace, Finalize, JsData)]
struct IntersectionObserverHost;

impl Class for IntersectionObserverHost {
    const NAME: &'static str = "IntersectionObserver";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("observe"),
            1,
            NativeFunction::from_fn_ptr(intersection_observe),
        );
        class.method(
            js_string!("unobserve"),
            1,
            NativeFunction::from_fn_ptr(signal_ignore),
        );
        class.method(
            js_string!("disconnect"),
            0,
            NativeFunction::from_fn_ptr(signal_ignore),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let callback = args.first().cloned().unwrap_or(JsValue::undefined());
        let object = instance.clone().upcast();
        object.set(js_string!("callback"), callback, true, context)?;
        Ok(())
    }
}

#[derive(Debug, Trace, Finalize, JsData)]
struct ResizeObserverHost;

impl Class for ResizeObserverHost {
    const NAME: &'static str = "ResizeObserver";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("observe"),
            1,
            NativeFunction::from_fn_ptr(resize_observe),
        );
        class.method(
            js_string!("unobserve"),
            1,
            NativeFunction::from_fn_ptr(signal_ignore),
        );
        class.method(
            js_string!("disconnect"),
            0,
            NativeFunction::from_fn_ptr(signal_ignore),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let callback = args.first().cloned().unwrap_or(JsValue::undefined());
        let object = instance.clone().upcast();
        object.set(js_string!("callback"), callback, true, context)?;
        Ok(())
    }
}

fn resize_observe(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let target = args.first().cloned().unwrap_or(JsValue::undefined());
    let id = observed_node(&target, context);
    let (width, height) = observer_size(id, context);
    let rect = ObjectInitializer::new(context)
        .property(js_string!("x"), JsValue::from(0), ATTR)
        .property(js_string!("y"), JsValue::from(0), ATTR)
        .property(js_string!("top"), JsValue::from(0), ATTR)
        .property(js_string!("left"), JsValue::from(0), ATTR)
        .property(js_string!("width"), JsValue::from(width), ATTR)
        .property(js_string!("height"), JsValue::from(height), ATTR)
        .property(js_string!("right"), JsValue::from(width), ATTR)
        .property(js_string!("bottom"), JsValue::from(height), ATTR)
        .build();
    let entry = ObjectInitializer::new(context)
        .property(js_string!("target"), target, ATTR)
        .property(js_string!("contentRect"), rect, ATTR)
        .build();
    queue_layout_observer(this, JsValue::from(entry), id, context)
}

fn intersection_observe(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let target = args.first().cloned().unwrap_or(JsValue::undefined());
    let id = observed_node(&target, context);
    let entry = ObjectInitializer::new(context)
        .property(js_string!("isIntersecting"), JsValue::from(true), ATTR)
        .property(js_string!("intersectionRatio"), JsValue::from(1), ATTR)
        .property(js_string!("target"), target, ATTR)
        .build();
    queue_layout_observer(this, JsValue::from(entry), id, context)
}

fn observer_size(id: Option<usize>, context: &mut Context) -> (f64, f64) {
    let Some(id) = id else {
        return (1280.0, 800.0);
    };
    let Ok(host) = crate::host(context) else {
        return (1280.0, 800.0);
    };
    let metrics = host.geometry.borrow().get(&id).copied();
    match metrics {
        Some(metrics) if metrics.width > 0.0 && metrics.height > 0.0 => {
            (f64::from(metrics.width), f64::from(metrics.height))
        }
        _ => (1280.0, 800.0),
    }
}

/// Deliver one observation per target, as a macrotask. Calling the callback
/// inside `observe` re-enters React during render and hits the nested-update limit.
fn queue_layout_observer(
    this: &JsValue,
    entry: JsValue,
    target: Option<usize>,
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let seen = object.get(js_string!("__seen"), context)?;
    let seen = if let Some(seen) = seen.as_object() {
        seen.clone()
    } else {
        let seen = ObjectInitializer::new(context).build();
        object.set(js_string!("__seen"), seen.clone(), true, context)?;
        seen
    };
    if let Some(id) = target {
        let key = js_string!(id.to_string());
        if seen.get(key.clone(), context)?.as_boolean() == Some(true) {
            return Ok(JsValue::undefined());
        }
        seen.set(key, JsValue::from(true), true, context)?;
    }
    let records = object.get(js_string!("__records"), context)?;
    let records = if records.is_object() {
        records
    } else {
        let records = JsValue::from(JsArray::from_iter([], context));
        object.set(js_string!("__records"), records.clone(), true, context)?;
        records
    };
    let Some(records) = records.as_object() else {
        return Ok(JsValue::undefined());
    };
    let len = records
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    records.set(len, entry, true, context)?;
    if object
        .get(js_string!("__scheduled"), context)?
        .as_boolean()
        != Some(true)
    {
        object.set(js_string!("__scheduled"), JsValue::from(true), true, context)?;
        crate::schedule_callback(context, this.clone(), 4, 0)?;
    }
    Ok(JsValue::undefined())
}

pub(crate) fn deliver_layout_observer(value: &JsValue, context: &mut Context) -> JsResult<()> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    object.set(
        js_string!("__scheduled"),
        JsValue::from(false),
        true,
        context,
    )?;
    let records = object.get(js_string!("__records"), context)?;
    object.set(
        js_string!("__records"),
        JsArray::from_iter([], context),
        true,
        context,
    )?;
    let callback = object.get(js_string!("callback"), context)?;
    if let Some(function) = callback
        .as_object()
        .filter(|function| function.is_callable())
    {
        let _ = function.call(value, &[records, value.clone()], context);
    }
    Ok(())
}

#[derive(Debug, Trace, Finalize, JsData)]
struct RangeHost;

impl Class for RangeHost {
    const NAME: &'static str = "Range";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        for name in [
            "setStart",
            "setEnd",
            "setStartBefore",
            "setStartAfter",
            "setEndBefore",
            "setEndAfter",
            "selectNode",
            "selectNodeContents",
            "collapse",
            "cloneContents",
            "deleteContents",
            "extractContents",
            "insertNode",
            "surroundContents",
        ] {
            class.method(
                js_string!(name.to_string()),
                0,
                NativeFunction::from_fn_ptr(range_noop),
            );
        }
        class.method(
            js_string!("getBoundingClientRect"),
            0,
            NativeFunction::from_fn_ptr(range_rect),
        );
        class.method(
            js_string!("getClientRects"),
            0,
            NativeFunction::from_fn_ptr(range_rects),
        );
        class.method(
            js_string!("cloneRange"),
            0,
            NativeFunction::from_fn_ptr(range_clone),
        );
        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(range_string),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }
}

fn range_noop(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn range_string(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!("").into())
}

fn range_rect(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(ObjectInitializer::new(context)
        .property(js_string!("x"), JsValue::from(0), ATTR)
        .property(js_string!("y"), JsValue::from(0), ATTR)
        .property(js_string!("width"), JsValue::from(0), ATTR)
        .property(js_string!("height"), JsValue::from(0), ATTR)
        .property(js_string!("top"), JsValue::from(0), ATTR)
        .property(js_string!("right"), JsValue::from(0), ATTR)
        .property(js_string!("bottom"), JsValue::from(0), ATTR)
        .property(js_string!("left"), JsValue::from(0), ATTR)
        .build()
        .into())
}

fn range_rects(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let rect = range_rect(this, args, context)?;
    Ok(JsArray::from_iter([rect], context).into())
}

fn range_clone(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let constructor = context.global_object().get(js_string!("Range"), context)?;
    let Some(constructor) = constructor.as_object() else {
        return Ok(JsValue::undefined());
    };
    constructor.construct(&[], None, context).map(JsValue::from)
}

#[derive(Debug, Trace, Finalize, JsData)]
struct FontFaceHost;

impl Class for FontFaceHost {
    const NAME: &'static str = "FontFace";

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("load"),
            0,
            NativeFunction::from_fn_ptr(font_face_load),
        );
        Ok(())
    }

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self)
    }

    fn object_constructor(
        instance: &JsObject<Self>,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<()> {
        let family = args
            .first()
            .cloned()
            .unwrap_or_else(|| js_string!("").into());
        let object = instance.clone().upcast();
        object.set(js_string!("family"), family, true, context)?;
        object.set(js_string!("status"), js_string!("loaded"), true, context)?;
        Ok(())
    }
}

fn font_face_load(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsPromise::resolve(this.clone(), context)?.into())
}

fn signal_ignore(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn signal_listen(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let event = arg_string(args, 0, context)?;
    if event != "abort" {
        return Ok(JsValue::undefined());
    }
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let count = object
        .get(js_string!("__n"), context)?
        .as_number()
        .unwrap_or(0.0) as u32;
    object.set(js_string!(format!("__l{count}")), handler, true, context)?;
    object.set(js_string!("__n"), JsValue::from(count + 1), true, context)?;
    Ok(JsValue::undefined())
}

fn signal_throw_if_aborted(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    if object
        .get(js_string!("aborted"), context)?
        .as_boolean()
        .unwrap_or(false)
    {
        let reason = object.get(js_string!("reason"), context)?;
        let message = reason.to_string(context)?.to_std_string_lossy();
        return Err(JsNativeError::typ().with_message(message).into());
    }
    Ok(JsValue::undefined())
}

fn signal_abort_static(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let constructor = context
        .global_object()
        .get(js_string!("AbortSignal"), context)?;
    let Some(constructor) = constructor.as_object() else {
        return Ok(JsValue::undefined());
    };
    let signal = constructor.construct(&[], Some(&constructor), context)?;
    let value = JsValue::from(signal.clone());
    abort_signal(&value, args.first().cloned(), context)?;
    Ok(value)
}

fn signal_timeout(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    // The timer is not wired to the host clock. Returning a live signal lets
    // the caller keep going; aborting immediately would cancel the fetch.
    let constructor = context
        .global_object()
        .get(js_string!("AbortSignal"), context)?;
    let Some(constructor) = constructor.as_object() else {
        return Ok(JsValue::undefined());
    };
    Ok(constructor
        .construct(&[], Some(&constructor), context)?
        .into())
}

fn controller_abort(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(controller) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let signal = controller.get(js_string!("signal"), context)?;
    abort_signal(&signal, args.first().cloned(), context)?;
    Ok(JsValue::undefined())
}

fn abort_signal(signal: &JsValue, reason: Option<JsValue>, context: &mut Context) -> JsResult<()> {
    let Some(object) = signal.as_object() else {
        return Ok(());
    };
    if object
        .get(js_string!("aborted"), context)?
        .as_boolean()
        .unwrap_or(false)
    {
        return Ok(());
    }
    let reason = reason.unwrap_or_else(|| js_string!("AbortError").into());
    object.set(js_string!("aborted"), JsValue::from(true), true, context)?;
    object.set(js_string!("reason"), reason, true, context)?;
    let count = object
        .get(js_string!("__n"), context)?
        .as_number()
        .unwrap_or(0.0) as u32;
    let mut handlers = Vec::new();
    for index in 0..count {
        handlers.push(object.get(js_string!(format!("__l{index}")), context)?);
    }
    let onabort = object.get(js_string!("onabort"), context)?;
    if !onabort.is_undefined() && !onabort.is_null() {
        handlers.push(onabort);
    }
    let event = ObjectInitializer::new(context)
        .property(js_string!("type"), js_string!("abort"), ATTR)
        .property(js_string!("target"), signal.clone(), ATTR)
        .build();
    for handler in handlers {
        if handler.is_callable()
            && let Some(function) = handler.as_object()
        {
            let _ = function.call(signal, &[JsValue::from(event.clone())], context);
        }
    }
    Ok(())
}

fn remember_port(context: &mut Context, id: u32, value: JsValue) -> JsResult<()> {
    let bag = context
        .global_object()
        .get(js_string!("__mithuPorts"), context)?;
    if let Some(bag) = bag.as_object() {
        bag.set(js_string!(id.to_string()), value, true, context)?;
    }
    Ok(())
}

fn port_post(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let peer = this
        .as_object()
        .and_then(|object| {
            object
                .downcast_ref::<MessagePortHost>()
                .map(|port| port.peer)
        })
        .unwrap_or(0);
    let data = clone_message(args.first().unwrap_or(&JsValue::undefined()), context)?;
    let bag = context
        .global_object()
        .get(js_string!("__mithuPorts"), context)?;
    let Some(bag) = bag.as_object() else {
        return Ok(JsValue::undefined());
    };
    let peer_value = bag.get(js_string!(peer.to_string()), context)?;
    let Some(peer_object) = peer_value.as_object() else {
        return Ok(JsValue::undefined());
    };
    let event = ObjectInitializer::new(context)
        .property(js_string!("data"), data, ATTR)
        .property(js_string!("target"), peer_value.clone(), ATTR)
        .build();
    let event = JsValue::from(event);
    let started = peer_object
        .get(js_string!("__started"), context)?
        .as_boolean()
        .unwrap_or(false);
    if started {
        crate::schedule_callback(context, event, 3, 0)?;
        return Ok(JsValue::undefined());
    }
    let existing = peer_object.get(js_string!("__queue"), context)?;
    let list = if let Some(list) = existing.as_object() {
        list
    } else {
        let list = JsObject::from(JsArray::from_iter([], context));
        peer_object.set(js_string!("__queue"), list.clone(), true, context)?;
        list
    };
    let len = list
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    list.set(len, event, true, context)?;
    Ok(JsValue::undefined())
}

fn port_onmessage_get(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::null());
    };
    object.get(js_string!("__onmessage"), context)
}

fn port_onmessage_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let handler = args.first().cloned().unwrap_or(JsValue::null());
    if let Some(object) = this.as_object() {
        object.set(js_string!("__onmessage"), handler.clone(), true, context)?;
    }
    port_start(this, &[], context)?;
    Ok(handler)
}

fn port_start(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let already = object
        .get(js_string!("__started"), context)?
        .as_boolean()
        .unwrap_or(false);
    object.set(js_string!("__started"), JsValue::from(true), true, context)?;
    if already {
        return Ok(JsValue::undefined());
    }
    let queue = object.get(js_string!("__queue"), context)?;
    object.set(js_string!("__queue"), JsValue::undefined(), true, context)?;
    if let Some(list) = queue.as_object() {
        let len = list
            .get(js_string!("length"), context)?
            .to_u32(context)
            .unwrap_or(0);
        for index in 0..len {
            let event = list.get(index, context)?;
            crate::schedule_callback(context, event, 3, 0)?;
        }
    }
    Ok(JsValue::undefined())
}

pub(crate) fn deliver_port_message(event: &JsValue, context: &mut Context) -> JsResult<()> {
    let Some(event_object) = event.as_object() else {
        return Ok(());
    };
    let target = event_object.get(js_string!("target"), context)?;
    let Some(peer) = target.as_object() else {
        return Ok(());
    };
    let handler = peer.get(js_string!("onmessage"), context)?;
    if let Some(function) = handler
        .as_object()
        .filter(|function| function.is_callable())
    {
        function.call(&target, &[event.clone()], context)?;
    }
    let listeners = peer.get(js_string!("__listeners"), context)?;
    if let Some(listeners) = listeners.as_object() {
        let len = listeners
            .get(js_string!("length"), context)?
            .to_u32(context)
            .unwrap_or(0);
        for index in 0..len {
            let listener = listeners.get(index, context)?;
            if let Some(function) = listener
                .as_object()
                .filter(|function| function.is_callable())
            {
                function.call(&target, &[event.clone()], context)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn clone_message_value(value: &JsValue, context: &mut Context) -> JsResult<JsValue> {
    clone_message(value, context)
}

fn port_listen(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind = arg_string(args, 0, context)?;
    if kind != "message" {
        return Ok(JsValue::undefined());
    }
    let Some(function) = args.get(1).cloned() else {
        return Ok(JsValue::undefined());
    };
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    let existing = object.get(js_string!("__listeners"), context)?;
    let list = if let Some(list) = existing.as_object() {
        list
    } else {
        let list = JsObject::from(JsArray::from_iter([], context));
        object.set(js_string!("__listeners"), list.clone(), true, context)?;
        list
    };
    let len = list
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0);
    list.set(len, function, true, context)?;
    Ok(JsValue::undefined())
}

fn clone_message(value: &JsValue, context: &mut Context) -> JsResult<JsValue> {
    if value.is_undefined()
        || value.is_null()
        || value.is_boolean()
        || value.is_number()
        || value.is_string()
    {
        return Ok(value.clone());
    }
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let Some(json) = json.as_object() else {
        return Ok(JsValue::null());
    };
    let stringify = json.get(js_string!("stringify"), context)?;
    let parse = json.get(js_string!("parse"), context)?;
    let Some(stringify) = stringify.as_object() else {
        return Ok(JsValue::null());
    };
    let Some(parse) = parse.as_object() else {
        return Ok(JsValue::null());
    };
    let text = stringify.call(&json.clone().into(), &[value.clone()], context)?;
    if text.is_undefined() {
        return Ok(JsValue::null());
    }
    parse.call(&json.into(), &[text], context)
}

fn crypto_object(context: &mut Context) -> JsResult<JsObject> {
    let mut subtle = ObjectInitializer::new(context);
    subtle.function(
        NativeFunction::from_fn_ptr(subtle_digest),
        js_string!("digest"),
        2,
    );
    let subtle = subtle.build();
    let mut object = ObjectInitializer::new(context);
    object
        .function(
            NativeFunction::from_fn_ptr(crypto_random),
            js_string!("getRandomValues"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crypto_uuid),
            js_string!("randomUUID"),
            0,
        )
        .property(js_string!("subtle"), subtle, ATTR);
    Ok(object.build())
}

fn crypto_random(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = args.first().and_then(|value| value.as_object()) else {
        return Err(JsNativeError::typ()
            .with_message("getRandomValues expects a typed array")
            .into());
    };
    let length = object
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0) as usize;
    if length > 65_536 {
        return Err(JsNativeError::typ()
            .with_message("getRandomValues quota exceeded")
            .into());
    }
    let bytes = random_bytes(length);
    for (index, byte) in bytes.iter().enumerate() {
        object.set(index, JsValue::from(u32::from(*byte)), true, context)?;
    }
    Ok(args.first().cloned().unwrap_or(JsValue::undefined()))
}

fn crypto_uuid(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(js_string!(random_uuid()).into())
}

fn subtle_digest(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let algo = digest_name(args.first(), context)?;
    let bytes = bytes_of(args.get(1), context)?;
    let digest = match algo.as_str() {
        "SHA1" => sha1(&bytes).to_vec(),
        "SHA256" => sha256(&bytes).to_vec(),
        "SHA384" => sha512(&bytes, true).to_vec(),
        "SHA512" => sha512(&bytes, false).to_vec(),
        _ => {
            return reject_digest(
                context,
                "crypto.subtle.digest implements SHA-1, SHA-256, SHA-384, and SHA-512",
            );
        }
    };
    let array = uint8_array(&digest, context)?;
    Ok(JsPromise::resolve(array, context)?.into())
}

fn digest_name(value: Option<&JsValue>, context: &mut Context) -> JsResult<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    let name = if value.is_object()
        && let Some(object) = value.as_object()
    {
        object.get(js_string!("name"), context)?
    } else {
        value.clone()
    };
    Ok(name
        .to_string(context)?
        .to_std_string_lossy()
        .to_ascii_uppercase()
        .replace('-', ""))
}

fn reject_digest(context: &mut Context, message: &str) -> JsResult<JsValue> {
    let (promise, resolvers) = JsPromise::new_pending(context);
    resolvers.reject.call(
        &JsValue::undefined(),
        &[js_string!(message).into()],
        context,
    )?;
    Ok(promise.into())
}

fn bytes_of(value: Option<&JsValue>, context: &mut Context) -> JsResult<Vec<u8>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_string() {
        return Ok(value.to_string(context)?.to_std_string_lossy().into_bytes());
    }
    let Some(object) = value.as_object() else {
        return Ok(Vec::new());
    };
    let length = object
        .get(js_string!("length"), context)?
        .to_u32(context)
        .unwrap_or(0) as usize;
    let mut bytes = Vec::with_capacity(length);
    for index in 0..length {
        bytes.push(object.get(index, context)?.to_u32(context).unwrap_or(0) as u8);
    }
    Ok(bytes)
}

fn random_bytes(length: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; length];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if file.read_exact(&mut bytes).is_ok() {
            return bytes;
        }
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0xA5A5_A5A5);
    for byte in &mut bytes {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (state >> 33) as u8;
    }
    bytes
}

fn random_uuid() -> String {
    let mut bytes = random_bytes(16);
    if bytes.len() == 16 {
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
    }
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes.first().copied().unwrap_or(0),
        bytes.get(1).copied().unwrap_or(0),
        bytes.get(2).copied().unwrap_or(0),
        bytes.get(3).copied().unwrap_or(0),
        bytes.get(4).copied().unwrap_or(0),
        bytes.get(5).copied().unwrap_or(0),
        bytes.get(6).copied().unwrap_or(0),
        bytes.get(7).copied().unwrap_or(0),
        bytes.get(8).copied().unwrap_or(0),
        bytes.get(9).copied().unwrap_or(0),
        bytes.get(10).copied().unwrap_or(0),
        bytes.get(11).copied().unwrap_or(0),
        bytes.get(12).copied().unwrap_or(0),
        bytes.get(13).copied().unwrap_or(0),
        bytes.get(14).copied().unwrap_or(0),
        bytes.get(15).copied().unwrap_or(0),
    )
}

fn sha1(message: &[u8]) -> [u8; 20] {
    let mut hash: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    let bit_len = (message.len() as u64).saturating_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut words = [0u32; 80];
        for index in 0..16 {
            words[index] = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let mut a = hash[0];
        let mut b = hash[1];
        let mut c = hash[2];
        let mut d = hash[3];
        let mut e = hash[4];
        for index in 0..80 {
            let (f, k) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(words[index]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (index, word) in hash.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha512(message: &[u8], sha384: bool) -> Vec<u8> {
    const K: [u64; 80] = [
        0x428a2f98d728ae22,
        0x7137449123ef65cd,
        0xb5c0fbcfec4d3b2f,
        0xe9b5dba58189dbbc,
        0x3956c25bf348b538,
        0x59f111f1b605d019,
        0x923f82a4af194f9b,
        0xab1c5ed5da6d8118,
        0xd807aa98a3030242,
        0x12835b0145706fbe,
        0x243185be4ee4b28c,
        0x550c7dc3d5ffb4e2,
        0x72be5d74f27b896f,
        0x80deb1fe3b1696b1,
        0x9bdc06a725c71235,
        0xc19bf174cf692694,
        0xe49b69c19ef14ad2,
        0xefbe4786384f25e3,
        0x0fc19dc68b8cd5b5,
        0x240ca1cc77ac9c65,
        0x2de92c6f592b0275,
        0x4a7484aa6ea6e483,
        0x5cb0a9dcbd41fbd4,
        0x76f988da831153b5,
        0x983e5152ee66dfab,
        0xa831c66d2db43210,
        0xb00327c898fb213f,
        0xbf597fc7beef0ee4,
        0xc6e00bf33da88fc2,
        0xd5a79147930aa725,
        0x06ca6351e003826f,
        0x142929670a0e6e70,
        0x27b70a8546d22ffc,
        0x2e1b21385c26c926,
        0x4d2c6dfc5ac42aed,
        0x53380d139d95b3df,
        0x650a73548baf63de,
        0x766a0abb3c77b2a8,
        0x81c2c92e47edaee6,
        0x92722c851482353b,
        0xa2bfe8a14cf10364,
        0xa81a664bbc423001,
        0xc24b8b70d0f89791,
        0xc76c51a30654be30,
        0xd192e819d6ef5218,
        0xd69906245565a910,
        0xf40e35855771202a,
        0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8,
        0x1e376c085141ab53,
        0x2748774cdf8eeb99,
        0x34b0bcb5e19b48a8,
        0x391c0cb3c5c95a63,
        0x4ed8aa4ae3418acb,
        0x5b9cca4f7763e373,
        0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc,
        0x78a5636f43172f60,
        0x84c87814a1f0ab72,
        0x8cc702081a6439ec,
        0x90befffa23631e28,
        0xa4506cebde82bde9,
        0xbef9a3f7b2c67915,
        0xc67178f2e372532b,
        0xca273eceea26619c,
        0xd186b8c721c0c207,
        0xeada7dd6cde0eb1e,
        0xf57d4f7fee6ed178,
        0x06f067aa72176fba,
        0x0a637dc5a2c898a6,
        0x113f9804bef90dae,
        0x1b710b35131c471b,
        0x28db77f523047d84,
        0x32caab7b40c72493,
        0x3c9ebe0a15c9bebc,
        0x431d67c49c100d4c,
        0x4cc5d4becb3e42b6,
        0x597f299cfc657e2a,
        0x5fcb6fab3ad6faec,
        0x6c44198c4a475817,
    ];
    let mut hash: [u64; 8] = if sha384 {
        [
            0xcbbb9d5dc1059ed8,
            0x629a292a367cd507,
            0x9159015a3070dd17,
            0x152fecd8f70e5939,
            0x67332667ffc00b31,
            0x8eb44a8768581511,
            0xdb0c2e0d64f98fa7,
            0x47b5481dbefa4fa4,
        ]
    } else {
        [
            0x6a09e667f3bcc908,
            0xbb67ae8584caa73b,
            0x3c6ef372fe94f82b,
            0xa54ff53a5f1d36f1,
            0x510e527fade682d1,
            0x9b05688c2b3e6c1f,
            0x1f83d9abfb41bd6b,
            0x5be0cd19137e2179,
        ]
    };
    let bit_len = (message.len() as u128).saturating_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 128 != 112 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(128) {
        let mut words = [0u64; 80];
        for index in 0..16 {
            words[index] = u64::from_be_bytes(chunk[index * 8..index * 8 + 8].try_into().unwrap());
        }
        for index in 16..80 {
            let s0 = words[index - 15].rotate_right(1)
                ^ words[index - 15].rotate_right(8)
                ^ (words[index - 15] >> 7);
            let s1 = words[index - 2].rotate_right(19)
                ^ words[index - 2].rotate_right(61)
                ^ (words[index - 2] >> 6);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let mut a = hash[0];
        let mut b = hash[1];
        let mut c = hash[2];
        let mut d = hash[3];
        let mut e = hash[4];
        let mut f = hash[5];
        let mut g = hash[6];
        let mut h = hash[7];
        for index in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
        hash[5] = hash[5].wrapping_add(f);
        hash[6] = hash[6].wrapping_add(g);
        hash[7] = hash[7].wrapping_add(h);
    }
    let words = if sha384 { 6 } else { 8 };
    let mut out = Vec::with_capacity(words * 8);
    for word in hash.iter().take(words) {
        out.extend_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut hash: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (message.len() as u64).saturating_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut words = [0u32; 64];
        for index in 0..16 {
            words[index] = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let mut a = hash[0];
        let mut b = hash[1];
        let mut c = hash[2];
        let mut d = hash[3];
        let mut e = hash[4];
        let mut f = hash[5];
        let mut g = hash[6];
        let mut h = hash[7];
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
        hash[5] = hash[5].wrapping_add(f);
        hash[6] = hash[6].wrapping_add(g);
        hash[7] = hash[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (index, word) in hash.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{sha1, sha256, sha512};

    #[test]
    fn sha256_of_abc_matches_the_standard_vector() {
        let digest = sha256(b"abc");
        assert_eq!(
            digest,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }

    #[test]
    fn sha1_and_sha512_of_abc_match_the_standard_vectors() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
            ]
        );
        assert_eq!(
            sha512(b"abc", true),
            [
                0xcb, 0x00, 0x75, 0x3f, 0x45, 0xa3, 0x5e, 0x8b, 0xb5, 0xa0, 0x3d, 0x69, 0x9a, 0xc6,
                0x50, 0x07, 0x27, 0x2c, 0x32, 0xab, 0x0e, 0xde, 0xd1, 0x63, 0x1a, 0x8b, 0x60, 0x5a,
                0x43, 0xff, 0x5b, 0xed, 0x80, 0x86, 0x07, 0x2b, 0xa1, 0xe7, 0xcc, 0x23, 0x58, 0xba,
                0xec, 0xa1, 0x34, 0xc8, 0x25, 0xa7,
            ]
        );
    }
}
