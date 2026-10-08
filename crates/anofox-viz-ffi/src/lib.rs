//! C FFI boundary for the render-only `anofox_visualization` DuckDB extension.
//! The C++ shell registers `anofox_render(spec VARCHAR) -> VARCHAR`, which calls
//! [`anofox_viz_render`] here. It delegates to
//! `anofox_visualization::host::render_spec_bytes_checked` (validation + size
//! caps) and reports failures as an error the C++ side raises as a SQL error.
//!
//! No panic ever unwinds across this boundary: every entry point runs inside
//! `catch_unwind` (the crate is built with the default `panic = "unwind"`).

use std::ffi::CString;
use std::os::raw::c_char;

/// Status codes returned by [`anofox_viz_render`].
pub const ANOFOX_VIZ_OK: i32 = 0;
pub const ANOFOX_VIZ_ERROR: i32 = 1;

fn to_c_string(s: String) -> *mut c_char {
    // Interior NULs cannot occur in SVG; strip them from error text defensively.
    let s = if s.contains('\0') {
        s.replace('\0', "")
    } else {
        s
    };
    CString::new(s)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Render the JSON panel spec `spec[0..len]` (UTF-8, need not be
/// NUL-terminated) to SVG.
///
/// Returns [`ANOFOX_VIZ_OK`] with `*out` = the SVG, or [`ANOFOX_VIZ_ERROR`] with
/// `*out` = an error message. Either way `*out` must be released with
/// [`anofox_viz_free`] (it may be null if allocation of the message failed).
///
/// # Safety
/// `spec` must point to `len` readable bytes (or be null with `len == 0`), and
/// `out` must be a valid pointer to write the result to.
#[no_mangle]
pub unsafe extern "C" fn anofox_viz_render(
    spec: *const c_char,
    len: usize,
    out: *mut *mut c_char,
) -> i32 {
    if out.is_null() {
        return ANOFOX_VIZ_ERROR;
    }
    let bytes: &[u8] = if spec.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(spec as *const u8, len)
    };
    let res = anofox_visualization::host::catch_panic(|| {
        anofox_visualization::host::render_spec_bytes_checked(
            bytes,
            &anofox_visualization::host::RenderLimits::default(),
        )
    });
    match res {
        Ok(svg) => {
            *out = to_c_string(svg);
            ANOFOX_VIZ_OK
        }
        Err(e) => {
            *out = to_c_string(e);
            ANOFOX_VIZ_ERROR
        }
    }
}

/// Free a string returned through [`anofox_viz_render`].
///
/// # Safety
/// `p` must be a pointer previously returned by [`anofox_viz_render`] (or null).
#[no_mangle]
pub unsafe extern "C" fn anofox_viz_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn call(spec: &[u8]) -> (i32, String) {
        let mut out: *mut c_char = std::ptr::null_mut();
        let rc = unsafe { anofox_viz_render(spec.as_ptr() as *const c_char, spec.len(), &mut out) };
        let s = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { anofox_viz_free(out) };
        (rc, s)
    }

    #[test]
    fn ok_and_errors() {
        let (rc, svg) =
            call(br#"{"rows":[{"c0":"a","c1":1}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}"#);
        assert_eq!(rc, ANOFOX_VIZ_OK);
        assert!(svg.starts_with("<svg"));
        let (rc, msg) = call(b"not json");
        assert_eq!(rc, ANOFOX_VIZ_ERROR);
        assert!(msg.contains("bad spec JSON"), "{msg}");
        let (rc, msg) = call(b"\xff\xfe");
        assert_eq!(rc, ANOFOX_VIZ_ERROR);
        assert!(msg.contains("UTF-8"), "{msg}");
    }
}
