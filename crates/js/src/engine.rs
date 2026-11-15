//! SpiderMonkey embedding for the page.
//!
//! DOM objects are reflectors: a `JSObject` plus Rust data kept alive for the
//! realm and traced by a root while any host handle exists. Microtasks drain
//! through SpiderMonkey's internal [`JS::JobQueue`] (`UseInternalJobQueues` /
//! `RunJobs`). Modules load through the `HostLoadImportedModule` hook
//! (`SetModuleLoadHook`) and `import.meta` through the module metadata hook.
//! Unhandled rejections go through `SetPromiseRejectionTrackerCallback`.
//!
//! The types in this module match the host-binding calls the DOM layer already
//! makes, so those calls talk to SpiderMonkey directly.

use std::any::{Any, TypeId};
use std::sync::Once;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::future::Future;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll};

use mozjs::context::JSContext;
use mozjs::jsapi::{self, JSClass, JSClassOps, JSContext as RawJSContext, JSObject, JSString, OnNewGlobalHookOption, Value};
use mozjs::jsval::{self, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{JSEngine, RealmOptions, SIMPLE_GLOBAL_CLASS, transform_str_to_source_text, CompileOptionsWrapper};

thread_local! {
    static CURRENT: Cell<*mut Context> = const { Cell::new(ptr::null_mut()) };
    /// SpiderMonkey allows one `JSContext` per thread. Pages and tests share
    /// it and each keep their own global.
    static RUNTIME: RefCell<Option<ThreadRuntime>> = const { RefCell::new(None) };
}

struct ThreadRuntime {
    raw: *mut RawJSContext,
    live: usize,
}

struct JsFrame {
    previous: *mut Context,
    realm: *mut jsapi::Realm,
    entered: bool,
}

fn enter(cx: *mut Context) -> JsFrame {
    let previous = CURRENT.with(|slot| {
        let previous = slot.get();
        slot.set(cx);
        previous
    });
    let (realm, entered) = if cx.is_null() {
        (ptr::null_mut(), false)
    } else {
        let context = unsafe { &*cx };
        match context.global.as_ref() {
            Some(global) if !global.ptr.is_null() => {
                let realm = unsafe { wrappers2::EnterRealm(&context.cx, global.ptr) };
                (realm, true)
            }
            _ => (ptr::null_mut(), false),
        }
    };
    JsFrame {
        previous,
        realm,
        entered,
    }
}

fn leave(frame: JsFrame) {
    if frame.entered {
        let cx = CURRENT.with(|slot| slot.get());
        if !cx.is_null() {
            let context = unsafe { &*cx };
            unsafe { wrappers2::LeaveRealm(&context.cx, frame.realm) };
        }
    }
    CURRENT.with(|slot| slot.set(frame.previous));
}

struct Entered(JsFrame);

/// Keeps this context current so reflector helpers can find it.
pub struct ContextBind(JsFrame);

impl Context {
    pub fn bind(&mut self) -> ContextBind {
        ContextBind(enter(self))
    }
}

impl Drop for ContextBind {
    fn drop(&mut self) {
        leave(std::mem::replace(
            &mut self.0,
            JsFrame {
                previous: ptr::null_mut(),
                realm: ptr::null_mut(),
                entered: false,
            },
        ));
    }
}
impl Entered {
    fn new(cx: *mut Context) -> Self {
        Self(enter(cx))
    }
}
impl Drop for Entered {
    fn drop(&mut self) {
        leave(std::mem::replace(
            &mut self.0,
            JsFrame {
                previous: ptr::null_mut(),
                realm: ptr::null_mut(),
                entered: false,
            },
        ));
    }
}

/// Long-lived roots live in a side table traced by an extra GC root tracer.
/// Stack `Rooted` values must be strictly nested, and these handles outlive
/// the temporaries that create them.
struct Root {
    value: Value,
    index: Rc<Cell<usize>>,
}

struct LiveRoot {
    value: Value,
    index: Rc<Cell<usize>>,
}

thread_local! {
    static LIVE_ROOTS: RefCell<Vec<LiveRoot>> = const { RefCell::new(Vec::new()) };
}

impl Root {
    fn new(value: Value) -> Self {
        LIVE_ROOTS.with(|live| {
            let mut live = live.borrow_mut();
            let index = Rc::new(Cell::new(live.len()));
            live.push(LiveRoot {
                value,
                index: Rc::clone(&index),
            });
            Self { value, index }
        })
    }

    fn value(&self) -> Value {
        self.value
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        LIVE_ROOTS.with(|live| {
            let mut live = live.borrow_mut();
            let index = self.index.get();
            if index < live.len() {
                live.swap_remove(index);
                if index < live.len() {
                    live[index].index.set(index);
                }
            }
        });
    }
}

unsafe extern "C" fn trace_live_roots(tracer: *mut jsapi::JSTracer, _data: *mut std::ffi::c_void) {
    LIVE_ROOTS.with(|live| {
        for root in live.borrow().iter() {
            unsafe { mozjs::gc::CustomTrace::trace(&root.value, tracer) };
        }
    });
}

struct NativeSlot {
    type_id: TypeId,
    data: UnsafeCell<Box<dyn Any>>,
}

pub struct Erased;

/// A host handle to a JavaScript object. `T` is the reflector's Rust data when
/// the object was created as a class instance.
pub struct JsObject<T: 'static = Erased> {
    ptr: *mut JSObject,
    root: ArcRoot,
    native: Option<Rc<NativeSlot>>,
    _marker: PhantomData<T>,
}

struct ArcRoot(Rc<Root>);

impl Clone for ArcRoot {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T: 'static> Clone for JsObject<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            root: self.root.clone(),
            native: self.native.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T: 'static> JsObject<T> {
    fn from_parts(ptr: *mut JSObject, root: ArcRoot, native: Option<Rc<NativeSlot>>) -> Self {
        Self {
            ptr,
            root,
            native,
            _marker: PhantomData,
        }
    }

    pub fn upcast(self) -> JsObject {
        JsObject {
            ptr: self.ptr,
            root: self.root,
            native: self.native,
            _marker: PhantomData,
        }
    }

    pub fn set_prototype(&self, proto: Option<JsObject>) {
        let Some(cx) = current() else {
            return;
        };
        unsafe { (*cx).set_proto(self.ptr, proto.as_ref().map(|proto| proto.ptr)) };
    }

    /// Boa's typed object exposes its reflector through `as_ref().borrow().data()`.
    pub fn as_ref(&self) -> TypedSlot<T> {
        TypedSlot {
            slot: self.native.clone(),
            _marker: PhantomData,
        }
    }
}

pub struct TypedSlot<T: 'static> {
    slot: Option<Rc<NativeSlot>>,
    _marker: PhantomData<T>,
}

impl<T: 'static> TypedSlot<T> {
    pub fn borrow(&self) -> TypedGuard<'_, T> {
        TypedGuard {
            slot: self.slot.as_deref(),
            _marker: PhantomData,
        }
    }
}

pub struct TypedGuard<'a, T: 'static> {
    slot: Option<&'a NativeSlot>,
    _marker: PhantomData<T>,
}

impl<'a, T: 'static> TypedGuard<'a, T> {
    pub fn data(&self) -> &T {
        self.slot
            .and_then(|slot| {
                let any = unsafe { &*slot.data.get() };
                any.downcast_ref::<T>()
            })
            .expect("reflector data")
    }
}

impl JsObject {
    pub fn get(&self, key: impl PropKeySpec, context: &mut Context) -> JsResult<JsValue> {
        context.get_prop(self.ptr, &key.as_key())
    }

    pub fn set(
        &self,
        key: impl PropKeySpec,
        value: impl Into<JsValue>,
        _strict: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        context.set_prop(self.ptr, &key.as_key(), &value.into())
    }

    pub fn call(
        &self,
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        context.call_value(self.ptr, this, args, false, None)
    }

    pub fn construct(
        &self,
        args: &[JsValue],
        new_target: Option<&JsObject>,
        context: &mut Context,
    ) -> JsResult<JsObject> {
        context.construct_value(self.ptr, args, new_target.map(|object| object.ptr))
    }

    pub fn is_callable(&self) -> bool {
        let Some(cx) = current() else {
            return false;
        };
        unsafe { (*cx).value_is_callable(jsval::ObjectValue(self.ptr)) }
    }

    pub fn is_constructor(&self) -> bool {
        self.is_callable()
    }

    pub fn is_array(&self) -> bool {
        let Some(cx) = current() else {
            return false;
        };
        unsafe { (*cx).object_is_array(self.ptr) }
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        let slot = self.native.as_ref()?;
        if slot.type_id != TypeId::of::<T>() {
            return None;
        }
        let any = unsafe { &*slot.data.get() };
        any.downcast_ref::<T>()
    }

    pub fn downcast_mut<T: 'static>(&self) -> Option<&mut T> {
        let slot = self.native.as_ref()?;
        if slot.type_id != TypeId::of::<T>() {
            return None;
        }
        let any = unsafe { &mut *slot.data.get() };
        any.downcast_mut::<T>()
    }

    pub fn define_property_or_throw(
        &self,
        key: impl PropKeySpec,
        descriptor: property::PropertyDescriptor,
        context: &mut Context,
    ) -> JsResult<bool> {
        if let Some(value) = descriptor.value.as_ref() {
            context.define_data(self.ptr, &key.as_key(), value, &descriptor)?;
            return Ok(true);
        }
        context.define_accessor(self.ptr, &key.as_key(), &descriptor)
    }
}

