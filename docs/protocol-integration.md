# Integrating OpenNOW with our protocol

This is the OpenNOW side of the v2 "GFN client" role: what the library does, how the POP program wires it in,
and where to read more. The protocol side's view is `~/Documents/Rasbari/protocol/docs/v2/opennow-library.md`,
and the design it serves is `~/Documents/Rasbari/protocol/docs/v2/DESIGN.md` (index: `docs/v2/README.md` there).
Library API, threading or behavior changes get written here and in that protocol file.

## The shape

v2 is pass-through. GFN's video packets reach the player unchanged except for 9 header bytes the POP
rewrites (`protocol/v2/src/video.c`); the player decrypts with GFN's own key. So OpenNOW's job on the POP is
small: hold a normal GFN session, hand every raw video packet to the POP the moment it arrives, keep GFN
happy with acks and QoS, and act on the levers the protocol pulls.

Two programs do it:

- `opennow-core` (Rust, `native/opennow-core`) signs in to NVIDIA, books the seat through CloudMatch and
  builds the session context. It is a child process speaking newline-delimited JSON on stdin/stdout
  (`docs/core-protocol.md`). Logins are plain JSON files under its data directory; it never uses a keychain.
- `libopennow_gfn.a` (Rust, `native/opennow-streamer/crates/opennow-gfn`, header
  `include/opennow_gfn.h`) runs the stream: RTSPS, the Mjolnir video socket, the ICE/DTLS/SCTP bundle,
  input, QoS, recovery. The POP links it.

## Explicit clipboard text (2026-10-19, PLAYER-13)

`opennow_gfn_text` now exposes the existing bounded native Unicode submission
path to the POP. It accepts 1..65536 bytes of valid UTF-8 with no NUL; 0 means
queued, -1 means invalid, unavailable or busy. `opennow_gfn_text_available` is
1 when native input is ready, 0 when unavailable, -1 for NULL. It does not
measure the host's CustomImeConfig or game text-box acceptance.

A successful submission atomically appends releases for both Shift/Ctrl/Alt/Win
keys, then the Unicode payload to the shared input queue. Invalid/full/busy
submissions append nothing. The existing lifecycle generation cancels pending
text on stop/restart; the native transport chunks at UTF-8 character boundaries.
This is typing into a focused caret, with no Ctrl+V and no clipboard overwrite.
The POP logs only metadata. No custom-message clipboard API or reliable multi-
POP clipboard forwarding is supplied here. ABI regression and streamer workspace
checks run without GFN access; live acceptance remains unverified.

## Building

```
cargo build --release --manifest-path native/opennow-core/Cargo.toml
cargo build --release -p opennow-gfn --manifest-path native/opennow-streamer/Cargo.toml
```

Link `native/opennow-streamer/target/release/libopennow_gfn.a` with `-lpthread -ldl -lm` on Linux.
`tools/gfn-collector/build.sh` is a working example. Build from the repo root so `.cargo/config.toml`
applies (it turns on ARMv8 AES for aarch64; x86 picks up AES-NI by itself).

## Starting a session, step by step

1. Start `opennow-core --data-dir <dir>` and send `core.hello` with `protocolVersion` 6.
2. `auth.session.get`. With no saved login: `auth.device.start` (gives a URL and an 8-digit code a person
   approves once), poll `auth.device.poll` until `authorized`, then `auth.device.complete`. The login is
   saved; later starts skip this.
3. `network.regions.list`, then `settings.set` `region` to the region's URL (India is
   `https://ap-india.cloudmatchbeta.nvidiagrid.net/`).
4. `session.create` with `appId`, `variantId` (same number), `scope` from the auth result, optional
   `title`. Send `{"type":"ack","id":"<request id>"}` within 10 seconds or the seat is deleted.
5. Poll `session.poll` with the session until `status` is 2 or 3 (queue and seat setup take about 15 s).
6. `streamer.prepare` with the session gives the session `context`.
7. In the POP: `opennow_gfn_create(&callbacks)`, then `opennow_gfn_command` with
   `{"id":"1","type":"hello","protocolVersion":7}` and
   `{"id":"2","type":"start","context":<context>}`. Video flows about 5 s later.
8. To end: `{"type":"stop"}` through `opennow_gfn_command`, `opennow_gfn_destroy`, then `session.stop`
   in `opennow-core` so the seat is released.

`tools/gfn-collector/run.py` does steps 1 to 8 end to end and is the reference.

## What comes out of the library

- `on_video_packet(user, data, len)`: every GFN video datagram exactly as it arrived, data and parity, in
  arrival order, on the library's video thread, before the library looks at it. The bytes are valid only
  during the call, so copy them into a pool slot, then do `v2_video_wrap` there and send. This is the
  real-packet replacement for `gfnsim` behind `send_video` in `protocol/v2/src/server.c`.
