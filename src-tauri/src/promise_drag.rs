use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::OnceLock;

use tauri::{AppHandle, Emitter};

type DropCallback = extern "C" fn(id: u64, src: *const c_char, dest: *const c_char);
type DockCallback = extern "C" fn();

extern "C" {
    fn db_set_drop_callback(cb: DropCallback);
    fn db_start_promise_drag(
        paths: *const *const c_char,
        count: c_int,
        out_err: *mut *const c_char,
    ) -> c_int;
    fn db_resolve_promise(id: u64, action: c_int, out_err: *mut *const c_char) -> c_int;
    fn db_clipboard_write_files(
        paths: *const *const c_char,
        count: c_int,
        out_err: *mut *const c_char,
    ) -> c_int;
    fn db_clipboard_read_files(
        out_paths: *mut *mut *mut c_char,
        out_err: *mut *const c_char,
    ) -> c_int;
    fn db_set_dock_badge(label: *const c_char);
    fn db_install_dock_menu(title: *const c_char, cb: DockCallback);
    fn db_quick_look(paths: *const *const c_char, count: c_int);
    fn db_file_icon_png(
        path: *const c_char,
        size: c_int,
        out_png: *mut *mut u8,
        out_len: *mut c_int,
        out_err: *mut *const c_char,
    ) -> c_int;
}

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

extern "C" fn on_drop(id: u64, src: *const c_char, dest: *const c_char) {
    if src.is_null() || dest.is_null() {
        return;
    }
    let src_s = unsafe { CStr::from_ptr(src) }.to_string_lossy().to_string();
    let dest_s = unsafe { CStr::from_ptr(dest) }
        .to_string_lossy()
        .to_string();
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "dualbeam://promise-drop",
            serde_json::json!({ "id": id, "src": src_s, "dest": dest_s }),
        );
    }
}

pub fn init(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());
    unsafe {
        db_set_drop_callback(on_drop);
    }
}

#[tauri::command]
pub fn start_promise_drag(paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no paths".to_string());
    }
    let cstrings: Vec<CString> = paths
        .iter()
        .map(|p| CString::new(p.clone()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    let mut err: *const c_char = std::ptr::null();
    let r =
        unsafe { db_start_promise_drag(ptrs.as_ptr(), ptrs.len() as c_int, &mut err as *mut _) };
    if r == 0 {
        Ok(())
    } else {
        let msg = if err.is_null() {
            format!("error {}", r)
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string();
            unsafe { libc::free(err as *mut libc::c_void) };
            s
        };
        Err(msg)
    }
}

#[tauri::command]
pub fn resolve_promise_drop(id: u64, action: String) -> Result<(), String> {
    let act = match action.as_str() {
        "overwrite" => 0,
        "cancel" => 1,
        "keep_both" => 2,
        _ => return Err(format!("invalid action: {}", action)),
    };
    let mut err: *const c_char = std::ptr::null();
    let r = unsafe { db_resolve_promise(id, act, &mut err as *mut _) };
    if r == 0 {
        Ok(())
    } else {
        let msg = if err.is_null() {
            format!("error {}", r)
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string();
            unsafe { libc::free(err as *mut libc::c_void) };
            s
        };
        Err(msg)
    }
}

pub fn clipboard_write_files(paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no paths".to_string());
    }
    let cstrings: Vec<CString> = paths
        .iter()
        .map(|p| CString::new(p.clone()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    let mut err: *const c_char = std::ptr::null();
    let r =
        unsafe { db_clipboard_write_files(ptrs.as_ptr(), ptrs.len() as c_int, &mut err as *mut _) };
    if r == 0 {
        Ok(())
    } else {
        let msg = if err.is_null() {
            format!("error {}", r)
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string();
            unsafe { libc::free(err as *mut libc::c_void) };
            s
        };
        Err(msg)
    }
}

pub fn clipboard_read_files() -> Result<Vec<String>, String> {
    let mut out_ptrs: *mut *mut c_char = std::ptr::null_mut();
    let mut err: *const c_char = std::ptr::null();
    let n = unsafe { db_clipboard_read_files(&mut out_ptrs as *mut _, &mut err as *mut _) };
    if n < 0 {
        let msg = if err.is_null() {
            format!("error {}", n)
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string();
            unsafe { libc::free(err as *mut libc::c_void) };
            s
        };
        return Err(msg);
    }
    let mut paths = Vec::with_capacity(n as usize);
    if n > 0 && !out_ptrs.is_null() {
        for i in 0..(n as isize) {
            let p = unsafe { *out_ptrs.offset(i) };
            if !p.is_null() {
                let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().to_string();
                paths.push(s);
                unsafe { libc::free(p as *mut libc::c_void) };
            }
        }
        unsafe { libc::free(out_ptrs as *mut libc::c_void) };
    }
    Ok(paths)
}

pub fn set_dock_badge(label: Option<String>) {
    match label {
        Some(s) if !s.is_empty() => {
            if let Ok(c) = CString::new(s) {
                unsafe { db_set_dock_badge(c.as_ptr()) };
            }
        }
        _ => unsafe { db_set_dock_badge(std::ptr::null()) },
    }
}

extern "C" fn on_dock_new_window() {
    if let Some(app) = APP_HANDLE.get() {
        crate::open_new_window(app);
    }
}

/// Install the Dock right-click menu with a "New Window" entry.
pub fn install_dock_menu(title: &str) {
    if let Ok(c) = CString::new(title) {
        unsafe { db_install_dock_menu(c.as_ptr(), on_dock_new_window) };
    }
}

/// Show the native Quick Look preview panel (Finder's spacebar preview) for
/// the given file paths. Re-triggering the same selection toggles it closed.
pub fn quick_look(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("keine Pfade angegeben".into());
    }
    let cstrings: Vec<CString> = paths
        .iter()
        .map(|p| CString::new(p.as_str()))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    unsafe { db_quick_look(ptrs.as_ptr(), ptrs.len() as c_int) };
    Ok(())
}

pub fn file_icon_png(path: &str, size: u32) -> Result<Vec<u8>, String> {
    let c = CString::new(path).map_err(|e| e.to_string())?;
    let mut out_png: *mut u8 = std::ptr::null_mut();
    let mut out_len: c_int = 0;
    let mut err: *const c_char = std::ptr::null();
    let r = unsafe {
        db_file_icon_png(
            c.as_ptr(),
            size as c_int,
            &mut out_png as *mut _,
            &mut out_len as *mut _,
            &mut err as *mut _,
        )
    };
    if r != 0 {
        let msg = if err.is_null() {
            format!("error {}", r)
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string();
            unsafe { libc::free(err as *mut libc::c_void) };
            s
        };
        return Err(msg);
    }
    if out_png.is_null() || out_len <= 0 {
        return Err("empty icon".to_string());
    }
    let bytes = unsafe { std::slice::from_raw_parts(out_png, out_len as usize) }.to_vec();
    unsafe { libc::free(out_png as *mut libc::c_void) };
    Ok(bytes)
}