#[derive(Clone)]
pub struct JsString {
    text: String,
}

impl JsString {
    pub fn to_std_string_lossy(&self) -> String {
        self.text.clone()
    }

    pub fn to_std_string_escaped(&self) -> String {
        self.text.clone()
    }
}

impl From<&str> for JsString {
    fn from(value: &str) -> Self {
        Self { text: value.to_string() }
    }
}

impl From<String> for JsString {
    fn from(value: String) -> Self {
        Self { text: value }
    }
}

impl From<&String> for JsString {
    fn from(value: &String) -> Self {
        Self { text: value.clone() }
    }
}

impl std::fmt::Display for JsString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.fmt(formatter)
    }
}

#[derive(Clone)]
pub struct JsValue {
    kind: ValueKind,
}

#[derive(Clone)]
enum ValueKind {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Object(JsObject),
}

impl JsValue {
    pub fn undefined() -> Self {
        Self { kind: ValueKind::Undefined }
    }

    pub fn null() -> Self {
        Self { kind: ValueKind::Null }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self.kind, ValueKind::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self.kind, ValueKind::Null)
    }

    pub fn is_null_or_undefined(&self) -> bool {
        matches!(self.kind, ValueKind::Null | ValueKind::Undefined)
    }

    pub fn to_boolean(&self) -> bool {
        match &self.kind {
            ValueKind::Undefined | ValueKind::Null => false,
            ValueKind::Bool(value) => *value,
            ValueKind::Number(value) => *value != 0.0 && !value.is_nan(),
            ValueKind::String(text) => !text.is_empty(),
            ValueKind::Object(_) => true,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self.kind, ValueKind::Object(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self.kind, ValueKind::String(_))
    }

    pub fn is_boolean(&self) -> bool {
        matches!(self.kind, ValueKind::Bool(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(self.kind, ValueKind::Number(_))
    }

    pub fn to_number(&self, context: &mut Context) -> JsResult<f64> {
        match &self.kind {
            ValueKind::Undefined => Ok(f64::NAN),
            ValueKind::Null | ValueKind::Bool(false) => Ok(0.0),
            ValueKind::Bool(true) => Ok(1.0),
            ValueKind::Number(value) => Ok(*value),
            ValueKind::String(text) => Ok(text.trim().parse().unwrap_or(f64::NAN)),
            ValueKind::Object(_) => context.to_number_value(self),
        }
    }

    pub fn to_u32(&self, context: &mut Context) -> JsResult<u32> {
        match &self.kind {
            ValueKind::Number(value) => Ok(number_to_u32(*value)),
            ValueKind::Bool(value) => Ok(u32::from(*value)),
            ValueKind::Null | ValueKind::Undefined => Ok(0),
            ValueKind::String(text) => Ok(number_to_u32(text.trim().parse().unwrap_or(f64::NAN))),
            ValueKind::Object(_) => context.to_u32_value(self),
        }
    }

    pub fn is_callable(&self) -> bool {
        self.as_object().is_some_and(|object| object.is_callable())
    }

    pub fn as_object(&self) -> Option<JsObject> {
        match &self.kind {
            ValueKind::Object(object) => Some(object.clone()),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<JsString> {
        match &self.kind {
            ValueKind::String(text) => Some(JsString { text: text.clone() }),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match &self.kind {
            ValueKind::Bool(value) => Some(*value),
            ValueKind::Number(value) => Some(*value != 0.0 && !value.is_nan()),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match &self.kind {
            ValueKind::Number(value) => Some(*value),
            ValueKind::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_constructor(&self) -> Option<JsObject> {
        self.as_object().filter(|object| object.is_constructor())
    }

    pub fn to_string(&self, context: &mut Context) -> JsResult<JsString> {
        if let ValueKind::String(text) = &self.kind {
            return Ok(JsString { text: text.clone() });
        }
        Ok(JsString {
            text: context.stringify(self),
        })
    }

    pub fn display(&self) -> DisplayValue {
        DisplayValue(match &self.kind {
            ValueKind::Undefined => "undefined".to_string(),
            ValueKind::Null => "null".to_string(),
            ValueKind::Bool(value) => value.to_string(),
            ValueKind::Number(value) => value.to_string(),
            ValueKind::String(text) => text.clone(),
            ValueKind::Object(_) => current()
                .map(|cx| unsafe { (*cx).stringify(self) })
                .unwrap_or_else(|| "[object Object]".to_string()),
        })
    }
}

pub struct DisplayValue(String);

impl std::fmt::Display for DisplayValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (ValueKind::Undefined, ValueKind::Undefined) | (ValueKind::Null, ValueKind::Null) => true,
            (ValueKind::Bool(left), ValueKind::Bool(right)) => left == right,
            (ValueKind::Number(left), ValueKind::Number(right)) => left == right,
            (ValueKind::String(left), ValueKind::String(right)) => left == right,
            (ValueKind::Object(left), ValueKind::Object(right)) => left.ptr == right.ptr,
            _ => false,
        }
    }
}

impl From<JsObject> for JsValue {
    fn from(object: JsObject) -> Self {
        Self { kind: ValueKind::Object(object) }
    }
}

impl From<JsString> for JsValue {
    fn from(value: JsString) -> Self {
        Self { kind: ValueKind::String(value.text) }
    }
}

impl From<bool> for JsValue {
    fn from(value: bool) -> Self {
        Self { kind: ValueKind::Bool(value) }
    }
}

impl From<i32> for JsValue {
    fn from(value: i32) -> Self {
        Self { kind: ValueKind::Number(value as f64) }
    }
}

impl From<u32> for JsValue {
    fn from(value: u32) -> Self {
        Self { kind: ValueKind::Number(value as f64) }
    }
}

impl From<f64> for JsValue {
    fn from(value: f64) -> Self {
        Self { kind: ValueKind::Number(value) }
    }
}

impl From<usize> for JsValue {
    fn from(value: usize) -> Self {
        Self { kind: ValueKind::Number(value as f64) }
    }
}

pub type JsResult<T> = Result<T, JsError>;

#[derive(Debug)]
pub struct JsError {
    message: String,
}

impl JsError {
    pub fn from_opaque(value: JsValue) -> Self {
        Self {
            message: value.display().to_string(),
        }
    }
}

impl std::fmt::Display for JsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for JsError {}

pub struct JsNativeError {
    message: String,
}

impl JsNativeError {
    pub fn typ() -> Self {
        Self { message: "TypeError".to_string() }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }
}

impl From<JsNativeError> for JsError {
    fn from(error: JsNativeError) -> Self {
        JsError { message: error.message }
    }
}

#[derive(Clone, Copy)]
pub struct NativeFunction(pub fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>);

impl NativeFunction {
    pub fn from_fn_ptr(body: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>) -> Self {
        Self(body)
    }
}

pub enum PropKey {
    Name(String),
    Index(u32),
}

pub trait PropKeySpec {
    fn as_key(&self) -> PropKey;
}

impl PropKeySpec for PropKey {
    fn as_key(&self) -> PropKey {
        match self {
            PropKey::Name(name) => PropKey::Name(name.clone()),
            PropKey::Index(index) => PropKey::Index(*index),
        }
    }
}

impl PropKeySpec for JsString {
    fn as_key(&self) -> PropKey {
        PropKey::Name(self.text.clone())
    }
}

impl PropKeySpec for &str {
    fn as_key(&self) -> PropKey {
        PropKey::Name((*self).to_string())
    }
}

impl PropKeySpec for String {
    fn as_key(&self) -> PropKey {
        PropKey::Name(self.clone())
    }
}

impl PropKeySpec for u32 {
    fn as_key(&self) -> PropKey {
        PropKey::Index(*self)
    }
}

impl PropKeySpec for usize {
    fn as_key(&self) -> PropKey {
        PropKey::Index(*self as u32)
    }
}

impl PropKeySpec for i32 {
    fn as_key(&self) -> PropKey {
        PropKey::Index(*self as u32)
    }
}

pub mod property {
    use super::JsValue;

    #[derive(Clone, Copy)]
    pub struct Attribute(u8);

    impl Attribute {
        pub const WRITABLE: Self = Self(0b001);
        pub const ENUMERABLE: Self = Self(0b010);
        pub const CONFIGURABLE: Self = Self(0b100);

        pub const fn union(self, other: Self) -> Self {
            Self(self.0 | other.0)
        }

        pub fn all() -> Self {
            Self::WRITABLE.union(Self::ENUMERABLE).union(Self::CONFIGURABLE)
        }

        pub(crate) fn js_attrs(self) -> u32 {
            let mut attrs = 0u32;
            if self.0 & 0b010 != 0 {
                attrs |= 0x01;
            }
            if self.0 & 0b001 == 0 {
                attrs |= 0x02;
            }
            if self.0 & 0b100 == 0 {
                attrs |= 0x04;
            }
            attrs
        }
    }

    pub struct PropertyDescriptor {
        pub value: Option<JsValue>,
        pub get: Option<JsValue>,
        pub set: Option<JsValue>,
        pub enumerable: bool,
        pub configurable: bool,
        pub writable: bool,
    }

    impl PropertyDescriptor {
        pub fn builder() -> Self {
            Self {
                value: None,
                get: None,
                set: None,
                enumerable: false,
                configurable: false,
                writable: false,
            }
        }

        pub fn value(mut self, value: impl Into<JsValue>) -> Self {
            self.value = Some(value.into());
            self
        }

        pub fn writable(mut self, value: bool) -> Self {
            self.writable = value;
            self
        }

        pub fn get(mut self, value: JsValue) -> Self {
            self.get = Some(value);
            self
        }

        pub fn maybe_set(mut self, value: Option<JsValue>) -> Self {
            self.set = value;
            self
        }

        pub fn enumerable(mut self, value: bool) -> Self {
            self.enumerable = value;
            self
        }

        pub fn configurable(mut self, value: bool) -> Self {
            self.configurable = value;
            self
        }
    }
}

pub use property::Attribute;

pub mod object {
    use super::{Context, JsFunction, JsObject, JsResult, JsString, JsValue, NativeFunction, PropKeySpec};

    pub struct ObjectInitializer<'a> {
        context: &'a mut Context,
        object: JsObject,
    }

    impl<'a> ObjectInitializer<'a> {
        pub fn new(context: &'a mut Context) -> Self {
            let object = context.new_object(None);
            Self { context, object }
        }

        pub fn with_native_data<T: 'static>(data: T, context: &'a mut Context) -> Self {
            let object = context.new_reflector(data).upcast();
            Self { context, object }
        }

        pub fn function(
            &mut self,
            native: NativeFunction,
            name: JsString,
            length: usize,
        ) -> &mut Self {
            let function = self.context.new_function(native, &name.text, length, false);
            let _ = self.context.set_prop(
                self.object.ptr,
                &name.as_key(),
                &JsValue::from(function),
            );
            self
        }

        pub fn property(
            &mut self,
            name: JsString,
            value: impl Into<JsValue>,
            _attribute: super::Attribute,
        ) -> &mut Self {
            let _ = self
                .context
                .set_prop(self.object.ptr, &name.as_key(), &value.into());
            self
        }

        pub fn accessor(
            &mut self,
            name: JsString,
            get: Option<JsFunction>,
            set: Option<JsFunction>,
            attribute: super::Attribute,
        ) -> &mut Self {
            let descriptor = super::property::PropertyDescriptor {
                value: None,
                get: get.map(JsValue::from),
                set: set.map(JsValue::from),
                enumerable: attribute.js_attrs() & 0x01 != 0,
                configurable: attribute.js_attrs() & 0x04 == 0,
                writable: false,
            };
            let _ = self
                .context
                .define_accessor(self.object.ptr, &name.as_key(), &descriptor);
            self
        }

        pub fn build(&mut self) -> JsObject {
            self.object.clone()
        }
    }

    pub struct FunctionObjectBuilder {
        context: *mut Context,
        native: NativeFunction,
        name: String,
        length: usize,
        constructor: bool,
    }

    impl FunctionObjectBuilder {
        pub fn new(realm: super::Realm, native: NativeFunction) -> Self {
            Self {
                context: realm.0,
                native,
                name: "native".to_string(),
                length: 0,
                constructor: false,
            }
        }

        pub fn name(mut self, name: JsString) -> Self {
            self.name = name.text;
            self
        }

        pub fn length(mut self, length: usize) -> Self {
            self.length = length;
            self
        }

        pub fn constructor(mut self, constructor: bool) -> Self {
            self.constructor = constructor;
            self
        }

        pub fn build(self) -> JsFunction {
            unsafe { (*self.context).new_function(self.native, &self.name, self.length, self.constructor) }
        }
    }

    pub mod builtins {
        use super::super::{Context, JsObject, JsResult, JsValue};

        #[derive(Clone)]
        pub struct JsArray(pub JsObject);

        impl JsArray {
            pub fn new(context: &mut Context) -> JsResult<Self> {
                Ok(Self(context.new_array(0)))
            }

            pub fn from_iter(values: impl IntoIterator<Item = JsValue>, context: &mut Context) -> Self {
                let values: Vec<JsValue> = values.into_iter().collect();
                let array = context.new_array(values.len() as u32);
                for (index, value) in values.into_iter().enumerate() {
                    let _ = context.set_prop(array.ptr, &super::super::PropKey::Index(index as u32), &value);
                }
                Self(array)
            }
        }

        impl From<JsArray> for JsObject {
            fn from(array: JsArray) -> Self {
                array.0
            }
        }

        impl From<JsArray> for JsValue {
            fn from(array: JsArray) -> Self {
                JsValue::from(array.0)
            }
        }

        #[derive(Clone)]
        pub struct JsFunction(pub JsObject);

        impl std::ops::Deref for JsFunction {
            type Target = JsObject;
            fn deref(&self) -> &JsObject {
                &self.0
            }
        }

        impl From<JsFunction> for JsValue {
            fn from(function: JsFunction) -> Self {
                JsValue::from(function.0)
            }
        }

        impl From<JsFunction> for JsObject {
            fn from(function: JsFunction) -> Self {
                function.0
            }
        }

        #[derive(Clone)]
        pub struct JsPromise {
            pub object: JsObject,
        }

        impl JsPromise {
            pub fn resolve(value: impl Into<JsValue>, context: &mut Context) -> JsResult<Self> {
                context.promise_resolve(value.into())
            }

            pub fn new_pending(context: &mut Context) -> (Self, Resolvers) {
                context.promise_pending()
            }

            pub fn state(&self) -> super::super::builtins::promise::PromiseState {
                let Some(cx) = super::super::current() else {
                    return super::super::builtins::promise::PromiseState::Pending;
                };
                unsafe { (*cx).promise_state(&self.object) }
            }
        }

        impl From<JsPromise> for JsValue {
            fn from(promise: JsPromise) -> Self {
                JsValue::from(promise.object)
            }
        }

        pub struct Resolvers {
            pub resolve: JsObject,
            pub reject: JsObject,
        }
    }
}

pub use object::builtins::{JsArray, JsFunction, JsPromise};

#[derive(Clone, Copy)]
pub struct Realm(*mut Context);

impl Context {
    pub fn realm(&mut self) -> Realm {
        Realm(self as *mut Context)
    }
}


pub mod script {
    use std::future::Future;
    use std::pin::Pin;

    use super::{Context, JsResult, JsValue, Source};

    pub struct Script {
        source: String,
        path: String,
    }

    impl Script {
        pub fn parse(source: Source, _realm: Option<&super::Realm>, context: &mut Context) -> JsResult<Self> {
            context.compile_script(&source.code, &source.path)?;
            Ok(Self {
                source: source.code,
                path: source.path,
            })
        }

        pub fn evaluate(&self, context: &mut Context) -> JsResult<JsValue> {
            context.eval_source(&self.source, &self.path)
        }

        pub fn evaluate_async_with_budget<'a>(
            &'a self,
            context: &'a mut Context,
            _budget: u32,
        ) -> EvalFuture<'a> {
            EvalFuture {
                context,
                source: &self.source,
                path: &self.path,
                started: false,
            }
        }
    }

    pub struct EvalFuture<'a> {
        context: &'a mut Context,
        source: &'a str,
        path: &'a str,
        started: bool,
    }

    impl Future for EvalFuture<'_> {
        type Output = JsResult<JsValue>;

        fn poll(self: Pin<&mut Self>, _task: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
            let this = self.get_mut();
            if !this.started {
                this.started = true;
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(this.context.eval_source(this.source, this.path))
        }
    }
}