- `on_media(user, media)`: Opus audio (`kind` 1). With `on_video_packet` set, no video comes here.
- `on_event(user, json, len)`: engine events and command replies as JSON: session state, `cursor-update`
  (GFN's cursor message, base64), telemetry, errors, and `termination` when the session ends.

Example: GFN sends piece 3 of frame 500, 1,304 bytes. `on_video_packet` gets those 1,304 bytes; the POP
copies them, rewrites 9 bytes and sends them to the player. Meanwhile the library notes from the clear
headers that frame 500 now has enough packets, acks it to GFN, and folds the packet into the next 0x0207.

## Header-only mode

Setting `on_video_packet` puts the library in header-only mode: it never decrypts, FEC-repairs, reorders
or assembles video. It reads the RTP and GS headers, which SRTP GCM-8 authenticates but leaves in the
clear, and from them sends GFN what a client owes it: frame acks (a frame counts once each FEC group has
enough data plus parity for the player to rebuild it), the 0x0207 bandwidth/loss/jitter samples, and a
keyframe request if no frame completes for too long. It sends no NACKs on GFN's leg in this mode.

## What goes in

- Input: `opennow_gfn_key`, `_mouse_move`, `_mouse_button`, `_mouse_wheel`, `_gamepad`, from any thread, and the
  matching `_at` calls that carry the producer's own microsecond clock. The library builds GFN's 0x0206/0x020d
  packets and puts them on the right SCTP channel. Input wakes the library at once; nothing waits on a timer.
- Engine commands as JSON through `opennow_gfn_command`: `hello`, `start`, `stop`, `shutdown`,
  `anti-afk-pulse`, `request-idr` (acts on the active session, otherwise answers `no-active-session`).
- Player-owned control, microphone and profile calls: see `integration-input-timing-mic.md`.

Keep input flowing or GFN treats the seat as idle. The collector nudges the mouse one pixel a second and
sends `anti-afk-pulse` every minute.

## Threads

The library runs its own threads per session: the bundle (control, audio, QoS), Mjolnir (video), RTSPS
keepalive, event forwarding, plus the wrapper's media and event threads. Callbacks run on those threads,
so what they touch on the POP side must be safe there. Sending on a UDP socket from the video thread is.


## More

- `integration-input-timing-mic.md`: timed input, microphone, player feedback, timing and profile calls, raw audio and control taps, key export.
- `integration-measured.md`: measured CPU, memory and stream rates from the first live run (2026-10-08).
- `integration-open-work.md`: what is still open on the library and POP side.
- `same-seat-handover.md`: the live driver and reclaiming a running official-client seat.
- `core-protocol.md`: the seat-booking program the POP starts first.


## D2 measured video route (October 20, 2026)

`POPCTL-9`, `POPSW-2`: the controller puts the player's selected video UDP budget in `session.networkTest.measuredDatagramBytes`. ANNOUNCE uses the negotiated SRTP tag length and 16-byte alignment; an explicit unusable measurement fails `nvst-video-mtu-invalid`. The raw encrypted-video tap requires `AEAD_AES_256_GCM_8` because that is the current protocol/player key format, and refuses other profiles explicitly. This does not remove ordinary receiver profile support. A prebuilt handoff must also fit the route budget.

New additive C ABI `uint16_t opennow_gfn_video_datagram_bytes(OpennowGfn*)` returns the negotiated whole GCM8 UDP size, or zero for NULL/unprepared/stopped. The POP reads it after start, validates the budget and publishes that actual size to the player. Build the matching library and POP together; do not assume a rebuilt POP can link an older library. The getter locks only on the setup path; no per-packet work was added in OpenNOW.

`cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace` passed 548 tests. `cargo build --release -p opennow-gfn --manifest-path native/opennow-streamer/Cargo.toml` passed. Logs `/tmp/rasbari-d2-rust-final-tests.log`, `/tmp/rasbari-d2-rust-build.log`. No live GFN seat was started by this chat. Full cross-repo flow, repair envelopes and exact native/controller tests: sibling `protocol/docs/v2/mtu-packet-sizing.md`.

## Startup metadata lifetime (October 23, 2026)

POPCTL-9: stream startup first clears the previous media resources, then publishes the newly negotiated video datagram size, player profile and timing metadata. Clearing resources after publishing these values erased them before v2pop could read the size, so a successful start returned zero through `opennow_gfn_video_datagram_bytes` and v2pop refused the stream. The metadata remains available after a successful start and is cleared on stop or a failed transport start. The regression `relay_start_preserves_negotiated_video_metadata_until_stop` starts a real local UDP receiver with a 1195-byte route budget, verifies the negotiated 1192-byte datagram and profile/timing, then verifies stop clears them. It needs no NVIDIA session.
