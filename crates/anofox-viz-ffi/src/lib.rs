//! C FFI boundary for the render-only `anofox_visualization` DuckDB extension.
//! The C++ shell registers `anofox_render(spec VARCHAR) -> VARCHAR`, which calls
//! `anofox_viz_render` here; it delegates to `anofox_visualization::render_spec`
//! (JSON panel spec -> SVG string).

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Render a JSON panel spec to an SVG string. Returns a heap-allocated C string
/// the caller must release with [`anofox_viz_free`]; returns null on bad UTF-8.
///
/// # Safety
/// `spec` must be a valid, NUL-terminated C string (or null).
#[no_mangle]
pub extern "C" fn anofox_viz_render(spec: *const c_char) -> *mut c_char {
    if spec.is_null() {
        return std::ptr::null_mut();
    }
    let spec = match unsafe { CStr::from_ptr(spec) }.to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    let svg = anofox_visualization::render_spec(spec);
    // render_spec never embeds NUL; fall back to null on the impossible case.
    CString::new(svg).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// Free a string returned by [`anofox_viz_render`].
///
/// # Safety
/// `p` must be a pointer previously returned by [`anofox_viz_render`] (or null).
#[no_mangle]
pub extern "C" fn anofox_viz_free(p: *mut c_char) {
    if !p.is_null() {
        unsafe { drop(CString::from_raw(p)) };
    }
}
