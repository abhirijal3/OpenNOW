# Official GFN client baseline (Windows, 2026-08-30)

What NVIDIA's own client did in one logged session, kept as evidence for tuning our client side. Source: the
GeForce NOW install's `geronimo.log`, `debug.log` and `sharedstorage.json`, build 2.0.87.131, Geronimo branch
`gs_04_90`. Only logs were read; no binary was disassembled. A later build can change command IDs and keep the
same log strings. The macOS client is in `gfn-macos-client-architecture.md`, and line-cited proof is in
`~/Documents/Rasbari/protocol/gfn-ghidra/`.

## Video

- 76 minutes of 10-bit HEVC at 2560x1440, 120 fps, `maxBitrate` 100000 kbps. Video on its own UDP socket
  (client 49005), audio and input on the ICE/DTLS/SCTP bundle (49006), server 5004.
- NACK settings: wait 52 ms, 1024-packet queue, 2048 pending, 3 retries, 4 ms backoff, dynamic FEC on. That
  session recovered 0 packets from FEC and 639 of 677 NACKed packets, so NACK is live for the official client.
- Recovery order: NACK, then flush a stalled decode queue (cap 30 then 40) and send IDR, then invalidate
  the lost frame range, then clear on the next I-frame. It can also freeze the display on a bad reference.
  Totals: 5 complete flushes, 195 flushed frames, 117 decoder drops. No PLI or FIR was sent, and intra-refresh
  was advertised but left off.
- The 438,096 "missed vsyncs" it logged are the vsync-off present path, not 438k stalls.

## Audio

- Opus 48 kHz stereo, RFC 2198 RED level 2 on playback (level 3 on the microphone), in-band FEC off.
- Adaptive jitter buffer: start 80 ms, min 60, max 200; underruns raise the threshold and overbuffer flushes.
- First audio packet arrived about 84 ms after start, before the first video frame. At teardown: 17,412 stale
  drops and 0 RED decode failures.

## Input

- Remote input type 7 is relative i16 dx/dy, type 5 is absolute x/y plus viewport size. System cursor ID 0 means
  hide, lock and relative; other IDs warp the local cursor to `coord / 65535 * window size`
  (32524, 33017 on 2560x1440 becomes 1270, 725).
- On every focus change it tells the host `accel=0, speed=10` (NVB feature type 10). No frame for it was ever
  captured, so we do not send a guess. Feature types seen: 0 cursor capture, 6 haptics, 8 track remote cursor
  image, 10 mouse settings.
- Gamepads: a 38-byte little-endian type-12 packet (payload 26, slot, bitmap, buttons, triggers, sticks, magic 85,
  timestamp), opened through SDL (Xbox One pad, VID 045E PID 02FF). Send nothing and games fall back to
  keyboard and mouse prompts, so keep resending the last state (we use 100 ms).
- Our wire contract for the same packets: `native-gamepad-input.md`.

## Lesson from a blank-video capture (2026-09-04)

Audio working while the video socket shows `inbound=0` for two 8-second timeouts is a video delivery failure
before authentication or decoding. It says nothing about the GPU or codec.
