# History: decisions, numbers and lessons

Compacted from retired checkpoints and plans (2026-10-08 to 2026-10-12). Current state is in `where-we-left-off.md`.

## 2026-10-08: desktop app removed

The Qt app (739 files), the Playnite plugin, translations, seven Qt CI workflows, the `opennow-streamer-ffi`
crate and two release tools were deleted: 868 files and 144,924 lines. Tests before and after: core 712 to 705
(the 7 gone were the deleted acceptance verifier and license tools), streamer 844 to 809 (the 35 gone were the
ffi crate's tests); no failures either side. The cut was done in stages on purpose: the decode crates could not be
deleted first because `opennow-streamer-core` imported `MediaRuntime` from them to start every session.

## 2026-10-10: one shared rule file

`AGENTS.md` is the single source of rules for Claude Code, Codex, Gemini and Antigravity; `CLAUDE.md` and
`GEMINI.md` import it, and one shared checkpoint lives in `where-we-left-off.md`.

## 2026-10-11: live runs through an OVH POP

- Virginia, HEVC, 1920x1200, 75 Mbps cap. The host hid the cursor in Dota 2 while the client drew its own,
  giving two cursors. `v2pop` now reads GFN's cursor-update events and tells the client when the host hides it.
  Presentation reached 60 fps with 0 dropped frames.
- India booking failed with HTTP 500 `INTERNAL_ERROR_STATUS 8A960000` and worked on retry (17:17:11 UTC, L40S).
  The cause was CloudMatch capacity exhaustion plus VM deprovisioning cooldown in Mumbai at the Sunday evening
  peak (about 21:40 to 21:55 IST). The core now reports it as `session_capacity_unavailable`. The good run
  delivered 2,282 frames at 60 fps (2.66 ms decode) and 7,600+ audio packets in 40 s; the seat was released
  cleanly and the client exited on disconnect.
- Lesson: after an AFK idle warning the India source stopped video at 15:58:18 UTC and the library emitted
  `nvst-recovery-exhausted` after 8.004 s of silence. Transport silence alone does not prove why the provider
  ended the session; do not hide it by pinning timeouts.
- `request-idr` was added as an engine command. Live driver and handover: `same-seat-handover.md`.
- Tests then: streamer 521 passed, core 135 on the OVH box.

## 2026-10-12: loss, cursor and keys

- During Wi-Fi channel hops over 54k frames, FEC rebuilt 532 frames and retransmission recovered 2,457 packets.
- VideoToolbox error -12909 on a lost reference tripped Moonlight's 20-frame decoder watchdog. The client must
  send 0x0301 (invalidate) and hold dependent P-frames instead of resetting.
- Remote cursor tracking now asks for confine level 2 (`0x030d` payload 2), and a 6-byte cached bitmap cursor
  (count 0, no position) is forwarded instead of dropped.
- The POP detects the SRTP rollover counter mid-stream, so a reconnect no longer fails with a key mismatch.
