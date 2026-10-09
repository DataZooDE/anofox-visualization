/* Manually maintained to match crates/anofox-viz-ffi/src/lib.rs */
#ifndef ANOFOX_VIZ_FFI_H
#define ANOFOX_VIZ_FFI_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#define ANOFOX_VIZ_OK 0
#define ANOFOX_VIZ_ERROR 1
/* Render the JSON panel spec spec[0..len] to SVG. Returns ANOFOX_VIZ_OK with
 * *out = SVG, or ANOFOX_VIZ_ERROR with *out = an error message. Always release
 * *out with anofox_viz_free (it may be NULL on allocation failure). Never
 * unwinds: Rust panics are caught and reported as ANOFOX_VIZ_ERROR. */
int anofox_viz_render(const char *spec, size_t len, char **out);
void anofox_viz_free(char *p);
/* The bundled SQL macros (single source: src/macros.rs). All returned strings
 * are static: never free them. */
size_t anofox_viz_macro_count(void);
/* field: 0 name, 1 body, 2 description, 3 example; NULL when out of range. */
const char *anofox_viz_macro_field(size_t i, int field);
/* list: 0 positional params, 1 named params (*second = default SQL),
 * 2 tags (*second = value); NULL past the end. */
const char *anofox_viz_macro_item(size_t i, int list, size_t j, const char **second);
#ifdef __cplusplus
}
#endif
#endif
