/* Manually maintained to match crates/anofox-viz-ffi/src/lib.rs */
#ifndef ANOFOX_VIZ_FFI_H
#define ANOFOX_VIZ_FFI_H
#include <stddef.h>
#ifndef __cplusplus
#include <stdbool.h>
#endif
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
/* The anofox integration-contract version (anofox_contract_version()). Static. */
const char *anofox_viz_contract_version(void);
/* Owner decision (#333): default DATAZOO_DISABLE_TELEMETRY to "1" for this
 * process if unset (an explicit value is left alone), so every anofox
 * extension loaded afterwards stays silent by default. Call first thing in
 * LoadInternal, before registering anything; see
 * anofox_visualization::host::telemetry for the thread-safety reasoning. */
void anofox_viz_ensure_telemetry_default(void);
/* Is telemetry currently disabled, under the exact parsing the anofox
 * extensions use ("1"/"true"/"yes")? Call after
 * anofox_viz_ensure_telemetry_default to decide whether the best-effort
 * `SET anofox_telemetry_enabled = false` (for an anofox extension loaded
 * before this one) matches this process's intent. */
bool anofox_viz_telemetry_is_disabled(void);
#ifdef __cplusplus
}
#endif
#endif