pub struct Source {
    code: String,
    path: String,
}

impl Source {
    pub fn from_bytes(code: &str) -> Self {
        Self {
            code: code.to_string(),
            path: "inline.js".to_string(),
        }
    }

    pub fn with_path(mut self, path: &Path) -> Self {
        self.path = path.display().to_string();
        self
    }
}

pub mod module {
    use super::{Context, JsObject, JsResult, JsString, Path, PathBuf};

    pub struct Module {
        record: JsObject,
        path: PathBuf,
    }

    impl Clone for Module {
        fn clone(&self) -> Self {
            Self {
                record: self.record.clone(),
                path: self.path.clone(),
            }
        }
    }

    impl Module {
        pub fn parse(source: super::Source, _realm: Option<&super::Realm>, context: &mut Context) -> JsResult<Self> {
            context.compile_module(&source.code, &source.path)
        }

        pub fn path(&self) -> Option<&Path> {
            Some(self.path.as_path())
        }

        pub fn load_link_evaluate(&self, context: &mut Context) -> super::JsPromise {
            context.link_and_evaluate(self)
        }

        pub(crate) fn record(&self) -> &JsObject {
            &self.record
        }

        pub(crate) fn from_record(record: JsObject, path: PathBuf) -> Self {
            Self { record, path }
        }
    }

    pub struct Referrer {
        path: Option<PathBuf>,
    }

    impl Referrer {
        pub fn path(&self) -> Option<&Path> {
            self.path.as_deref()
        }

        pub(crate) fn new(path: Option<PathBuf>) -> Self {
            Self { path }
        }
    }

    pub struct ModuleRequest {
        specifier: JsString,
    }

    impl ModuleRequest {
        pub fn specifier(&self) -> &JsString {
            &self.specifier
        }

        pub(crate) fn new(specifier: String) -> Self {
            Self {
                specifier: JsString::from(specifier),
            }
        }
    }

    pub trait ModuleLoader {
        fn init_import_meta(&self, import_meta: &JsObject, module: &Module, context: &mut Context);
        fn load_imported_module(
            &self,
            referrer: Referrer,
            request: ModuleRequest,
            context: &mut Context,
        ) -> JsResult<Module>;
    }

    pub(crate) struct ModuleRecord;
}

pub use module::{Module, ModuleLoader, ModuleRequest, Referrer};

pub mod context {
    use super::{JsPromise, JsValue, OperationType};

