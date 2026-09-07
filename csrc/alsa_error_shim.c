/* ALSA error handler shim (Linux only).
 *
 * libasound reports problems (e.g. "pcm_dmix.c: unable to open slave" while
 * cpal enumerates PCMs) through a C-variadic callback that prints to stderr by
 * default. Stable Rust cannot define variadic functions, so the callback lives
 * here and hands the formatted line to vtmate_alsa_log() in src/audio.rs,
 * which routes it through crate::log::log().
 *
 * The prototype is declared by hand so no ALSA headers are needed at build
 * time; the symbol itself comes from the libasound cpal already links.
 */
#include <stdarg.h>
#include <stdio.h>

typedef void (*snd_lib_error_handler_t)(const char *file, int line,
                                        const char *function, int err,
                                        const char *fmt, ...);
extern int snd_lib_error_set_handler(snd_lib_error_handler_t handler);
extern void vtmate_alsa_log(const char *msg);

static void vtmate_alsa_error_handler(const char *file, int line,
                                      const char *function, int err,
                                      const char *fmt, ...) {
  char msg[512];
  char out[640];
  va_list ap;
  (void)err;
  va_start(ap, fmt);
  vsnprintf(msg, sizeof msg, fmt ? fmt : "", ap);
  va_end(ap);
  snprintf(out, sizeof out, "ALSA lib %s:%d:(%s) %s", file ? file : "?", line,
           function ? function : "?", msg);
  vtmate_alsa_log(out);
}

void vtmate_install_alsa_error_handler(void) {
  snd_lib_error_set_handler(vtmate_alsa_error_handler);
}
