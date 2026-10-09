# Integrating OpenNOW with our protocol

Written 2026-10-08 on branch `opennow-library` (abhirijal3/OpenNOW). This is the OpenNOW side of the v2
"GFN client" role: what the library does today, how the POP program wires it in, and what is left. The
protocol side's view of the same thing is `~/Documents/GitHub/protocol/docs/v2/opennow-library.md`, and the
design it serves is `~/Documents/GitHub/protocol/docs/v2/DESIGN.md` (index: `docs/v2/README.md` there).

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

- Input: `opennow_gfn_key`, `_mouse_move`, `_mouse_button`, `_mouse_wheel`, `_gamepad`, from any thread.
  The library builds GFN's 0x0206/0x020d packets and puts them on the right SCTP channel. Input wakes the
  library at once; nothing waits on a timer.
- Engine commands as JSON through `opennow_gfn_command`: `hello`, `start`, `stop`, `shutdown`,
  `anti-afk-pulse`.

Keep input flowing or GFN treats the seat as idle. The collector nudges the mouse one pixel a second and
sends `anti-afk-pulse` every minute.

## Threads

The library runs its own threads per session: the bundle (control, audio, QoS), Mjolnir (video), RTSPS
keepalive, event forwarding, plus the wrapper's media and event threads. Callbacks run on those threads,
so what they touch on the POP side must be safe there. Sending on a UDP socket from the video thread is.

## Measured (2026-10-08, GFN India, L40S in Mumbai, from an M1 Mac)

| | Library CPU (one core) | Notes |
|---|---|---|
| Start of the day | about 8% | 1 ms bundle polling, 250 us event polling, 40 NAT pings/s |
| Wakeup fixes | 6.4% | |
| Header-only mode | 1.5% | bundle 0.93%, video 0.43%, events 0.03% |

Most of the remaining 1.5% is macOS's unconnected `sendto` (about 95 us a call); the same call on Linux
measured 1.4 to 3.9 us, so Linux should land well under 1% per session. Live heap is about 1.3 MB per
session. GFN accepted it: every frame acked (4,106 in 70 s at 60 fps), QoS reports at about 19 a second.
The stream itself on a menu screen: 180 to 400 video packets a second, 1.3 to 3.8 Mbps, 40% of video
packets GFN parity, 200 Opus packets a second.

## What is left

In the order that gets a real GFN frame to a player soonest:

1. Export the video SRTP key, salt, SSRC and profile to C. `v2`'s accept needs them so the player can
   restore GFN's header and decrypt. The library already has them (it puts the key in the ANNOUNCE).
   Without this, everything forwarded is unreadable.
2. Wire the library into `protocol/v2/src/server.c` in place of `gfnsim` (protocol side).
3. The levers as C calls: the rate into 0x0207's bandwidth field with utilisation kept low, 0x0318 max
   bitrate, 0x0301 invalidate a frame range, 0x0302 keyframe.
4. Audio: find out whether str0m will hand over its DTLS-SRTP keys so audio passes through encrypted like
   video (`wrapping-gfn.md`); otherwise keep handing the POP decrypted Opus.
5. Pass GFN's input bytes through raw if the player builds them (`wrapping-gfn.md` proposes it), and
   GFN's other player-facing commands (bitmap cursor 0x0110, haptics, HDR, terminate 0x0109) as raw bytes
   instead of JSON events.
6. A Linux capacity test: several sessions on one Ubuntu box, CPU and RAM per session. One NVIDIA account
   gets one seat, so it needs several accounts.
7. Many accounts in `opennow-core`: today it holds one active account per process.
8. Later, if measurement asks for it: one event loop for every session instead of threads per session.

To revisit: the library asks GFN for `x-nv-vqos[0].drc.enable:0`, so GFN never changes resolution, only
bitrate. Whether the middleman should allow it is open (Abhi, 2026-10-08).

Open questions on the protocol side that touch this: DESIGN.md's "Four pieces" text still says whole
access units while the code is pass-through; whether the parity line in `wrapping-gfn.md` means forward or
drop; one loop for many sessions against v1's one process per session; and the terms-of-use question about
relaying a GFN session.