    pub trait HostHooks {
        fn promise_rejection_tracker(
            &self,
            promise: &JsPromise,
            operation: OperationType,
            context: &mut super::Context,
        );
    }
}

pub use context::HostHooks;

pub mod builtins {
    pub mod promise {
        use super::super::JsValue;

        #[derive(Clone)]
        pub enum PromiseState {
            Pending,
            Fulfilled(Placeholder),
            Rejected(JsValue),
        }

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct Placeholder;

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum OperationType {
            Reject,
            Handle,
        }
    }
}

pub use builtins::promise::OperationType;

pub mod vm {
    pub struct RuntimeLimits {
        recursion: u32,
    }

    impl Default for RuntimeLimits {
        fn default() -> Self {
            Self { recursion: 1_536 }
        }
    }

    impl RuntimeLimits {
        pub fn set_stack_size_limit(&mut self, _limit: usize) {}

        pub fn set_recursion_limit(&mut self, limit: u32) {
            self.recursion = limit;
        }
    }
}

pub mod class {
    use super::{Context, JsFunction, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, PropKeySpec};

    pub trait Class: Sized + 'static {
        const NAME: &'static str;
        const LENGTH: usize = 0;

        fn init(class: &mut ClassBuilder<'_>) -> JsResult<()>;

        fn data_constructor(
            new_target: &JsValue,
            args: &[JsValue],
            context: &mut Context,
        ) -> JsResult<Self>;

        fn object_constructor(
            _instance: &JsObject<Self>,
            _args: &[JsValue],
            _context: &mut Context,
        ) -> JsResult<()> {
            Ok(())
        }

        fn construct(
            new_target: &JsValue,
            args: &[JsValue],
            context: &mut Context,
        ) -> JsResult<JsObject> {
            if new_target.is_undefined() {
                return Err(JsNativeError::typ()
                    .with_message(format!(
                        "cannot call constructor of native class `{}` without new",
                        Self::NAME
                    ))
                    .into());
            }
            let data = Self::data_constructor(new_target, args, context)?;
            let object = context.new_reflector(data);
            if let Some(constructor) = new_target.as_object()
                && let Ok(proto) = constructor.get(JsString::from("prototype"), context)
                && let Some(proto) = proto.as_object()
            {
                object.set_prototype(Some(proto));
            }
            Self::object_constructor(&object, args, context)?;
            Ok(object.upcast())
        }

        fn from_data(data: Self, context: &mut Context) -> JsResult<JsObject> {
            let object = context.new_reflector(data);
            if let Some(info) = context.class_proto::<Self>() {
                object.set_prototype(Some(info));
            }
            Self::object_constructor(&object, &[], context)?;
            Ok(object.upcast())
        }
    }

    pub struct ClassBuilder<'a> {
        context: &'a mut Context,
        prototype: JsObject,
        constructor: JsObject,
    }

    impl<'a> ClassBuilder<'a> {
        pub fn method(
            &mut self,
            name: JsString,
            length: usize,
            function: NativeFunction,
        ) -> &mut Self {
            let function = self.context.new_function(function, &name.text, length, false);
            let descriptor = super::property::PropertyDescriptor::builder()
                .value(JsValue::from(function))
                .writable(true)
                .enumerable(false)
                .configurable(true);
            let _ = self.context.define_data(
                self.prototype.ptr,
                &name.as_key(),
                descriptor.value.as_ref().unwrap(),
                &descriptor,
            );
            self
        }

        pub fn static_method(
            &mut self,
            name: JsString,
            length: usize,
            function: NativeFunction,
        ) -> &mut Self {
            let function = self.context.new_function(function, &name.text, length, false);
            let _ = self.context.set_prop(
                self.constructor.ptr,
                &name.as_key(),
                &JsValue::from(function),
            );
            self
        }

        pub fn property(
            &mut self,
            name: JsString,
            value: impl Into<JsValue>,
            _attribute: super::Attribute,
        ) -> &mut Self {
            let _ = self.context.set_prop(
                self.prototype.ptr,
                &name.as_key(),
                &value.into(),
            );
            self
        }

        pub fn static_property(
            &mut self,
            name: JsString,
            value: impl Into<JsValue>,
            _attribute: super::Attribute,
        ) -> &mut Self {
            let _ = self.context.set_prop(
                self.constructor.ptr,
                &name.as_key(),
                &value.into(),
            );
            self
        }

        pub fn accessor(
            &mut self,
            name: JsString,
            get: Option<JsFunction>,
            set: Option<JsFunction>,
            attribute: super::Attribute,
        ) -> &mut Self {
            let descriptor = super::property::PropertyDescriptor {
                value: None,
                get: get.map(JsValue::from),
                set: set.map(JsValue::from),
                enumerable: attribute.js_attrs() & 0x01 != 0,
                configurable: attribute.js_attrs() & 0x04 == 0,
                writable: false,
            };
            let _ = self
                .context
                .define_accessor(self.prototype.ptr, &name.as_key(), &descriptor);
            self
        }

        pub fn context(&mut self) -> &mut Context {
            self.context
        }
    }

    pub(crate) fn build_class<T: Class>(context: &mut Context) -> JsResult<JsObject> {
        let prototype = context.new_object(None);
        let constructor = context.new_function(
            NativeFunction::from_fn_ptr(construct_class::<T>),
            T::NAME,
            T::LENGTH,
            true,
        );
        {
            let _entered = super::Entered::new(context as *mut Context);
            unsafe {
                mozjs::rooted!(in(context.raw) let ctor = constructor.ptr);
                mozjs::rooted!(in(context.raw) let proto = prototype.ptr);
                let linked = mozjs::rust::wrappers2::JS_LinkConstructorAndPrototype(
                    context.js(),
                    ctor.handle().into(),
                    proto.handle().into(),
                );
                if !linked {
                    return Err(context.take_exception());
                }
            }
        }
        context.remember_class::<T>(prototype.clone());
        let mut builder = ClassBuilder {
            context,
            prototype,
            constructor: constructor.0.clone(),
        };
        T::init(&mut builder)?;
        Ok(constructor.0)
    }

    fn construct_class<T: Class>(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        let target = if context.constructing() {
            context.new_target().unwrap_or_else(|| this.clone())
        } else {
            JsValue::undefined()
        };
        T::construct(&target, args, context).map(JsValue::from)
    }

    struct PropKeyName;
    impl PropKeyName {
        fn name(name: &str) -> super::PropKey {
            super::PropKey::Name(name.to_string())
        }
    }
}

pub use class::{Class, ClassBuilder};

struct ClassProto {
    prototype: JsObject,
}

fn ensure_engine() {
    static START: Once = Once::new();
    START.call_once(|| {
        let engine = JSEngine::init().expect("spidermonkey");
        // The engine can be initialized once per process. Leaking it keeps
        // later contexts, including parallel tests, able to create runtimes.
        Box::leak(Box::new(engine));
        unsafe extern "C" fn shutdown_engine() {
            unsafe { mozjs::jsapi::JS_ShutDown() };
        }
        unsafe {
            libc_atexit(shutdown_engine);
        }
    });
}

unsafe extern "C" {
    fn atexit(callback: unsafe extern "C" fn()) -> i32;
}

fn libc_atexit(callback: unsafe extern "C" fn()) {
    unsafe {
        atexit(callback);
    }
}

pub struct Context {
    cx: JSContext,
    raw: *mut RawJSContext,
    global: Option<JsObject>,
    old_realm: *mut jsapi::Realm,
    data: HashMap<TypeId, Box<dyn Any>>,
    natives: HashMap<usize, Rc<NativeSlot>>,
    functions: HashMap<usize, NativeFunction>,
    classes: HashMap<TypeId, ClassProto>,
    loader: Option<Rc<dyn ModuleLoader>>,
    hooks: Option<Rc<dyn HostHooks>>,
    constructing: Cell<bool>,
    new_target: RefCell<Option<JsValue>>,
    modules: HashMap<usize, (JsObject, PathBuf)>,
}

// SpiderMonkey contexts are thread-affine. The page session keeps this value
// on the thread that created it.
unsafe impl Send for Context {}

fn current() -> Option<*mut Context> {
    let ptr = CURRENT.with(|slot| slot.get());
    if ptr.is_null() { None } else { Some(ptr) }
}

impl Context {
    pub fn builder() -> ContextBuilder {
        ContextBuilder {
            loader: None,
            hooks: None,
        }
    }

    pub fn default() -> Self {
        Self::builder().build().expect("spidermonkey")
    }

    pub fn set_runtime_limits(&mut self, _limits: vm::RuntimeLimits) {}

    pub fn insert_data<T: 'static>(&mut self, value: T) {
        self.data.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn get_data<T: 'static>(&self) -> Option<&T> {
        self.data.get(&TypeId::of::<T>()).and_then(|value| value.downcast_ref())
    }

    pub fn global_object(&mut self) -> JsObject {
        self.global.clone().expect("global")
    }

    pub fn register_global_property(
        &mut self,
        name: JsString,
        value: impl Into<JsValue>,
        _attribute: Attribute,
    ) -> JsResult<()> {
        let global = self.global_object();
        self.set_prop(global.ptr, &name.as_key(), &value.into())
    }

    pub fn register_global_callable(
        &mut self,
        name: JsString,
        length: usize,
        native: NativeFunction,
    ) -> JsResult<()> {
        let function = self.new_function(native, &name.text, length, false);
        let global = self.global_object();
        self.set_prop(global.ptr, &name.as_key(), &JsValue::from(function))
    }

    pub fn register_global_class<T: Class>(&mut self) -> JsResult<()> {
        let constructor = class::build_class::<T>(self)?;
        self.register_global_property(
            JsString::from(T::NAME),
            constructor,
            Attribute::all(),
        )
    }

    pub fn eval(&mut self, source: Source) -> JsResult<JsValue> {
        self.eval_source(&source.code, &source.path)
    }

    pub fn run_jobs(&mut self) -> JsResult<()> {
        let _entered = Entered::new(self as *mut Context);
        unsafe { wrappers2::RunJobs(self.js()) };
        if unsafe { wrappers2::JS_IsExceptionPending(self.js()) } {
            return Err(self.take_exception());
        }
        Ok(())
    }

    pub fn intrinsics(&mut self) -> Intrinsics<'_> {
        Intrinsics { context: self }
    }

    fn js(&mut self) -> &mut JSContext {
        &mut self.cx
    }

    fn raw_cx(&self) -> *mut RawJSContext {
        self.raw
    }
}

