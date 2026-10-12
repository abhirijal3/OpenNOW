# GeForce NOW Protocol Parity and Implementation Map

This document establishes the complete wire protocol catalog of NVIDIA's GeForce NOW (Mjolnir / NVST), our current v2 implementation across the client and POP, and the step-by-step 1-by-1 execution plan to achieve complete parity with GFN's official client.

---

## 1. Downlink: Rig to Client

On the downlink, GFN separates media from control. Media datagrams arrive over UDP via RTP and SRTP, while state, cursor, and control updates travel over an ordered SCTP data channel. Our POP acts as a cut-through relay: it performs an in-place 9-byte header rewrite on media datagrams to route over our single UDP socket, while intercepting SCTP control packets and translating them into our unified `V2_PKT_MSG` message layer.

### 1.1 Media Stream Datagrams

| GFN Packet / Channel | Wire Layout & Semantics | Current v2 Implementation | Parity Gaps & Next Action |
|---|---|---|---|
| **Video RTP (PT 101)**<br>UDP media port | 12-byte RTP header + 4-byte extension tag (`0x4753` "GS") + 16-byte NVIDIA header + AES-GCM encrypted Annex-B bitstream. Typical MTU 1408 B. | POP rewrites 9 bytes of the 32-byte header into `V2_PKT_DATA` (0x10). Client restores original 9 bytes and validates GFN's AES-GCM tag in-place via `gfn_srtp_open`. | **Parity achieved on wire.** Verify short-tail padding logic in `rx.c` so corrupted fragments never reach the decoder. |
| **Video Parity Shards (PT 101)**<br>UDP media port | Cauchy Reed-Solomon parity packets over GF(2^8) appended to each FEC group (2 to 30 shards per frame). | In-place 9-byte rewrite into `V2_PKT_DATA`. `rx.c` stores parity in a per-group buffer and triggers `try_rebuild` when `have + parity == sources`. | **Parity achieved on wire.** Rebuilds missing shards in-place without memory allocation. |
| **Video NACK Resend (PT 106)**<br>UDP media port | Retransmitted video shard with RTP payload type rewritten to 106 and RTP timestamp updated to resend send time. | Forwarded with bit 0 of our spare header set to mark resend. `rx.c` tracks resend arrivals separately from first-send link metrics. | **Parity achieved on wire.** Client ignores retransmits if FEC group already rebuilt the missing packet. |
| **Audio RTP (PT 111 / RED 63)**<br>UDP audio port (48000) | 5 ms Opus CELT packets (`mode 1002`, 48 kHz stereo, 200 pkt/s), AES-256-GCM encrypted, protected by RS(6, 4) parity. | In-place rewrite into `V2_PKT_AUDIO` (0x16). Client verifies GFN's AEAD tag and feeds Moonlight's sound system. | Need client-side Cauchy RS(6, 4) audio FEC decoder in `audio.c` to rebuild 2 lost audio packets per 6-packet window without RTT delay. |

### 1.2 Downlink Control & Notice Commands (SCTP → V2_PKT_MSG)

| GFN Command | Channel & Reliability | Payload & Purpose | Current v2 Implementation | Parity Gaps & Next Action |
|---|---|---|---|---|
| **0x0101**<br>QoS Info | SCTP 0<br>Reliable | 80 bytes: Server-measured RTT, stream ID, and quality breakdown (downlink, uplink, GPU, WiFi score). | Read by POP for telemetry logging; discarded before reaching client. | **Gap**: Forward server RTT to client HUD to eliminate "unavailable" latency stats in UI. |
| **0x0102**<br>Command Outcome | SCTP 0<br>Reliable | 4 bytes: `u16 command`, `u16 outcome` (0 = OK, 1 = Error). | Consumed internally by POP or OpenNOW driver. | **Parity achieved.** |
| **0x0103 / 0x0104 / 0x0105**<br>Termination Timers | SCTP 0<br>Reliable | 8 bytes: `u32 timer_type` (1 to 8), `u32 seconds_remaining`. Warns of pending idle/AFK shutdown. | Forwarded as `V2_PKT_MSG` kind 0x05 (Notice). Surfaced in client logs. | **Parity achieved.** Connect notice to client onscreen alert banner. |
| **0x0109**<br>Session Terminate | SCTP 0<br>Reliable | Big-endian result code + text string. Cloud instance is deprovisioning or terminating. | Forwarded as `V2_PKT_MSG` kind 0x05 (Notice), triggering client shutdown. | **Parity achieved.** |
| **0x010b**<br>Rumble / Haptics | SCTP 0<br>Reliable | 4-byte header + records of `u16 pad`, `u16 low_motor`, `u16 high_motor`, `u16 duration`. | Forwarded as `V2_PKT_MSG` kind 0x04 (Rumble) and kind 0x09 (Haptics Enable). | **Parity achieved.** Verified driving gamepad motors via SDL. |
| **0x010e**<br>HDR Mode | SCTP 0<br>Reliable | 8 bytes: `u32 mode` (0 SDR, 1/2 HDR10 PQ), `u32 stream`. | Evaluated during stream setup to configure layer pixel format (BGRA8 vs BGR10A2). | **Parity achieved.** |
| **0x010f**<br>System Cursor | SCTP 0<br>Reliable | `u32 id` (0 hides and locks), optional `u16 x, y` (0..65535 normalized), optional confine flag. | Forwarded as `V2_PKT_MSG` kind 0x02 (Cursor State: 8 bytes latest-wins). Client updates native cursor. | **Parity achieved.** Client locks on ID 0 and warps local pointer before unhiding. |
| **0x0110**<br>Bitmap Cursor | SCTP 0<br>Reliable | `u32 id`, `u16 len` (0 = cache hit), `u16 fmt`, `u16 hot_x, hot_y`, 32-bit BMP data, normalized position, scale. | Forwarded as `V2_PKT_MSG` kind 0x03 (Cursor Shape blob). Client caches bitmap and creates `NSCursor`. | **Parity achieved.** Bitmaps cached by ID across session; 6-byte cache hit messages respected. |
| **0x0408**<br>Audio Layout | SCTP 0<br>Reliable | 12 bytes: Channels, coupled streams, and 8-byte speaker channel mapping. | Forwarded as `V2_PKT_MSG` kind 0x06 (Audio Layout). Unlocks client audio processing. | **Parity achieved.** |
| **Media Keys**<br>*(Internal Setup)* | RTSP / OpenNOW C ABI | 32-byte video AES key + 12-byte salt, 32-byte audio key + 12-byte salt, SSRCs, codec, FPS. | POP packages into 108-byte `V2_PKT_MSG` kind 0x01 (Keys). Client installs SRTP context, wipes secrets, and ACKs. | **Parity achieved.** GFN video gate opens only after client ACKs keys. |

