# OpenNOW native streamer

This workspace implements the native GeForce NOW NVST runtime: RTSPS negotiation, the dedicated Mjolnir SRTP video socket, the NVST ICE/DTLS/SCTP bundle used for audio, microphone, RTCP and input, and bounded media queues. It does not implement the browser WebRTC offer/answer or trickle-ICE protocol, and it does not load or redistribute NVIDIA client libraries.

It takes one complete CloudMatch session context from `opennow-core`, reserves its bundle and Mjolnir sockets, and performs OPTIONS, DESCRIBE, SETUP, ANNOUNCE, PLAY, keepalive and TEARDOWN.

## Crates

- `opennow-streamer-protocol`: session and command DTOs.
- `opennow-streamer-core`: NVST lifecycle, command routing, media feedback and recording.
- `opennow-streamer-transport`: Mjolnir SRTP plus the NVST-required ICE/DTLS/SCTP, RTCP and input. No platform dependency; it emits whole access units and Opus packets.
- `opennow-streamer-hid`: controller HID encoding.
- `opennow-streamer-platform` and `opennow-streamer-platform-{windows,macos,linux}`: decode, audio output and GPU frame publication. The library won't decode or draw, so these go once core stops depending on them.

## Microphone upstream

The native RTSP path requires the server's DESCRIBE to advertise
`x-nv-general.rtcMicOnNativeBundle:1`. Only then, and with microphone opt-in,
ANNOUNCE includes that flag and `x-nv-mic.micSsrcConfig.senderSsrc:1`. The existing
str0m bundle sends Opus payload type 111 on SSRC 1, independently of downlink
audio. This follows the native bundle findings documented in OpenNOW-Mac's
`docs/StreamTransportArchitecture.md` (reference revision `88a09bd68598651b367aa4744e7528da9c074d28`).
Legacy RTSP/UDP microphone carriage is not implemented.

Capture produces mono 48 kHz PCM and an off-callback encoder produces 10 ms Opus
frames at 32 kbps. The frame size must match the server's `mic.frameSize:10`: the
cloud PC's microphone stays silent when it receives 20 ms frames. PCM, encoded audio, and transport queues each hold at most five
frames and drop old data under load. Mute closes capture and clears pending audio;
unmute preserves the RTP clock and transport sequence lifetime. Capture or uplink
failure disables only the microphone, not game audio/video. Session termination
also closes capture. Queue drops are reported in microphone diagnostics without
logging audio contents.

## Session liveness and ping

Session liveness follows [OpenNOW-Mac's live-tested keepalive method](https://github.com/OpenCloudGaming/OpenNOW-Mac/blob/90627114383501dd18ef165baa005d9ea603fdf3/GFN/NVST/Rtsp/NvstRtspConnection.swift#L161-L234):
send `GET_PARAMETER` with the RTSP Session header every two seconds over the existing
RTSPS WebSocket, and match the response. A `551 Option Not Supported` response
still completes the keepalive. Some seats do not answer STUN/ICE probes and close the
connection on client WebSocket pings, so those pings are replaced by session-scoped
RTSP keepalives. Server-initiated WebSocket pings are still answered with pongs.

The optional `pingMs` field measures network round-trip time on the active session. It prefers
the nominated ICE candidate pair's measured RTT, then authenticated STUN/NATT replies
on the dedicated video socket or control/audio bundle. RTSPS keepalive response times
are not used: they include application-level request handling and can overstate network latency.
STUN replies must match an outstanding transaction, the peer address, fingerprint, and
message integrity. Each receiver tracks at most 64 probes. ICE statistics only refresh
the sample when the pair's response count changes; rereading old statistics cannot keep
an old measurement alive.

The bundle remembers up to 64 emitted ICE transactions and forwards each authenticated
success response to the ICE library only once. Delayed duplicate replies must not overwrite
the original completion time and turn the age of an old probe into the displayed network RTT.
This does not cap genuine network latency or filter first replies based on their timing.

Session liveness follows [OpenNOW-Mac's live-tested keepalive method](https://github.com/OpenCloudGaming/OpenNOW-Mac/blob/90627114383501dd18ef165baa005d9ea603fdf3/GFN/NVST/Rtsp/NvstRtspConnection.swift#L161-L234):
send `GET_PARAMETER` with the RTSP Session header every two seconds over the existing
RTSPS WebSocket, and match the response. A `551 Option Not Supported` response
still completes the keepalive. Some seats do not answer STUN/ICE probes and close the
connection on client WebSocket pings, so those pings are replaced by session-scoped
RTSP keepalives. Server-initiated WebSocket pings are still answered with pongs.

## Checks

```sh
cargo fmt --manifest-path native/opennow-streamer/Cargo.toml --all -- --check
cargo clippy --manifest-path native/opennow-streamer/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace
```
