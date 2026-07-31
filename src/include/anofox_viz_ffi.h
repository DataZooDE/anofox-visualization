/* Manually maintained to match crates/anofox-viz-ffi/src/lib.rs */
#ifndef ANOFOX_VIZ_FFI_H
#define ANOFOX_VIZ_FFI_H
#ifdef __cplusplus
extern "C" {
#endif
/* Render a JSON panel spec to SVG; free the result with anofox_viz_free. */
char *anofox_viz_render(const char *spec);
void anofox_viz_free(char *p);
#ifdef __cplusplus
}
#endif
#endif