---

## 2. Uplink: Client to Rig

On the uplink, our client transmits inputs, acknowledgments, and recovery triggers over UDP to the POP. The POP unwraps these packets and calls OpenNOW / GFN driver functions, which serialize them onto GFN's SCTP channels.

| GFN Wire Command | GFN Channel & Wire Format | Current v2 Implementation | Parity Gaps & Next Action |
|---|---|---|---|
| **0x0200**<br>Keepalive | SCTP 0<br>Reliable | `u16 0x0200`, `u16 len 4`, `u32 stream`. Dispatched every 3.0 seconds. | Handled between client and POP via `V2_PKT_KEEPALIVE` every 500 ms; POP dispatches GFN keepalive to rig. | **Parity achieved.** |
| **0x0206 / 0x020d**<br>Remote Input | SCTP 0 (keys/mouse)<br>SCTP 10 (gamepad) | Key/mouse events wrapped in 0x0e timestamp envelope (52 B per key). Gamepads use 0x020d with 9-byte timestamp header. | Client batches motion on 4 ms timer into `V2_PKT_INPUT`; flushes clicks immediately. POP translates to GFN commands. | **Gap**: Verify POP passes absolute mouse coordinates to GFN rather than dropping them. |
| **0x0207**<br>QoS Feedback | SCTP 6<br>Partially reliable | 52 bytes sent every 50 ms: BWE (kbps), round-trip delay, jitter, loss fraction, decoder FPS, renderer FPS. | POP synthesizes GFN 0x0207 from our rate target and client stats. Feeds NVENC rate governor. | **Gap**: Ensure client reports real decoder/render FPS so GFN doesn't throttle or over-drive slow hardware. |
| **0x0202**<br>Audio QoS | SCTP 6<br>Partially reliable | 124 bytes sent every 70 ms: Audio packets received, packets lost, jitter buffer delay, underruns. | POP synthesizes 0x0202 from audio packet counters and transmits to GFN audio channel. | **Parity achieved.** |
| **0x020c**<br>Burst Loss Report | SCTP 6<br>Partially reliable | Dispatched per lossy frame: `lostBytes = lost_packets * packet_size`. | Generated by POP when client reports unrecoverable missing frame shards. | **Parity achieved.** Governs server FEC adaptation. |
| **0x020b**<br>NACK Feasibility | SCTP 0<br>Reliable | Enables or disables retransmission: active when RTT < 48 ms, disabled when RTT ≥ 52 ms. | POP evaluates round-trip time and controls NACK state toward GFN. | **Parity achieved.** |
| **0x0301**<br>Reference Invalidation | SCTP 0<br>Reliable | `u16 stream`, `u32 first_frame`, `u32 last_frame`. Commands NVENC to encode P-frame off older reference. | Client emits `V2_PKT_RECOVERY` (type Invalidation). POP translates directly to GFN command 0x0301. | **Parity achieved.** Prevents full-frame IDR bitrate spikes during intermediate loss. |
| **0x0302**<br>IDR Keyframe Request | SCTP 0<br>Reliable | Demands immediate full IDR keyframe from NVENC. | Client emits `V2_PKT_RECOVERY` (type IDR). POP transmits GFN command 0x0302. | **Parity achieved.** Used only as last-resort recovery. |
| **0x0312**<br>Recovery Mode | SCTP 0<br>Reliable | Switches stream to intra-refresh slice waves. | Supported in `recovery.c` as secondary recovery step. | **Parity achieved.** |
| **0x0315 / 0x0316**<br>Policer Adaptation | SCTP 6<br>Partially reliable | 0x0315: Loss histogram by frame size. 0x0316: Requests server reduce MTU / packet size. | Partially supported in POP; client does not yet trigger automatic MTU reductions on frame-size loss spikes. | **Gap**: Implement frame-size loss histogram in client to detect mobile hotspot / router policers. |
| **0x0317**<br>RTP NACK Request | SCTP 6<br>Partially reliable | 10-byte records with 64-bit masks: `u16 base_seq`, `u64 mask` (covering 65 sequences per record). | Client sends `V2_PKT_ACK` to POP; POP maps missing packet runs into GFN 0x0317 requests. | **Gap**: Shift 4 ms poll timer and attempt limit (`min(3, 52/RTT)`) to client so NACK timing matches GFN 1:1. |
| **0x0318**<br>Max Bitrate Ceiling | SCTP 0<br>Reliable | `u32 stream`, `u32 max_kbps` (1,000 to 100,000 kbps). Caps host NVENC bitrate ceiling. | Client passes requested bitrate ceiling in stream options; POP transmits 0x0318 to GFN. | **Parity achieved.** |
| **0x0320 / 0x0321**<br>Window / System State | SCTP 0<br>Reliable | Reports window focus, active state, and frame number. Unlocks host cursor updates. | Transmitted by POP during session startup to initialize host input and cursor hooks. | **Parity achieved.** |
| **0x0300 / 0x030b**<br>Disconnect | SCTP 0<br>Reliable | Graceful session termination. | Dispatched when client sends `V2_PKT_END`. POP reaps remote seat cleanly. | **Parity achieved.** |