pub struct Intrinsics<'a> {
    context: &'a mut Context,
}

impl<'a> Intrinsics<'a> {
    pub fn constructors(self) -> Constructors<'a> {
        Constructors { context: self.context }
    }
}

pub struct Constructors<'a> {
    context: &'a mut Context,
}

impl<'a> Constructors<'a> {
    pub fn typed_uint8_array(self) -> TypedArrayCtor<'a> {
        TypedArrayCtor { context: self.context }
    }
}

pub struct TypedArrayCtor<'a> {
    context: &'a mut Context,
}

impl<'a> TypedArrayCtor<'a> {
    pub fn constructor(&mut self) -> JsObject {
        self.context
            .global_object()
            .get(JsString::from("Uint8Array"), self.context)
            .ok()
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| self.context.new_object(None))
    }
}

pub struct ContextBuilder {
    loader: Option<Rc<dyn ModuleLoader>>,
    hooks: Option<Rc<dyn HostHooks>>,
}

impl ContextBuilder {
    pub fn module_loader(mut self, loader: Rc<dyn ModuleLoader>) -> Self {
        self.loader = Some(loader);
        self
    }

    pub fn host_hooks(mut self, hooks: Rc<dyn HostHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn build(self) -> Result<Context, JsError> {
        Context::init(self.loader, self.hooks)
    }
}

fn acquire_runtime() -> Result<*mut RawJSContext, JsError> {
    RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(runtime) = slot.as_mut() {
            runtime.live += 1;
            return Ok(runtime.raw);
        }
        let created = unsafe { jsapi::JS_NewContext(64 * 1024 * 1024, ptr::null_mut()) };
        let created = NonNull::new(created).ok_or_else(|| JsError {
            message: "JS_NewContext failed".to_string(),
        })?;
        let mut cx = unsafe { JSContext::from_ptr(created) };
        unsafe {
            wrappers2::UseInternalJobQueues(&cx);
            wrappers2::JS_AddExtraGCRootsTracer(&cx, Some(trace_live_roots), ptr::null_mut());
            wrappers2::JS_SetGCParameter(&mut cx, jsapi::JSGCParamKey::JSGC_NURSERY_ENABLED, 0);
            wrappers2::JS_SetGCParameter(&mut cx, jsapi::JSGCParamKey::JSGC_COMPACTING_ENABLED, 0);
            let cache = jsapi::__BindgenOpaqueArray::<u64, 2>::default();
            jsapi::InitSelfHostedCode(created.as_ptr(), cache, None);
        }
        *slot = Some(ThreadRuntime {
            raw: created.as_ptr(),
            live: 1,
        });
        Ok(created.as_ptr())
    })
}

fn release_runtime(raw: *mut RawJSContext) {
    RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(runtime) = slot.as_mut() else {
            return;
        };
        if runtime.raw != raw {
            return;
        }
        runtime.live = runtime.live.saturating_sub(1);
        if runtime.live == 0 {
            unsafe { jsapi::JS_DestroyContext(raw) };
            *slot = None;
        }
    });
}

impl Context {
    fn init(loader: Option<Rc<dyn ModuleLoader>>, hooks: Option<Rc<dyn HostHooks>>) -> Result<Self, JsError> {
        ensure_engine();
        let raw = acquire_runtime()?;
        let mut cx = unsafe { JSContext::from_ptr(NonNull::new_unchecked(raw)) };
        let options = RealmOptions::default();
        let global_ptr = unsafe {
            wrappers2::JS_NewGlobalObject(
                &mut cx,
                &SIMPLE_GLOBAL_CLASS,
                ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        };
        if global_ptr.is_null() {
            release_runtime(raw);
            return Err(JsError {
                message: "JS_NewGlobalObject failed".to_string(),
            });
        }
        let entered = unsafe { wrappers2::EnterRealm(&cx, global_ptr) };
        unsafe { wrappers2::InitRealmStandardClasses(&mut cx) };
        let mut context = Context {
            cx,
            raw,
            global: None,
            old_realm: ptr::null_mut(),
            data: HashMap::new(),
            natives: HashMap::new(),
            functions: HashMap::new(),
            classes: HashMap::new(),
            loader,
            hooks,
            constructing: Cell::new(false),
            new_target: RefCell::new(None),
            modules: HashMap::new(),
        };
        unsafe {
            install_hooks(&mut context);
        }
        let global = context.wrap_object(global_ptr);
        context.global = Some(global);
        unsafe { wrappers2::LeaveRealm(&context.cx, entered) };
        Ok(context)
    }

    fn constructing(&self) -> bool {
        self.constructing.get()
    }

    fn new_target(&self) -> Option<JsValue> {
        self.new_target.borrow().clone()
    }

    fn class_proto<T: 'static>(&self) -> Option<JsObject> {
        self.classes.get(&TypeId::of::<T>()).map(|info| info.prototype.clone())
    }

