//! doubao-ime 的 C ABI（同步）。
//!
//! 设计约定：
//! * 所有函数以 `dbao_` 前缀。
//! * 返回 `int` 的函数：`0` 成功，负值对应 [`doubao_ime::ErrorKind`]（取负）。
//! * 字符串以 UTF-8、NUL 结尾传入；返回的字符串由本库分配，须用 [`dbao_string_free`] 释放。
//! * 句柄（`DbaoClient*`）由 [`dbao_client_new`] 创建，[`dbao_client_free`] 释放，线程安全（内部 `Client` 可 `Clone`）。
//! * 最近一次错误信息可用 [`dbao_last_error`] 取得（线程局部）。
//!
//! 头文件见 `include/doubao_ime.h`。

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

use doubao_ime::blocking::Client;
use doubao_ime::{AsrOptions, Error, ErrorKind};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// 不透明客户端句柄。
pub struct DbaoClient {
    inner: Client,
}

fn set_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(msg).ok());
}

fn err_code(kind: ErrorKind) -> c_int {
    -(kind as c_int)
}

fn record(err: &Error) -> c_int {
    set_error(err.to_string());
    err_code(err.kind())
}

/// 参数错误码（NUL 指针、非法 UTF-8）。
const DBAO_ERR_ARG: c_int = -100;

unsafe fn cstr<'a>(p: *const c_char) -> Result<&'a str, c_int> {
    if p.is_null() {
        set_error("参数为空指针".into());
        return Err(DBAO_ERR_ARG);
    }
    CStr::from_ptr(p).to_str().map_err(|_| {
        set_error("参数不是合法 UTF-8".into());
        DBAO_ERR_ARG
    })
}

fn out_string(out: *mut *mut c_char, s: String) -> c_int {
    match CString::new(s) {
        Ok(c) => {
            unsafe { *out = c.into_raw() };
            0
        }
        Err(_) => {
            set_error("结果含内部 NUL".into());
            DBAO_ERR_ARG
        }
    }
}

/// 取得当前线程最近一次错误信息（借用指针，勿释放；下次调用前有效）。返回 NULL 表示无错误。
#[no_mangle]
pub extern "C" fn dbao_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ref().map_or(ptr::null(), |c| c.as_ptr()))
}

/// 释放由本库返回的字符串。
///
/// # Safety
/// `s` 必须是本库某个函数返回的指针，且只释放一次。
#[no_mangle]
pub unsafe extern "C" fn dbao_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// 创建客户端。成功返回非空句柄，失败返回 NULL（详见 [`dbao_last_error`]）。
#[no_mangle]
pub extern "C" fn dbao_client_new() -> *mut DbaoClient {
    match Client::new() {
        Ok(inner) => Box::into_raw(Box::new(DbaoClient { inner })),
        Err(e) => {
            record(&e);
            ptr::null_mut()
        }
    }
}

/// 释放客户端句柄。
///
/// # Safety
/// `client` 必须来自 [`dbao_client_new`]，且只释放一次。
#[no_mangle]
pub unsafe extern "C" fn dbao_client_free(client: *mut DbaoClient) {
    if !client.is_null() {
        drop(Box::from_raw(client));
    }
}

/// 文字整理。整理结果写入 `*out`（须用 [`dbao_string_free`] 释放）。
///
/// # Safety
/// `client` 有效；`text` 为 UTF-8 NUL 结尾；`out` 指向可写指针。
#[no_mangle]
pub unsafe extern "C" fn dbao_organize(
    client: *const DbaoClient,
    text: *const c_char,
    out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || out.is_null() {
        set_error("参数为空指针".into());
        return DBAO_ERR_ARG;
    }
    let text = match cstr(text) {
        Ok(t) => t,
        Err(c) => return c,
    };
    match (*client).inner.organize(text) {
        Ok(o) => out_string(out, o.content),
        Err(e) => record(&e),
    }
}

/// 识别一个 WAV 文件（16kHz 单声道 16bit）。`compat` 非 0 时启用兼容模式。
/// 识别文本写入 `*out`（须用 [`dbao_string_free`] 释放）。
///
/// # Safety
/// 同 [`dbao_organize`]，`path` 为 UTF-8 NUL 结尾文件路径。
#[no_mangle]
pub unsafe extern "C" fn dbao_recognize_file(
    client: *const DbaoClient,
    path: *const c_char,
    compat: c_int,
    out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || out.is_null() {
        set_error("参数为空指针".into());
        return DBAO_ERR_ARG;
    }
    let path = match cstr(path) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let opts = AsrOptions {
        compat: compat != 0,
        ..Default::default()
    };
    match (*client).inner.recognize_file(path, opts) {
        Ok(text) => out_string(out, text),
        Err(e) => record(&e),
    }
}

/// 识别一段 PCM（16kHz 单声道 s16le）。`compat` 非 0 时启用兼容模式。
///
/// # Safety
/// `pcm` 指向至少 `len` 字节；其余同上。
#[no_mangle]
pub unsafe extern "C" fn dbao_recognize_pcm(
    client: *const DbaoClient,
    pcm: *const u8,
    len: usize,
    compat: c_int,
    out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || out.is_null() || (pcm.is_null() && len != 0) {
        set_error("参数为空指针".into());
        return DBAO_ERR_ARG;
    }
    let data = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(pcm, len)
    };
    let opts = AsrOptions {
        compat: compat != 0,
        ..Default::default()
    };
    match (*client).inner.recognize_pcm(data, opts) {
        Ok(text) => out_string(out, text),
        Err(e) => record(&e),
    }
}