---

## 3. Step-by-Step 1-by-1 Execution Plan

To reach 1:1 parity with the official client, we will work through these specific protocol items in order:

### Step 1: Input Synchronization and Absolute Pointer Forwarding
- **Objective**: Match GFN's input smoothness and cursor positioning.
- **Actions**:
  1. Audit `popin.c` to ensure `POP_CMD_MOUSE_ABS` events from `V2_PKT_INPUT` are forwarded to OpenNOW's `opennow_gfn_mouse_move` with absolute coordinates instead of being discarded.
  2. Verify 4 ms client-side motion batching: ensure mouse moves buffer into microsecond-tagged sub-events, while mouse button clicks and keyboard transitions flush immediately.
  3. Validate window blur handling: ensure `SDL_WINDOWEVENT_FOCUS_LOST` sends synthetic releases for all held keys and mouse buttons.

### Step 2: NACK Timing and Retransmission Alignment
- **Objective**: Match GFN's packet retransmission efficiency.
- **Actions**:
  1. Enforce GFN's retry budget formula: $\min\left(3, \lfloor 52 / \text{RTT} \rfloor\right)$ with retry intervals at $\text{RTT} + 4\text{ ms}$.
  2. Align the client-side NACK poll timer to 4 ms with a 1 ms initial wait grace period.
  3. Ensure the POP transforms missing packet runs directly into GFN 0x0317 bitmask records on channel 1.

### Step 3: Reference Frame Invalidation (0x0301) and Bitstream Concealment
- **Objective**: Eliminate video freezes and decoder watchdog resets during packet loss.
- **Actions**:
  1. Ensure intermediate packet drops trigger 0x0301 RFI rather than defaulting to 0x0302 IDR requests.
  2. Implement skip-frame bitstream concealment in the client before submitting to VideoToolbox: when an intermediate frame is lost, synthesize a skip frame to preserve the reference buffer and prevent error -12909.
  3. Adjust the decoder recovery gate: hold dependent P-frames only until the recovery P-frame lands, rather than tripping the 20-frame reset.

### Step 4: QoS Reporting and Telemetry Synchronization (0x0207)
- **Objective**: Ensure host NVENC rate control respects client hardware capabilities.
- **Actions**:
  1. Calculate sustainable decoder FPS and renderer FPS in Moonlight's client statistics.
  2. Package these values into `V2_PKT_STATS` and feed them into the POP's 0x0207 periodic QoS report.
  3. Forward server RTT from 0x0101 to the client's onscreen overlay.

### Step 5: Audio Cauchy RS(6, 4) Error Correction
- **Objective**: Eliminate audio pops and crackles during packet bursts.
- **Actions**:
  1. Implement Cauchy Reed-Solomon RS(6, 4) packet decoding on the client audio thread.
  2. Reconstruct lost 5 ms Opus packets in-place across each 30 ms window before submitting to the Opus decoder.
  3. Configure the client audio jitter buffer to dynamically adapt headroom upon detecting packet jitter bursts.