    fn remember_class<T: 'static>(&mut self, prototype: JsObject) {
        self.classes.insert(TypeId::of::<T>(), ClassProto { prototype });
    }

    fn new_object(&mut self, proto: Option<*mut JSObject>) -> JsObject {
        let _entered = Entered::new(self as *mut Context);
        let ptr = unsafe {
            if let Some(proto) = proto {
                rooted!(in(self.raw) let proto_root = proto);
                wrappers2::JS_NewObjectWithGivenProto(self.js(), ptr::null(), proto_root.handle())
            } else {
                wrappers2::JS_NewPlainObject(self.js())
            }
        };
        self.wrap_object(ptr)
    }

    fn new_reflector<T: 'static>(&mut self, data: T) -> JsObject<T> {
        let object = self.new_object(None);
        let slot = Rc::new(NativeSlot {
            type_id: TypeId::of::<T>(),
            data: UnsafeCell::new(Box::new(data)),
        });
        self.natives.insert(object.ptr as usize, Rc::clone(&slot));
        JsObject::from_parts(object.ptr, object.root, Some(slot))
    }

    fn new_array(&mut self, length: u32) -> JsObject {
        let _entered = Entered::new(self as *mut Context);
        let ptr = unsafe { wrappers2::NewArrayObject1(self.js(), length as usize) };
        self.wrap_object(ptr)
    }

    fn new_function(&mut self, native: NativeFunction, name: &str, length: usize, constructor: bool) -> JsFunction {
        let _entered = Entered::new(self as *mut Context);
        let c_name = CString::new(name).unwrap_or_else(|_| CString::new("native").unwrap());
        // JS_NewFunction tests JSFUN_CONSTRUCTOR (0x400), not FunctionFlags::CONSTRUCTOR.
        let flags = if constructor { 0x400 } else { 0 };
        let function = unsafe {
            wrappers2::JS_NewFunction(
                self.js(),
                Some(host_native),
                length as u32,
                flags,
                c_name.as_ptr(),
            )
        };
        let ptr = function as *mut JSObject;
        self.functions.insert(ptr as usize, native);
        JsFunction(self.wrap_object(ptr))
    }

    fn wrap_object(&mut self, ptr: *mut JSObject) -> JsObject {
        if ptr.is_null() {
            return self.placeholder_object();
        }
        let native = self.natives.get(&(ptr as usize)).cloned();
        let root = ArcRoot(Rc::new(Root::new(jsval::ObjectValue(ptr))));
        JsObject::from_parts(ptr, root, native)
    }

    fn placeholder_object(&mut self) -> JsObject {
        let ptr = unsafe { wrappers2::JS_NewPlainObject(self.js()) };
        let root = ArcRoot(Rc::new(Root::new(jsval::ObjectValue(ptr))));
        JsObject::from_parts(ptr, root, None)
    }

    fn wrap_value(&mut self, value: Value) -> JsValue {
        unsafe {
            if value.is_undefined() {
                JsValue::undefined()
            } else if value.is_null() {
                JsValue::null()
            } else if value.is_boolean() {
                JsValue::from(value.to_boolean())
            } else if value.is_int32() {
                JsValue::from(value.to_int32())
            } else if value.is_double() {
                JsValue::from(value.to_double())
            } else if value.is_string() {
                JsValue::from(JsString { text: string_to_rust(self.js(), value.to_string()) })
            } else if value.is_object() {
                JsValue::from(self.wrap_object(value.to_object()))
            } else {
                JsValue::undefined()
            }
        }
    }

    fn to_number_value(&mut self, value: &JsValue) -> JsResult<f64> {
        let raw = self.to_raw(value);
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let stored = raw);
            let mut out = 0f64;
            if !wrappers2::ToNumberSlow(self.js(), stored.handle().into(), &mut out) {
                return Err(self.take_exception());
            }
            Ok(out)
        }
    }

    fn to_u32_value(&mut self, value: &JsValue) -> JsResult<u32> {
        let raw = self.to_raw(value);
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let stored = raw);
            let mut out = 0u32;
            if !wrappers2::ToUint32Slow(self.js(), stored.handle().into(), &mut out) {
                return Err(self.take_exception());
            }
            Ok(out)
        }
    }

    fn to_raw(&mut self, value: &JsValue) -> Value {
        match &value.kind {
            ValueKind::Undefined => UndefinedValue(),
            ValueKind::Null => jsval::NullValue(),
            ValueKind::Bool(bit) => jsval::BooleanValue(*bit),
            ValueKind::Number(number) => {
                if number.fract() == 0.0 && *number >= i32::MIN as f64 && *number <= i32::MAX as f64 {
                    jsval::Int32Value(*number as i32)
                } else {
                    jsval::DoubleValue(*number)
                }
            }
            ValueKind::String(text) => {
                let string = new_string(self, text);
                if string.is_null() {
                    UndefinedValue()
                } else {
                    jsval::StringValue(unsafe { &*string })
                }
            }
            ValueKind::Object(object) => jsval::ObjectValue(object.ptr),
        }
    }

    fn get_prop(&mut self, object: *mut JSObject, key: &PropKey) -> JsResult<JsValue> {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            rooted!(in(self.raw) let mut rval = UndefinedValue());
            let ok = match key {
                PropKey::Index(index) => {
                    wrappers2::JS_GetElement(self.js(), obj.handle().into(), *index, rval.handle_mut())
                }
                PropKey::Name(name) => {
                    let units = utf16(name);
                    wrappers2::JS_GetUCProperty(
                        self.js(),
                        obj.handle().into(),
                        units.as_ptr(),
                        units.len(),
                        rval.handle_mut(),
                    )
                }
            };
            if !ok {
                return Err(self.take_exception());
            }
            Ok(self.wrap_value(rval.get()))
        }
    }

    fn define_data(
        &mut self,
        object: *mut JSObject,
        key: &PropKey,
        value: &JsValue,
        descriptor: &property::PropertyDescriptor,
    ) -> JsResult<()> {
        let mut attrs = 0u32;
        if descriptor.enumerable {
            attrs |= 0x01;
        }
        if !descriptor.writable {
            attrs |= 0x02;
        }
        if !descriptor.configurable {
            attrs |= 0x04;
        }
        let _entered = Entered::new(self as *mut Context);
        let raw = self.to_raw(value);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            rooted!(in(self.raw) let stored = raw);
            let ok = match key {
                PropKey::Name(name) => {
                    let units = utf16(name);
                    wrappers2::JS_DefineUCProperty2(
                        self.js(),
                        obj.handle().into(),
                        units.as_ptr(),
                        units.len(),
                        stored.handle().into(),
                        attrs,
                    )
                }
                PropKey::Index(_) => false,
            };
            if !ok {
                return Err(self.take_exception());
            }
        }
        Ok(())
    }

    fn set_prop(&mut self, object: *mut JSObject, key: &PropKey, value: &JsValue) -> JsResult<()> {
        let _entered = Entered::new(self as *mut Context);
        let raw = self.to_raw(value);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            rooted!(in(self.raw) let stored = raw);
            let ok = match key {
                PropKey::Index(index) => wrappers2::JS_SetElement(
                    self.js(),
                    obj.handle().into(),
                    *index,
                    stored.handle().into(),
                ),
                PropKey::Name(name) => {
                    let units = utf16(name);
                    wrappers2::JS_SetUCProperty(
                        self.js(),
                        obj.handle().into(),
                        units.as_ptr(),
                        units.len(),
                        stored.handle().into(),
                    )
                }
            };
            if !ok {
                return Err(self.take_exception());
            }
        }
        Ok(())
    }

    fn set_proto(&mut self, object: *mut JSObject, proto: Option<*mut JSObject>) {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            rooted!(in(self.raw) let proto_root = proto.unwrap_or(ptr::null_mut()));
            wrappers2::JS_SetPrototype(self.js(), obj.handle().into(), proto_root.handle().into());
        }
    }

    fn define_accessor(&mut self, object: *mut JSObject, key: &PropKey, descriptor: &property::PropertyDescriptor) -> JsResult<bool> {
        let getter = descriptor.get.as_ref().and_then(|value| value.as_object()).map(|object| object.ptr).unwrap_or(ptr::null_mut());
        let setter = descriptor.set.as_ref().and_then(|value| value.as_object()).map(|object| object.ptr).unwrap_or(ptr::null_mut());
        let mut attrs = 0u32;
        if descriptor.enumerable {
            attrs |= 0x01;
        }
        if descriptor.configurable {
            // configurable means not permanent
        } else {
            attrs |= 0x04;
        }
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            rooted!(in(self.raw) let get = getter);
            rooted!(in(self.raw) let set = setter);
            let ok = match key {
                PropKey::Name(name) => {
                    let units = utf16(name);
                    wrappers2::JS_DefineUCProperty3(
                        self.js(),
                        obj.handle().into(),
                        units.as_ptr(),
                        units.len(),
                        get.handle().into(),
                        set.handle().into(),
                        attrs,
                    )
                }
                PropKey::Index(_) => false,
            };
            if !ok {
                return Err(self.take_exception());
            }
        }
        Ok(true)
    }

    fn call_value(
        &mut self,
        function: *mut JSObject,
        this: &JsValue,
        args: &[JsValue],
        constructing: bool,
        new_target: Option<*mut JSObject>,
    ) -> JsResult<JsValue> {
        let _entered = Entered::new(self as *mut Context);
        self.constructing.set(constructing);
        if let Some(target) = new_target {
            *self.new_target.borrow_mut() = Some(JsValue::from(self.wrap_object(target)));
        }
        let this_raw = self.to_raw(this);
        let fun_raw = jsval::ObjectValue(function);
        let mut raw_args: Vec<Value> = Vec::with_capacity(args.len());
        for arg in args {
            raw_args.push(self.to_raw(arg));
        }
        let result = unsafe {
            rooted!(in(self.raw) let thisv = this_raw);
            rooted!(in(self.raw) let funv = fun_raw);
            rooted!(in(self.raw) let mut rval = UndefinedValue());
            let array = handle_values(&raw_args);
            let ok = if constructing {
                rooted!(in(self.raw) let mut obj: *mut JSObject = ptr::null_mut());
                let called = wrappers2::Construct1(
                    self.js(),
                    funv.handle().into(),
                    &array,
                    obj.handle_mut(),
                );
                if called {
                    rval.set(jsval::ObjectOrNullValue(obj.get()));
                }
                called
            } else {
                wrappers2::Call(
                    self.js(),
                    thisv.handle().into(),
                    funv.handle().into(),
                    &array,
                    rval.handle_mut(),
                )
            };
            if !ok {
                Err(self.take_exception())
            } else {
                Ok(self.wrap_value(rval.get()))
            }
        };
        self.constructing.set(false);
        *self.new_target.borrow_mut() = None;
        result
    }

    fn construct_value(
        &mut self,
        function: *mut JSObject,
        args: &[JsValue],
        new_target: Option<*mut JSObject>,
    ) -> JsResult<JsObject> {
        let target = new_target.unwrap_or(function);
        let value = self.call_value(function, &JsValue::undefined(), args, true, Some(target))?;
        value.as_object().ok_or_else(|| JsError { message: "constructor did not return an object".to_string() })
    }

    fn value_is_callable(&mut self, value: Value) -> bool {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let stored = value);
            wrappers2::JS_TypeOfValue(self.js(), stored.handle().into()) == jsapi::JSType::JSTYPE_FUNCTION
        }
    }

    fn object_is_array(&mut self, object: *mut JSObject) -> bool {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let obj = object);
            let mut is_array = false;
            wrappers2::IsArrayObject1(self.js(), obj.handle().into(), &mut is_array);
            is_array
        }
    }

    fn stringify(&mut self, value: &JsValue) -> String {
        let _entered = Entered::new(self as *mut Context);
        let raw = self.to_raw(value);
        unsafe {
            rooted!(in(self.raw) let stored = raw);
            let string = mozjs::rust::ToString(self.js(), stored.handle());
            if string.is_null() {
                wrappers2::JS_ClearPendingException(self.js());
                return value.display_fallback();
            }
            string_to_rust(self.js(), string)
        }
    }

    fn take_exception(&mut self) -> JsError {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let mut rval = UndefinedValue());
            if !wrappers2::JS_GetPendingException(self.js(), rval.handle_mut()) {
                wrappers2::JS_ClearPendingException(self.js());
                return JsError { message: "JavaScript error".to_string() };
            }
            wrappers2::JS_ClearPendingException(self.js());
            let value = self.wrap_value(rval.get());
            let mut message = self.stringify(&value);
            if let Some(object) = value.as_object()
                && let Ok(stack) = self.get_prop(object.ptr, &PropKey::Name("stack".to_string()))
            {
                let stack = self.stringify(&stack);
                if !stack.is_empty() && stack != "undefined" {
                    message.push('\n');
                    message.push_str(&stack);
                }
            }
            JsError { message }
        }
    }

    fn compile_script(&mut self, code: &str, path: &str) -> JsResult<()> {
        let prepared = prepare_source(code);
        let code = prepared.as_ref();
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            let filename = CString::new(path).unwrap_or_else(|_| CString::new("inline.js").unwrap());
            let options = CompileOptionsWrapper::new(self.js(), filename, 1);
            let mut source = transform_str_to_source_text(code);
            let script = wrappers2::Compile1(self.js(), options.ptr, &mut source);
            if script.is_null() {
                return Err(self.take_exception());
            }
        }
        Ok(())
    }

    fn eval_source(&mut self, code: &str, path: &str) -> JsResult<JsValue> {
        let prepared = prepare_source(code);
        let code = prepared.as_ref();
        let _entered = Entered::new(self as *mut Context);
        let global = self.global_object();
        unsafe {
            rooted!(in(self.raw) let glob = global.ptr);
            rooted!(in(self.raw) let mut rval = UndefinedValue());
            let filename = CString::new(path).unwrap_or_else(|_| CString::new("inline.js").unwrap());
            let options = CompileOptionsWrapper::new(self.js(), filename, 1);
            let evaluated = mozjs::rust::evaluate_script(
                self.js(),
                glob.handle(),
                code,
                rval.handle_mut(),
                options,
            );
            if evaluated.is_err() {
                return Err(self.take_exception());
            }
            Ok(self.wrap_value(rval.get()))
        }
    }

    fn compile_module(&mut self, code: &str, path: &str) -> JsResult<Module> {
        let prepared = prepare_source(code);
        let code = prepared.as_ref();
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            let filename = CString::new(path).unwrap_or_else(|_| CString::new("module.js").unwrap());
            let options = CompileOptionsWrapper::new(self.js(), filename, 1);
            let mut source = transform_str_to_source_text(code);
            let module = wrappers2::CompileModule1(self.js(), options.ptr, &mut source);
            if module.is_null() {
                return Err(self.take_exception());
            }
            let record = self.wrap_object(module);
            self.modules.insert(record.ptr as usize, (record.clone(), PathBuf::from(path)));
            Ok(Module::from_record(record, PathBuf::from(path)))
        }
    }

    fn link_and_evaluate(&mut self, module: &Module) -> JsPromise {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let record = module.record().ptr);
            rooted!(in(self.raw) let host = UndefinedValue());
            let loaded = wrappers2::LoadRequestedModules(
                self.js(),
                record.handle().into(),
                host.handle().into(),
                Some(module_resolved),
                Some(module_rejected),
            );
            if !loaded {
                return self.rejected_promise();
            }
            if !wrappers2::ModuleLink(self.js(), record.handle().into()) {
                return self.rejected_promise();
            }
            rooted!(in(self.raw) let mut rval = UndefinedValue());
            if !wrappers2::ModuleEvaluate(self.js(), record.handle().into(), rval.handle_mut()) {
                return self.rejected_promise();
            }
            let _ = wrappers2::RunJobs(self.js());
            let value = self.wrap_value(rval.get());
            if let Some(object) = value.as_object() {
                JsPromise { object }
            } else {
                self.promise_resolve(JsValue::undefined()).unwrap_or_else(|_| self.rejected_promise())
            }
        }
    }

    fn promise_resolve(&mut self, value: JsValue) -> JsResult<JsPromise> {
        let global = self.global_object();
        let promise_ctor = self.get_prop(global.ptr, &PropKey::Name("Promise".to_string()))?;
        let function = promise_ctor.as_object().ok_or_else(|| JsError { message: "Promise is missing".to_string() })?;
        let resolve = self.get_prop(function.ptr, &PropKey::Name("resolve".to_string()))?;
        let resolve = resolve.as_object().ok_or_else(|| JsError { message: "Promise.resolve is missing".to_string() })?;
        let result = resolve.call(&promise_ctor, &[value], self)?;
        let object = result.as_object().ok_or_else(|| JsError { message: "Promise.resolve did not return a promise".to_string() })?;
        Ok(JsPromise { object })
    }

    fn rejected_promise(&mut self) -> JsPromise {
        let message = self.take_exception();
        self.promise_reject(message.message)
    }

    fn promise_reject(&mut self, message: String) -> JsPromise {
        let (promise, resolvers) = self.promise_pending();
        let _ = resolvers.reject.call(&JsValue::undefined(), &[JsValue::from(JsString::from(message))], self);
        promise
    }

    fn promise_state(&mut self, promise: &JsObject) -> builtins::promise::PromiseState {
        let _entered = Entered::new(self as *mut Context);
        unsafe {
            rooted!(in(self.raw) let obj = promise.ptr);
            let state = wrappers2::GetPromiseState(obj.handle().into());
            if state == jsapi::PromiseState::Pending {
                builtins::promise::PromiseState::Pending
            } else if state == jsapi::PromiseState::Rejected {
                rooted!(in(self.raw) let mut rval = UndefinedValue());
                wrappers2::JS_GetPromiseResult(obj.handle().into(), rval.handle_mut());
                builtins::promise::PromiseState::Rejected(self.wrap_value(rval.get()))
            } else {
                builtins::promise::PromiseState::Fulfilled(builtins::promise::Placeholder)
            }
        }
    }
}

