# Input, microphone, feedback and timing calls

C ABI added on top of the basic video and input calls (`opennow_gfn.h`, in `crates/opennow-gfn/include`).
The protocol side of the same contract is `~/Documents/Rasbari/protocol/docs/v2/opennow-library.md`.

## Timed input

Six `opennow_gfn_*_at` calls (key, mouse move, absolute mouse, button, wheel, gamepad) take u64 producer
microseconds. `CapturedInputSample.source_timestamp_us` carries that clock through the engine queue into NVST
serialization, so a delayed POP queue drain does not make old input look new. The untimed calls still use the
library's own capture time. POP receipt and liveness use a separate clock. This preserves source time; it is
not an interpretation of NVIDIA's timing.

## Microphone

`opennow_gfn_microphone_available` (1, 0, or -1 invalid or busy), `_enable` (0 accepted, -1 rejected) and
`_opus` (1 to 1275 bytes plus a 48 kHz RTP timestamp; 0 accepted, -1 rejected). They use the existing bounded
5-frame, 100 ms microphone queue and the negotiated RTP/SRTP sender; the POP adds no codec. The negotiated
microphone mode and remote SDP must support it, and acceptance does not prove delivery. Disabling clears
pending audio before the next enable.

## Player-owned feedback and control

- `opennow_gfn_set_player_feedback(enabled)`: hands feedback ownership to the player. Each claim, including a
  takeover, clears the old queue and starts a new generation, so the previous owner's reports are dropped.
- `opennow_gfn_send_control` and `_send_control_deadline` (1 to 84000 us lifetime): non-blocking enqueue of one
  complete native command (4 to 768 bytes) onto the negotiated reliable or partially reliable channel. Allowed
  commands are 0x0202, 0x0207, 0x0203 version 3 (1 to 16 records), 0x0204, 0x020c, 0x0317, 0x0301, 0x0302 and
  0x0312; anything else, or a full queue (64), returns -1. A newer 0x0207 replaces an older one, and expired
  partial-channel items are dropped instead of blocking reliable ones.
- `opennow_gfn_set_control_callback`: receives complete 0x0101, 0x0102, 0x0109 and 0x0112 commands from the
  server, headers and versioned tails intact.
- `opennow_gfn_player_profile` (40 bytes) and `opennow_gfn_player_timing` (12 bytes, the 0x0112 config):
  the server's latest enable or disable overrides the startup seed and survives a player takeover; a new
  session starts clean. Unknown display and decode stages are never invented by the library.
- `opennow_gfn_upstream_rtt_us`: last fresh GFN-leg round trip, 0 when there is none.

## Raw audio and keys

`opennow_gfn_set_audio_callback` delivers each authenticated original Opus RTP packet with the derived receive
key, salt and profile (0 AES-256-GCM, 1 AES-128-GCM), valid only during the call, so the POP can pass audio
through encrypted. Keys come from the DTLS-SRTP export in `nvst_keyexport.rs` and are zeroed on drop. Video
master key material still travels in the RTSPS session context; `v2pop` derives the packet keys from it.

## Cursor

Bitmap cursor notifications (0x0110) are parsed and forwarded; a malformed one keeps its raw diagnostics but
does not trigger the native capture handoff, and zero-count cached cursor updates are valid. Remote cursor
tracking asks for the confine level (`0x030d` payload 2), matching the official client.

Detail and validation of the three-repo work: `~/Documents/Rasbari/protocol/docs/v2/gfn-input-five-implementation-2026-10-13.md`.
