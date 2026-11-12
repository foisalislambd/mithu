//! Host classes a page script expects to exist: `URL`, `TextEncoder`,
//! `XMLHttpRequest`, `MutationObserver`, and `history`.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::object::ObjectInitializer;
use boa_engine::object::builtins::{JsArray, JsPromise};
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
    context.register_global_class::<HtmlImage>()?;
    context.register_global_class::<HtmlElementHost>()?;
    context.register_global_class::<DocumentHost>()?;
    context.register_global_class::<DomEvent>()?;
    context.register_global_class::<CustomEventHost>()?;
    context.register_global_class::<MessageChannelHost>()?;
    context.register_global_class::<MessagePortHost>()?;
    let image = context.global_object().get(js_string!("Image"), context)?;
    context.register_global_property(js_string!("HTMLImageElement"), image, Attribute::all())?;
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
    #[unsafe_ignore_trace]
    asynchronous: bool,
    #[unsafe_ignore_trace]
    status: u16,
    #[unsafe_ignore_trace]
    body: String,
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

fn xhr_header(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn xhr_send(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("XMLHttpRequest method called on the wrong object")
    })?;
    let (method, url, asynchronous) = {
        let Some(xhr) = object.downcast_ref::<Xhr>() else {
            return Err(JsNativeError::typ()
                .with_message("XMLHttpRequest method called on the wrong object")
                .into());
        };
        (xhr.method.clone(), xhr.url.clone(), xhr.asynchronous)
    };
    let body = args
        .first()
        .filter(|value| !value.is_undefined() && !value.is_null())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_lossy())
        .unwrap_or_default();
    let (status, text) = xhr_roundtrip(&method, &url, &body, context);
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
    object.set(js_string!("response"), js_string!(text), true, context)?;
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

fn observer_observe(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(id) = args.first().and_then(crate::node_of) else {
        return Err(JsNativeError::typ()
            .with_message("MutationObserver.observe expects a node")
            .into());
    };
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("MutationObserver method called on the wrong object")
            .into());
    };
    object.set(
        js_string!("__target"),
        JsValue::from(id.0 as u32),
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
    let records = {
        let host = host(context)?;
        std::mem::take(&mut host.doc.borrow_mut().mutations)
    };
    if records.is_empty() {
        return Ok(());
    }
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
        let mine: Vec<_> = records
            .iter()
            .filter(|record| record.target.0 == target as usize)
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        let array = record_array(&mine, context)?;
        let callback = object.get(js_string!("callback"), context)?;
        if let Some(callback) = callback
            .as_object()
            .filter(|function| function.is_callable())
        {
            let _ = callback.call(&observer, &[array, observer.clone()], context);
        }
    }
    Ok(())
}

fn record_array(records: &[mithu_dom::DomMutation], context: &mut Context) -> JsResult<JsValue> {
    let mut values = Vec::new();
    for record in records {
        let attribute = match &record.attribute {
            Some(name) => js_string!(name.clone()).into(),
            None => JsValue::null(),
        };
        let target = crate::element_value(record.target.0, context);
        let mut item = ObjectInitializer::new(context);
        item.property(js_string!("type"), js_string!(record.kind.clone()), ATTR)
            .property(js_string!("attributeName"), attribute, ATTR)
            .property(js_string!("target"), target, ATTR);
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
struct HtmlElementHost;

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
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
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
    let mut event = ObjectInitializer::new(context);
    event.property(js_string!("data"), data, ATTR);
    let event = JsValue::from(event.build());
    let handler = peer_object.get(js_string!("onmessage"), context)?;
    if let Some(function) = handler
        .as_object()
        .filter(|function| function.is_callable())
    {
        let _ = function.call(&peer_value, &[event.clone()], context);
    }
    let listeners = peer_object.get(js_string!("__listeners"), context)?;
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
                let _ = function.call(&peer_value, &[event.clone()], context);
            }
        }
    }
    Ok(JsValue::undefined())
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
    let algo = arg_string(args, 0, context)?
        .to_ascii_uppercase()
        .replace('-', "");
    let bytes = bytes_of(args.get(1), context)?;
    let digest = if algo == "SHA256" {
        sha256(&bytes).to_vec()
    } else {
        return reject_digest(context, "crypto.subtle.digest only implements SHA-256");
    };
    let array = uint8_array(&digest, context)?;
    Ok(JsPromise::resolve(array, context)?.into())
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
    use super::sha256;

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
}
