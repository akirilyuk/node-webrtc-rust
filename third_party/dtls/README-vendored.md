# Vendored `dtls` 0.17.1

- Upstream: https://github.com/webrtc-rs/webrtc, crate `dtls` 0.17.1 (as published on crates.io).
- Why vendored: the published crate stops serving the peer after the handshake completes on the side that sends the last flight, so one lost final-flight datagram leaves the other peer connecting forever.
- Change: the side that sends the final handshake flight keeps it and resends it when the peer retransmits its previous flight (RFC 6347 section 4.2.4). See `src/conn/mod.rs` (`last_flight`, `FINAL_FLIGHT_RESEND_WINDOW`) and `src/handshaker.rs`. Everything else is byte-identical to the published crate.
- Upstream PR: TBD
- Wired in through `[patch.crates-io]` in the workspace `Cargo.toml`. Drop the vendored copy and the patch once upstream releases a fix.
