# HTTP Capture field order extension

Based on the unmodified crates.io `h2` 0.4.19 source, MIT licensed (see LICENSE).
This local fork is selected by the workspace `[patch.crates-io]`. No registry
cache files are modified.

Changes:
- `ext::HeaderOrder(Vec<String>)` preserves every field name occurrence, including
  pseudo headers, as HPACK decoding completes (within the existing header limits).
- Incoming request and final response extensions expose this sequence before
  `HeaderMap` iteration can reorder it. Existing protocol validation still applies.
- Outgoing request extensions can specify a complete sequence. Missing, additional,
  or misplaced pseudo fields are rejected. Values still come from the validated
  request, not the extension. `Host` in place of `:authority` is supported.
- The HPACK encoder receives fields in this sequence; duplicate values retain
  occurrence order and sensitivity flags. Without an extension, upstream behavior
  remains unchanged. Frame splitting and compression state remain library-managed.

This preserves decoded field order, not HPACK representation, frame boundaries,
HTTP/2 SETTINGS, priorities, TLS ClientHello, or response forwarding order.

The capture-core multiplex integration tests exercise ordered pseudo headers,
interleaved duplicates, multiple streams, large bodies, and header-order rejection.
Review/rebase the small source patch when updating h2; do not blindly update the
version in this directory.
