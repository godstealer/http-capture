#ifndef HTTP_CAPTURE_IOS_H
#define HTTP_CAPTURE_IOS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// The callback must copy packet bytes before returning. It can be called from a
// Rust runtime thread and must remain valid until hc_ios_stop has returned.
typedef void (*hc_ios_packet_callback)(const uint8_t *packet, size_t length, void *context);

// Every char* result is a JSON object {"ok":true,"value":...} or
// {"ok":false,"error":"..."}. Release it with hc_ios_free_string.
char *hc_ios_start(const char *data_dir, uint16_t port, bool capture_quic,
                   hc_ios_packet_callback callback, void *context);
char *hc_ios_stop(void);
char *hc_ios_command(const char *request_json);
bool hc_ios_input_packet(const uint8_t *packet, size_t length);
void hc_ios_free_string(char *result);

#endif
