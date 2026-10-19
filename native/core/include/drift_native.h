#ifndef DRIFT_NATIVE_H
#define DRIFT_NATIVE_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* JSON replies are Rust-owned UTF-8 strings. Free exactly once with
 * drift_string_free, never with C/Swift free(). No credentials are persisted. */
char *drift_start(const char *private_data_directory, const char *device_name);
char *drift_export_path(const char *completed_transfer_id, uint32_t file_index);
void drift_stop(void);
void drift_string_free(char *reply);
#ifdef __cplusplus
}
#endif
#endif
