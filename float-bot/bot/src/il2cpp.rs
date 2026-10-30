// Minimal binding to the il2cpp API exported by GameAssembly.dll. Everything is resolved by name at
// runtime, so it keeps working across game updates as long as the names stay the same.
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

use hudhook::windows::core::PCSTR;
use hudhook::windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

pub type P = *mut c_void;

macro_rules! api {
    ($($name:ident($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub struct Api { $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }
        impl Api {
            unsafe fn load() -> Option<Self> {
                let m = GetModuleHandleA(PCSTR(c"GameAssembly.dll".as_ptr() as _)).ok()?;
                Some(Self { $($name: std::mem::transmute(GetProcAddress(
                    m, PCSTR(concat!("il2cpp_", stringify!($name), "\0").as_ptr()))?),)* })
            }
        }
    };
}

api! {
    domain_get() -> P;
    domain_get_assemblies(P, *mut usize) -> *const P;
    assembly_get_image(P) -> P;
    image_get_name(P) -> *const c_char;
    class_from_name(P, *const c_char, *const c_char) -> P;
    class_get_method_from_name(P, *const c_char, i32) -> P;
    class_get_field_from_name(P, *const c_char) -> P;
    field_get_offset(P) -> usize;
    object_get_class(P) -> P;
    thread_attach(P) -> P;
    thread_detach(P);
    string_new(*const c_char) -> P;
    string_chars(P) -> *const u16;
    string_length(P) -> i32;
    array_length(P) -> u32;
    class_get_type(P) -> P;
    type_get_object(P) -> P;
    type_get_name(P) -> *mut c_char;
    free(*mut c_char);
    class_get_methods(P, *mut P) -> P;
    method_get_name(P) -> *const c_char;
    method_get_param_count(P) -> u32;
    method_get_param(P, u32) -> P;
    method_is_generic(P) -> bool;
    class_get_fields(P, *mut P) -> P;
    field_get_name(P) -> *const c_char;
    field_get_flags(P) -> i32;
    field_static_get_value(P, *mut c_void);
    field_static_set_value(P, *mut c_void);
    resolve_icall(*const c_char) -> P;
}

static API: OnceLock<Api> = OnceLock::new();

pub fn api() -> &'static Api {
    API.get().expect("il2cpp api not loaded")
}

/// Loads the API and checks the runtime is up (Assembly-CSharp present).
pub unsafe fn init() -> bool {
    if API.get().is_none() {
        match Api::load() {
            Some(a) => _ = API.set(a),
            None => return false,
        }
    }
    !(api().domain_get)().is_null() && image("Assembly-CSharp.dll").is_some()
}

/// (methodPointer, MethodInfo*) — the MethodInfo goes as the hidden last argument of every call.
#[derive(Clone, Copy)]
pub struct M(pub usize, pub usize);

/// call!(method, fn(ArgTypes..) -> Ret, args..). `C-unwind` lets a managed exception propagate
/// back to Unity (which logs it) instead of aborting the process.
#[macro_export]
macro_rules! call {
    ($m:expr, fn($($t:ty),*) $(-> $r:ty)? $(, $a:expr)*) => {{
        let m: $crate::il2cpp::M = $m;
        let f: unsafe extern "C-unwind" fn($($t,)* usize) $(-> $r)? = std::mem::transmute(m.0);
        f($($a,)* m.1)
    }};
}

pub unsafe fn image(name: &str) -> Option<P> {
    let a = api();
    let mut n = 0usize;
    let asms = (a.domain_get_assemblies)((a.domain_get)(), &mut n);
    (0..n).map(|i| (a.assembly_get_image)(*asms.add(i))).find(|&img| {
        CStr::from_ptr((a.image_get_name)(img)).to_bytes() == name.as_bytes()
    })
}

pub unsafe fn class(image_name: &str, ns: &str, name: &str) -> Result<P, String> {
    let img = image(image_name).ok_or(format!("image {image_name}"))?;
    let (ns_c, name_c) = (CString::new(ns).unwrap(), CString::new(name).unwrap());
    let k = (api().class_from_name)(img, ns_c.as_ptr(), name_c.as_ptr());
    if k.is_null() {
        return Err(format!("classe {ns}.{name}"));
    }
    Ok(k)
}

pub unsafe fn method(klass: P, name: &str, argc: i32) -> Result<M, String> {
    let c = CString::new(name).unwrap();
    let mi = (api().class_get_method_from_name)(klass, c.as_ptr(), argc);
    let ptr = if mi.is_null() { 0 } else { *(mi as *const usize) };
    if ptr == 0 {
        return Err(format!("methode {name}/{argc}"));
    }
    Ok(M(ptr, mi as usize))
}

/// Picks one overload by its exact parameter type names (get_method_from_name only matches arity).
pub unsafe fn method_sig(klass: P, name: &str, params: &[&str]) -> Result<M, String> {
    let a = api();
    let mut iter: P = std::ptr::null_mut();
    loop {
        let mi = (a.class_get_methods)(klass, &mut iter);
        if mi.is_null() {
            return Err(format!("methode {name}({})", params.join(", ")));
        }
        if CStr::from_ptr((a.method_get_name)(mi)).to_bytes() != name.as_bytes()
            || (a.method_is_generic)(mi)
            || (a.method_get_param_count)(mi) as usize != params.len()
        {
            continue;
        }
        let matches = params.iter().enumerate().all(|(i, want)| {
            let n = (a.type_get_name)((a.method_get_param)(mi, i as u32));
            let ok = CStr::from_ptr(n).to_bytes() == want.as_bytes();
            (a.free)(n);
            ok
        });
        let ptr = *(mi as *const usize);
        if matches && ptr != 0 {
            return Ok(M(ptr, mi as usize));
        }
    }
}

/// The System.Type object of a class, for Unity APIs taking a Type.
pub unsafe fn type_object(klass: P) -> P {
    (api().type_get_object)((api().class_get_type)(klass))
}

/// (value, name) of every constant of an enum class.
pub unsafe fn enum_values(klass: P) -> Vec<(i32, String)> {
    let a = api();
    let mut out = Vec::new();
    let mut iter: P = std::ptr::null_mut();
    loop {
        let f = (a.class_get_fields)(klass, &mut iter);
        if f.is_null() {
            return out;
        }
        if (a.field_get_flags)(f) & 0x10 != 0 {
            // FIELD_ATTRIBUTE_STATIC: the enum constants (value__ is the only instance field).
            let mut v = 0i32;
            (a.field_static_get_value)(f, &mut v as *mut i32 as _);
            out.push((v, CStr::from_ptr((a.field_get_name)(f)).to_string_lossy().into_owned()));
        }
    }
}

/// Field offset looked up on the object's runtime class (for nested or generic classes).
pub unsafe fn field_of(obj: P, name: &str) -> Result<usize, String> {
    field((api().object_get_class)(obj), name)
}

pub unsafe fn field(klass: P, name: &str) -> Result<usize, String> {
    let c = CString::new(name).unwrap();
    let f = (api().class_get_field_from_name)(klass, c.as_ptr());
    if f.is_null() {
        return Err(format!("champ {name}"));
    }
    Ok((api().field_get_offset)(f))
}

/// A static field (its FieldInfo), for static_set.
pub unsafe fn static_field(klass: P, name: &str) -> Result<P, String> {
    let c = CString::new(name).unwrap();
    let f = (api().class_get_field_from_name)(klass, c.as_ptr());
    if f.is_null() || (api().field_get_flags)(f) & 0x10 == 0 {
        return Err(format!("champ statique {name}"));
    }
    Ok(f)
}

/// Writes a static field (its class must be initialized: call one of its methods first).
pub unsafe fn static_set<T>(f: P, mut v: T) {
    (api().field_static_set_value)(f, &mut v as *mut T as *mut c_void);
}

/// The engine's native function behind an internal call, like "UnityEngine.Time::get_deltaTime()":
/// there even when the managed wrapper was stripped from the build.
pub unsafe fn icall(name: &str) -> Option<P> {
    let c = CString::new(name).unwrap();
    let f = (api().resolve_icall)(c.as_ptr());
    (!f.is_null()).then_some(f)
}

pub unsafe fn rd<T: Copy>(obj: P, off: usize) -> T {
    *((obj as usize + off) as *const T)
}

pub unsafe fn wr<T>(obj: P, off: usize, v: T) {
    *((obj as usize + off) as *mut T) = v;
}

pub unsafe fn string(s: P) -> String {
    if s.is_null() {
        return String::new();
    }
    let a = api();
    String::from_utf16_lossy(std::slice::from_raw_parts((a.string_chars)(s), (a.string_length)(s) as usize))
}

pub unsafe fn new_string(s: &str) -> P {
    let c = CString::new(s).unwrap();
    (api().string_new)(c.as_ptr())
}

/// Elements of a managed reference-type array.
pub unsafe fn array(arr: P) -> Vec<P> {
    values(arr)
}

/// Elements of a managed array (data starts after the 0x20-byte array header).
pub unsafe fn values<T: Copy>(arr: P) -> Vec<T> {
    if arr.is_null() {
        return Vec::new();
    }
    let n = (api().array_length)(arr) as usize;
    std::slice::from_raw_parts((arr as usize + 0x20) as *const T, n).to_vec()
}

/// Elements of a System.Collections.Generic.List<T> of references.
pub unsafe fn list(l: P) -> Vec<P> {
    if l.is_null() {
        return Vec::new();
    }
    let k = (api().object_get_class)(l);
    let (Ok(items), Ok(size)) = (field(k, "_items"), field(k, "_size")) else { return Vec::new() };
    let mut v = array(rd(l, items));
    v.truncate(rd::<i32>(l, size).max(0) as usize);
    v
}

/// A destroyed UnityEngine.Object keeps its managed wrapper but m_CachedPtr (0x10) becomes null.
pub unsafe fn alive(obj: P) -> bool {
    !obj.is_null() && rd::<usize>(obj, 0x10) != 0
}