impl JsValue {
    fn display_fallback(&self) -> String {
        match &self.kind {
            ValueKind::Undefined => "undefined".to_string(),
            ValueKind::Null => "null".to_string(),
            ValueKind::Bool(value) => value.to_string(),
            ValueKind::Number(value) => value.to_string(),
            ValueKind::String(text) => text.clone(),
            ValueKind::Object(_) => "[object Object]".to_string(),
        }
    }
}

impl Context {
    // Stored only while a promise executor runs.
    // Declared via a side field added below through the existing struct... 
}

// The pending resolver slot lives beside the context so the executor native can
// see it without a second borrow of Context's fields map.
thread_local! {
    static PENDING_RESOLVERS: RefCell<Option<Rc<RefCell<Option<(JsObject, JsObject)>>>>> = const { RefCell::new(None) };
}

fn capture_resolvers(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let resolve = args.first().and_then(|value| value.as_object());
    let reject = args.get(1).and_then(|value| value.as_object());
    if let (Some(resolve), Some(reject)) = (resolve, reject) {
        PENDING_RESOLVERS.with(|slot| {
            if let Some(captured) = slot.borrow().as_ref() {
                *captured.borrow_mut() = Some((resolve, reject));
            }
        });
    }
    Ok(JsValue::undefined())
}

unsafe extern "C" fn host_native(_cx: *mut RawJSContext, argc: u32, vp: *mut Value) -> bool {
    unsafe {
        let Some(context) = current() else {
            return false;
        };
        let previous = enter(context);
        let callee = *vp;
        let this_raw = *vp.add(1);
        let constructing = this_raw.is_magic();
        (*context).constructing.set(constructing);
        if constructing {
            let target = *vp.add(2 + argc as usize);
            *(*context).new_target.borrow_mut() = Some((*context).wrap_value(target));
        }
        let function = if callee.is_object() {
            callee.to_object() as usize
        } else {
            0
        };
        let native = (*context).functions.get(&function).copied();
        let this_value = (*context).wrap_value(this_raw);
        let mut args = Vec::with_capacity(argc as usize);
        for index in 0..argc {
            args.push((*context).wrap_value(*vp.add(2 + index as usize)));
        }
        let result = if let Some(native) = native {
            native.0(&this_value, &args, &mut *context)
        } else {
            Err(JsError { message: "native function is missing".to_string() })
        };
        (*context).constructing.set(false);
        *(*context).new_target.borrow_mut() = None;
        leave(previous);
        match result {
            Ok(value) => {
                // SpiderMonkey 153 stores the return value over the callee at
                // vp[0]. vp[1] stays the this-value.
                let raw = (*context).to_raw(&value);
                *vp = raw;
                true
            }
            Err(error) => {
                throw_message(&mut *context, &error.message);
                false
            }
        }
    }
}

fn throw_message(context: &mut Context, message: &str) {
    let global = context.global_object();
    if let Ok(ctor) = context.get_prop(global.ptr, &PropKey::Name("TypeError".to_string()))
        && let Some(ctor) = ctor.as_object()
        && let Ok(error) = ctor.construct(&[JsValue::from(JsString::from(message.to_string()))], None, context)
    {
        let raw = jsval::ObjectValue(error.ptr);
        unsafe {
            rooted!(in(context.raw) let stored = raw);
            wrappers2::JS_SetPendingException(
                context.js(),
                stored.handle().into(),
                jsapi::ExceptionStackBehavior::Capture,
            );
        }
        return;
    }
    let c_message = CString::new(message).unwrap_or_else(|_| CString::new("error").unwrap());
    unsafe { wrappers2::ReportErrorUTF8(context.js(), c_message.as_ptr()) };
}

unsafe fn install_hooks(context: &mut Context) {
    unsafe {
        wrappers2::SetPromiseRejectionTrackerCallback(
            context.js(),
            Some(rejection_tracker),
            ptr::null_mut(),
        );
        let runtime = wrappers2::JS_GetRuntime(context.js());
        jsapi::SetModuleLoadHook(runtime, Some(module_load_hook));
        jsapi::SetModuleMetadataHook(runtime, Some(module_meta_hook));
    }
}

