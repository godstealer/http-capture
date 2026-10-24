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
// Host access to shared history, saved requests, rules, certificates and manual sending while the
// Packet Tunnel is stopped. Pass the App Group's HTTP Capture directory.
char *hc_ios_offline_command(const char *data_dir, const char *request_json);
// Export the stopped tunnel's shared history to a new file in the host app's
// temporary directory. An existing output file is never overwritten.
char *hc_ios_offline_export_session(const char *data_dir, const char *output_path);
// Portable configuration uses the same v1 JSON/.hccfg format as desktop and
// Android. Import only while the tunnel is stopped; restart the app after commit.
char *hc_ios_config_export(const char *data_dir, const char *output_path,
                           const char *options_json, const char *passphrase);
char *hc_ios_config_preview(const char *data_dir, const char *input_path,
                            const char *passphrase);
char *hc_ios_config_import(const char *data_dir, const char *input_path,
                           const char *sections_json, const char *expected_revision,
                           const char *passphrase);
bool hc_ios_input_packet(const uint8_t *packet, size_t length);
void hc_ios_free_string(char *result);

#endif
