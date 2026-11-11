//! Host classes a page script expects to exist: `URL`, `TextEncoder`,
//! `XMLHttpRequest`, `MutationObserver`, and `history`.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{
    Context, Finalize, JsData, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Trace,
    js_string,
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
        object.set(js_string!("origin"), js_string!(origin_of(&url)), true, context)?;
        object.set(
            js_string!("protocol"),
            js_string!(format!("{}:", url.scheme)),
            true,
            context,
        )?;
        object.set(js_string!("username"), js_string!(""), true, context)?;
        object.set(js_string!("password"), js_string!(""), true, context)?;
        object.set(js_string!("host"), js_string!(host_of(&url)), true, context)?;
        object.set(js_string!("hostname"), js_string!(url.host.clone()), true, context)?;
        object.set(js_string!("port"), js_string!(port_of(&url)), true, context)?;
        object.set(js_string!("pathname"), js_string!(url.path.clone()), true, context)?;
        object.set(js_string!("search"), js_string!(search), true, context)?;
        object.set(js_string!("hash"), js_string!(hash_of(&url)), true, context)?;
        object.set(js_string!("searchParams"), params, true, context)?;
        Ok(())
    }
}

fn url_to_string(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this
        .as_object()
        .and_then(|object| object.downcast_ref::<PageUrl>().map(|data| data.url.clone()))
        .ok_or_else(|| JsNativeError::typ().with_message("URL method called on the wrong object"))?;
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
        class.method(js_string!("get"), 1, NativeFunction::from_fn_ptr(params_get));
        class.method(js_string!("has"), 1, NativeFunction::from_fn_ptr(params_has));
        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(params_to_string),
        );
        class.method(js_string!("set"), 2, NativeFunction::from_fn_ptr(params_set));
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
        .and_then(|object| object.downcast_ref::<SearchParams>().map(|data| data.query.clone()))
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

fn params_to_string(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
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
    let Some(mut xhr) = object.downcast_mut::<Xhr>() else {
        return Err(JsNativeError::typ()
            .with_message("XMLHttpRequest method called on the wrong object")
            .into());
    };
    xhr.method = method;
    xhr.url = url;
    object.set(js_string!("readyState"), JsValue::from(1), true, context)?;
    Ok(JsValue::undefined())
}

fn xhr_header(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn xhr_send(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("XMLHttpRequest method called on the wrong object")
    })?;
    let (method, url) = {
        let Some(xhr) = object.downcast_ref::<Xhr>() else {
            return Err(JsNativeError::typ()
                .with_message("XMLHttpRequest method called on the wrong object")
                .into());
        };
        (xhr.method.clone(), xhr.url.clone())
    };
    let body = args
        .first()
        .filter(|value| !value.is_undefined() && !value.is_null())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_lossy())
        .unwrap_or_default();
    let (status, text) = xhr_roundtrip(&method, &url, &body, context);
    object.set(js_string!("status"), JsValue::from(u32::from(status)), true, context)?;
    object.set(
        js_string!("statusText"),
        js_string!(if (200..300).contains(&status) { "OK" } else { "" }),
        true,
        context,
    )?;
    object.set(js_string!("responseText"), js_string!(text.clone()), true, context)?;
    object.set(js_string!("response"), js_string!(text), true, context)?;
    object.set(js_string!("readyState"), JsValue::from(4), true, context)?;
    call_handler(&object, "onreadystatechange", context)?;
    if (200..300).contains(&status) || status == 0 {
        call_handler(&object, "onload", context)?;
    } else {
        call_handler(&object, "onerror", context)?;
    }
    Ok(JsValue::undefined())
}

fn call_handler(object: &JsObject, name: &str, context: &mut Context) -> JsResult<()> {
    let handler = object.get(js_string!(name), context)?;
    if let Some(function) = handler.as_object().filter(|function| function.is_callable()) {
        let _ = function.call(&object.clone().into(), &[], context);
    }
    Ok(())
}

fn xhr_roundtrip(method: &str, spec: &str, body: &str, context: &mut Context) -> (u16, String) {
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
    let result = if method == "POST" {
        mithu_net::post(&url, "application/x-www-form-urlencoded", body.as_bytes())
    } else if let Some(page) = initiator.as_ref() {
        mithu_net::get_from(&url, &page.host)
    } else {
        mithu_net::get(&url)
    };
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
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
        );
        class.method(
            js_string!("disconnect"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
        );
        class.method(
            js_string!("takeRecords"),
            0,
            NativeFunction::from_fn_ptr(|_, _, context| {
                Ok(boa_engine::object::builtins::JsArray::from_iter([], context).into())
            }),
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
}

fn history_object(context: &mut Context) -> JsResult<JsObject> {
    let length = native(context, "get length", 0, history_length);
    let mut object = ObjectInitializer::new(context);
    object
        .accessor(js_string!("length"), Some(length), None, ATTR)
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
        .function(NativeFunction::from_fn_ptr(history_back), js_string!("back"), 0)
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
    let next = resolve_request(&location, &spec).map_err(|error| {
        JsNativeError::typ().with_message(error)
    })?;
    Ok(Some(next.to_display()))
}

fn history_push(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(next) = history_url(args, context)? else {
        return Ok(JsValue::undefined());
    };
    let host = host(context)?;
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

fn history_replace(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(next) = history_url(args, context)? else {
        return Ok(JsValue::undefined());
    };
    let host = host(context)?;
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
