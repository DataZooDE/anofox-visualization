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

// ── Bundled SQL macros (single source: `anofox_visualization::macros`) ─────
//
// The C++ shell registers every entry as an internal macro. Strings are
// NUL-terminated copies built once and kept for the life of the process, so
// the returned pointers stay valid (and must NOT be freed).

struct CMacro {
    name: CString,
    body: CString,
    description: CString,
    example: CString,
    params: Vec<CString>,
    named: Vec<(CString, CString)>,
    tags: Vec<(CString, CString)>,
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn c_macros() -> &'static [CMacro] {
    static M: std::sync::OnceLock<Vec<CMacro>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        anofox_visualization::macros::MACROS
            .iter()
            .map(|m| CMacro {
                name: cstr(m.name),
                body: cstr(m.body),
                description: cstr(m.description),
                example: cstr(m.example),
                params: m.params.iter().map(|p| cstr(p)).collect(),
                named: m.named.iter().map(|(n, d)| (cstr(n), cstr(d))).collect(),
                tags: m.tags.iter().map(|(k, v)| (cstr(k), cstr(v))).collect(),
            })
            .collect()
    })
}

/// Number of bundled macros.
#[no_mangle]
pub extern "C" fn anofox_viz_macro_count() -> usize {
    std::panic::catch_unwind(|| c_macros().len()).unwrap_or(0)
}

/// Field `field` of macro `i`: 0 = name, 1 = body, 2 = description,
/// 3 = example. Null when out of range. Static — do not free.
#[no_mangle]
pub extern "C" fn anofox_viz_macro_field(i: usize, field: i32) -> *const c_char {
    std::panic::catch_unwind(|| {
        c_macros()
            .get(i)
            .and_then(|m| match field {
                0 => Some(m.name.as_ptr()),
                1 => Some(m.body.as_ptr()),
                2 => Some(m.description.as_ptr()),
                3 => Some(m.example.as_ptr()),
                _ => None,
            })
            .unwrap_or(std::ptr::null())
    })
    .unwrap_or(std::ptr::null())
}

/// Entry `j` of list `list` of macro `i`: list 0 = positional parameters
/// (`*second` = null), 1 = named parameters (`*second` = default SQL),
/// 2 = tags (`*second` = value). Returns null past the end. Static.
///
/// # Safety
/// `second` must be null or a valid pointer to write to.
#[no_mangle]
pub unsafe extern "C" fn anofox_viz_macro_item(
    i: usize,
    list: i32,
    j: usize,
    second: *mut *const c_char,
) -> *const c_char {
    let r = std::panic::catch_unwind(|| {
        let m = c_macros().get(i)?;
        match list {
            0 => m.params.get(j).map(|p| (p.as_ptr(), std::ptr::null())),
            1 => m.named.get(j).map(|(a, b)| (a.as_ptr(), b.as_ptr())),
            2 => m.tags.get(j).map(|(a, b)| (a.as_ptr(), b.as_ptr())),
            _ => None,
        }
    })
    .ok()
    .flatten();
    let (a, b) = r.unwrap_or((std::ptr::null(), std::ptr::null()));
    if !second.is_null() {
        *second = b;
    }
    a
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

    #[test]
    fn macro_table() {
        let n = anofox_viz_macro_count();
        assert_eq!(n, anofox_visualization::macros::MACROS.len());
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        let names: Vec<String> = (0..n).map(|i| s(anofox_viz_macro_field(i, 0))).collect();
        assert!(names.contains(&"anofox_plot_terms".to_string()));
        let mut second: *const c_char = std::ptr::null();
        let i = names.iter().position(|n| n == "anofox_xy").unwrap();
        assert_eq!(
            s(unsafe { anofox_viz_macro_item(i, 0, 1, &mut second) }),
            "y"
        );
        assert_eq!(
            s(unsafe { anofox_viz_macro_item(i, 1, 0, &mut second) }),
            "kind"
        );
        assert_eq!(s(second), "'BARCHART'");
        assert!(unsafe { anofox_viz_macro_item(i, 1, 9, &mut second) }.is_null());
        assert!(anofox_viz_macro_field(n, 0).is_null());
    }
}
