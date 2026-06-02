#ifndef DUALBEAM_PROMISE_DRAG_H
#define DUALBEAM_PROMISE_DRAG_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*db_drop_callback)(uint64_t id, const char *src, const char *dest);

void db_set_drop_callback(db_drop_callback cb);

int db_start_promise_drag(const char *const *paths, int count, const char **out_err);

/* action: 0 = overwrite, 1 = cancel, 2 = keep_both (auto-rename) */
int db_resolve_promise(uint64_t id, int action, const char **out_err);

/* Write file URLs to NSPasteboard.general so other apps (Finder etc.) can paste them. */
int db_clipboard_write_files(const char *const *paths, int count, const char **out_err);

/* Read file URLs from NSPasteboard.general. Returns count (>=0) or negative on error.
 * On success, *out_paths is a malloc'd array of malloc'd UTF-8 strings of size count.
 * Caller must free each string and the array. */
int db_clipboard_read_files(char ***out_paths, const char **out_err);

/* Set (or clear) the Dock tile badge label. Pass NULL or empty string to clear. */
void db_set_dock_badge(const char *label);

/* Render the native macOS icon for a file path as PNG bytes.
 * size is the desired pixel size (square). On success returns 0 and sets
 * *out_png to a malloc'd buffer of *out_len bytes (caller frees). On error
 * returns non-zero and may set *out_err (malloc'd, caller frees). */
int db_file_icon_png(const char *path, int size, unsigned char **out_png,
                     int *out_len, const char **out_err);

/* Remove macOS-injected text-service items (AutoFill, Writing Tools, Emoji &
 * Symbols, Start Dictation, Substitutions, Speech, ...) from the app's Edit
 * menu, keeping only the standard Cut/Copy/Paste/Select All entries.
 * Must be called on the main thread after the app menu has been installed. */
void db_clean_edit_menu(void);

#ifdef __cplusplus
}
#endif

#endif