unsafe extern "C" fn module_load_hook(
    _cx: *mut RawJSContext,
    referrer: jsapi::Handle<*mut jsapi::JSScript>,
    module_request: jsapi::Handle<*mut JSObject>,
    _host_defined: jsapi::Handle<Value>,
    payload: jsapi::Handle<Value>,
    _line: u32,
    _column: jsapi::ColumnNumberOneOrigin,
) -> bool {
    unsafe {
        let Some(context) = current() else {
            return false;
        };
        let previous = enter(context);
        let specifier = {
            let request = mozjs::gc::Handle::from_raw(module_request);
            let string = wrappers2::GetModuleRequestSpecifier((*context).js(), request);
            if string.is_null() {
                leave(previous);
                return false;
            }
            string_to_rust((*context).js(), string)
        };
        let path = if referrer.ptr.is_null() || (*referrer.ptr).is_null() {
            None
        } else {
            let script = mozjs::gc::Handle::from_raw(referrer);
            let module_object = wrappers2::GetModuleObject(script);
            if module_object.is_null() {
                None
            } else {
                (*context)
                    .modules
                    .get(&(module_object as usize))
                    .map(|(_, path)| path.clone())
            }
        };
        let loaded = (*context).loader.clone().map(|loader| {
            loader.load_imported_module(
                module::Referrer::new(path),
                module::ModuleRequest::new(specifier.clone()),
                &mut *context,
            )
        });
        let finished = match loaded {
            Some(Ok(module)) => {
                rooted!(in((*context).raw) let record = module.record().ptr);
                let request = mozjs::gc::Handle::from_raw(module_request);
                let payload_handle = mozjs::gc::Handle::from_raw(payload);
                let referrer_handle = mozjs::gc::Handle::from_raw(referrer);
                // A dynamic `import()` passes a promise. SpiderMonkey then
                // links immediately, which rejects a module that is still New.
                // Load its dependencies first so the record becomes Unlinked.
                let dynamic = payload_is_promise((*context).raw, payload);
                if dynamic {
                    rooted!(in((*context).raw) let host = UndefinedValue());
                    let _ = wrappers2::LoadRequestedModules(
                        (*context).js(),
                        record.handle().into(),
                        host.handle().into(),
                        Some(module_resolved),
                        Some(module_rejected),
                    );
                }
                wrappers2::FinishLoadingImportedModule(
                    (*context).js(),
                    referrer_handle,
                    request,
                    payload_handle,
                    record.handle().into(),
                    false,
                )
            }
            Some(Err(error)) => {
                throw_message(&mut *context, &error.message);
                false
            }
            None => {
                throw_message(&mut *context, "module loader is missing");
                false
            }
        };
        leave(previous);
        finished
    }
}

unsafe extern "C" fn module_meta_hook(
    _cx: *mut RawJSContext,
    module_record: jsapi::Handle<*mut JSObject>,
    meta_object: jsapi::Handle<*mut JSObject>,
) -> bool {
    unsafe {
        let Some(context) = current() else {
            return false;
        };
        let previous = enter(context);
        let record_ptr = *module_record.ptr;
        let path = (*context)
            .modules
            .get(&(record_ptr as usize))
            .map(|(_, path)| path.clone())
            .unwrap_or_default();
        if let Some(loader) = (*context).loader.clone() {
            let record = (*context).wrap_object(record_ptr);
            let meta = (*context).wrap_object(*meta_object.ptr);
            let module = module::Module::from_record(record, path);
            loader.init_import_meta(&meta, &module, &mut *context);
        }
        leave(previous);
        true
    }
}

unsafe extern "C" fn rejection_tracker(
    _cx: *mut RawJSContext,
    _muted: bool,
    promise: jsapi::HandleObject,
    state: jsapi::PromiseRejectionHandlingState,
    _data: *mut std::os::raw::c_void,
) {
    unsafe {
        let Some(context) = current() else {
            return;
        };
        let previous = enter(context);
        let ptr = *promise.ptr;
        let object = (*context).wrap_object(ptr);
        let promise = JsPromise { object: object.clone() };
        let operation = if state == jsapi::PromiseRejectionHandlingState::Unhandled {
            OperationType::Reject
        } else {
            OperationType::Handle
        };
        if let Some(hooks) = (*context).hooks.clone() {
            hooks.promise_rejection_tracker(&promise, operation, &mut *context);
        }
        leave(previous);
    }
}

/// SpiderMonkey's Unicode property escapes need the `intl` build, which
/// cannot link next to Parley's ICU. Rewrite the common categories so a
/// pattern such as `/\p{L}/u` still parses.
fn prepare_source(code: &str) -> std::borrow::Cow<'_, str> {
    if !code.contains("\\p{") && !code.contains("\\P{") {
        return std::borrow::Cow::Borrowed(code);
    }
    let mut out = String::with_capacity(code.len() + 32);
    let mut rest = code;
    while let Some(index) = rest.find('\\') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        let negated = after.starts_with('P');
        let property = after.starts_with('p') || negated;
        if property && after[1..].starts_with('{') {
            if let Some(end) = after[2..].find('}') {
                let name = &after[2..2 + end];
                let class = unicode_property_class(name, negated);
                if inside_character_class(&out) {
                    out.push_str(&class);
                } else if negated {
                    out.push_str(&format!("[^{class}]"));
                } else {
                    out.push_str(&format!("[{class}]"));
                }
                rest = &after[2 + end + 1..];
                continue;
            }
        }
        out.push('\\');
        rest = after;
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

fn inside_character_class(so_far: &str) -> bool {
    let start = so_far.len().saturating_sub(500);
    let slice = &so_far[start..];
    let mut depth = 0i32;
    let mut escaped = false;
    for ch in slice.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '[' {
            depth += 1;
        } else if ch == ']' && depth > 0 {
            depth -= 1;
        }
    }
    depth > 0
}

fn unicode_property_class(name: &str, negated: bool) -> String {
    let key = name
        .split('=')
        .next_back()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
        .replace(['_', ' '], "");
    let class = match key.as_str() {
        "l" | "letter" | "latin" => "A-Za-z\\u00C0-\\u024F\\u0400-\\u04FF\\u0900-\\u097F\\u0980-\\u09FF",
        "ll" | "lowercaseletter" => "a-z\\u00DF-\\u00FF",
        "lu" | "uppercaseletter" => "A-Z\\u00C0-\\u00D6\\u00D8-\\u00DE",
        "n" | "number" | "nd" | "decimalnumber" => "0-9",
        "z" | "zs" | "separator" | "spaceseparator" => " \\t\\u00A0",
        "p" | "punctuation" => "!-/:-@\\[-`{-~",
        "emoji" => "\\u{1F300}-\\u{1FAFF}",
        _ if negated => "\\s\\S",
        _ => "\\s\\S",
    };
    let _ = negated;
    class.to_string()
}

fn payload_is_promise(raw: *mut RawJSContext, payload: jsapi::Handle<Value>) -> bool {
    unsafe {
        let value = *payload.ptr;
        if !value.is_object() {
            return false;
        }
        rooted!(in(raw) let object = value.to_object());
        wrappers2::IsPromiseObject(object.handle().into())
    }
}

unsafe extern "C" fn module_resolved(_cx: *mut RawJSContext, _host: jsapi::Handle<Value>) -> bool {
    true
}

unsafe extern "C" fn module_rejected(_cx: *mut RawJSContext, _host: jsapi::Handle<Value>, _error: jsapi::Handle<Value>) -> bool {
    true
}

fn number_to_u32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    let modulo = truncated.rem_euclid(2f64.powi(32));
    modulo as u32
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn new_string(context: &mut Context, text: &str) -> *mut JSString {
    let chars = mozjs::conversions::Utf8Chars::from(text);
    unsafe { wrappers2::JS_NewStringCopyUTF8N(context.js(), &*chars) }
}

thread_local! {
    static ENCODED: RefCell<String> = const { RefCell::new(String::new()) };
}

unsafe extern "C" fn encoded_callback(ptr: *const std::ffi::c_char) {
    if ptr.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    ENCODED.with(|slot| *slot.borrow_mut() = text);
}

fn string_to_rust(cx: &mut JSContext, string: *mut JSString) -> String {
    if string.is_null() {
        return String::new();
    }
    unsafe {
        let raw = cx.raw_cx();
        rooted!(in(raw) let rooted_string = string);
        wrappers2::EncodeStringToUTF8(cx, rooted_string.handle().into(), encoded_callback);
    }
    ENCODED.with(|slot| slot.borrow().clone())
}

fn handle_values(values: &[Value]) -> jsapi::HandleValueArray {
    jsapi::HandleValueArray {
        length_: values.len(),
        elements_: if values.is_empty() { ptr::null() } else { values.as_ptr() },
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // Drop runs this body first, then the fields. Clear JS objects before
        // the last user destroys the shared context.
        self.global.take();
        self.natives.clear();
        self.classes.clear();
        self.modules.clear();
        self.functions.clear();
        release_runtime(self.raw);
    }
}

// Fix promise_pending to publish the slot before constructing.
impl Context {
    fn promise_pending(&mut self) -> (JsPromise, object::builtins::Resolvers) {
        let captured: Rc<RefCell<Option<(JsObject, JsObject)>>> = Rc::new(RefCell::new(None));
        PENDING_RESOLVERS.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&captured)));
        let executor = self.new_function(NativeFunction::from_fn_ptr(capture_resolvers), "executor", 2, false);
        let global = self.global_object();
        let ctor = self
            .get_prop(global.ptr, &PropKey::Name("Promise".to_string()))
            .ok()
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| self.new_object(None));
        let object = ctor
            .construct(&[JsValue::from(JsFunction(executor.0.clone()))], None, self)
            .unwrap_or_else(|_| self.new_object(None));
        PENDING_RESOLVERS.with(|slot| *slot.borrow_mut() = None);
        let pair = captured.borrow_mut().take().unwrap_or_else(|| (self.new_object(None), self.new_object(None)));
        (
            JsPromise { object },
            object::builtins::Resolvers { resolve: pair.0, reject: pair.1 },
        )
    }
}
