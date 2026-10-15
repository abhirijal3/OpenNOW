# GFN wire catalog (Mjolnir / NVST)

The packets on each leg of a GFN session: media on UDP (RTP/SRTP), state, cursor and control on an ordered or
partially reliable SCTP data channel. The POP relays them in place with a 9-byte header rewrite. This file lists
only the GFN wire side. Which packets our POP and client forward, and the current gaps, are kept up to date in
`~/Documents/Rasbari/protocol/docs/v2/packet-map.md`; byte-level proof is in `gfn-ghidra/` there.

---

## 1. Downlink: Rig to Client

On the downlink, GFN separates media from control. Media datagrams arrive over UDP via RTP and SRTP, while state, cursor, and control updates travel over an ordered SCTP data channel. Our POP acts as a cut-through relay: it performs an in-place 9-byte header rewrite on media datagrams to route over our single UDP socket, while intercepting SCTP control packets and translating them into our unified `V2_PKT_MSG` message layer.

### 1.1 Media Stream Datagrams

| GFN Packet / Channel | Wire Layout & Semantics |
| --- | --- |
| **Video RTP (PT 101)**<br>UDP media port | 12-byte RTP header + 4-byte extension tag (`0x4753` "GS") + 16-byte NVIDIA header + AES-GCM encrypted Annex-B bitstream. Typical MTU 1408 B. |
| **Video Parity Shards (PT 101)**<br>UDP media port | Cauchy Reed-Solomon parity packets over GF(2^8) appended to each FEC group (2 to 30 shards per frame). |
| **Video NACK Resend (PT 106)**<br>UDP media port | Retransmitted video shard with RTP payload type rewritten to 106 and RTP timestamp updated to resend send time. |
| **Audio RTP (PT 111 / RED 63)**<br>UDP audio port (48000) | 5 ms Opus CELT packets (`mode 1002`, 48 kHz stereo, 200 pkt/s), AES-256-GCM encrypted, protected by RS(6, 4) parity. |

### 1.2 Downlink Control & Notice Commands (SCTP → V2_PKT_MSG)

| GFN Command | Channel & Reliability | Payload & Purpose |
| --- | --- | --- |
| **0x0101**<br>QoS Info | SCTP 0<br>Reliable | 80 bytes: Server-measured RTT, stream ID, and quality breakdown (downlink, uplink, GPU, WiFi score). |
| **0x0102**<br>Command Outcome | SCTP 0<br>Reliable | 4 bytes: `u16 command`, `u16 outcome` (0 = OK, 1 = Error). |
| **0x0103 / 0x0104 / 0x0105**<br>Termination Timers | SCTP 0<br>Reliable | 8 bytes: `u32 timer_type` (1 to 8), `u32 seconds_remaining`. Warns of pending idle/AFK shutdown. |
| **0x0109**<br>Session Terminate | SCTP 0<br>Reliable | Big-endian result code + text string. Cloud instance is deprovisioning or terminating. |
| **0x010b**<br>Rumble / Haptics | SCTP 0<br>Reliable | 4-byte header + records of `u16 pad`, `u16 low_motor`, `u16 high_motor`, `u16 duration`. |
| **0x010e**<br>HDR Mode | SCTP 0<br>Reliable | 8 bytes: `u32 mode` (0 SDR, 1/2 HDR10 PQ), `u32 stream`. |
| **0x010f**<br>System Cursor | SCTP 0<br>Reliable | `u32 id` (0 hides and locks), optional `u16 x, y` (0..65535 normalized), optional confine flag. |
| **0x0110**<br>Bitmap Cursor | SCTP 0<br>Reliable | `u32 id`, `u16 len` (0 = cache hit), `u16 fmt`, `u16 hot_x, hot_y`, 32-bit BMP data, normalized position, scale. |
| **0x0408**<br>Audio Layout | SCTP 0<br>Reliable | 12 bytes: Channels, coupled streams, and 8-byte speaker channel mapping. |
| **Media Keys**<br>*(Internal Setup)* | RTSP / OpenNOW C ABI | 32-byte video AES key + 12-byte salt, 32-byte audio key + 12-byte salt, SSRCs, codec, FPS. |

---

## 2. Uplink: Client to Rig

On the uplink, our client transmits inputs, acknowledgments, and recovery triggers over UDP to the POP. The POP unwraps these packets and calls OpenNOW / GFN driver functions, which serialize them onto GFN's SCTP channels.

| GFN Wire Command | GFN Channel & Wire Format |
| --- | --- |
| **0x0200**<br>Keepalive | SCTP 0<br>Reliable | `u16 0x0200`, `u16 len 4`, `u32 stream`. Dispatched every 3.0 seconds. |
| **0x0206 / 0x020d**<br>Remote Input | SCTP 0 (keys/mouse)<br>SCTP 10 (gamepad) | Key/mouse events wrapped in 0x0e timestamp envelope (52 B per key). Gamepads use 0x020d with 9-byte timestamp header. |
| **0x0207**<br>QoS Feedback | SCTP 6<br>Partially reliable | 52 bytes sent every 50 ms: BWE (kbps), round-trip delay, jitter, loss fraction, decoder FPS, renderer FPS. |
| **0x0202**<br>Audio QoS | SCTP 6<br>Partially reliable | 124 bytes sent every 70 ms: Audio packets received, packets lost, jitter buffer delay, underruns. |
| **0x020c**<br>Burst Loss Report | SCTP 6<br>Partially reliable | Dispatched per lossy frame: `lostBytes = lost_packets * packet_size`. |
| **0x020b**<br>NACK Feasibility | SCTP 0<br>Reliable | Enables or disables retransmission: active when RTT < 48 ms, disabled when RTT ≥ 52 ms. |
| **0x0301**<br>Reference Invalidation | SCTP 0<br>Reliable | `u16 stream`, `u32 first_frame`, `u32 last_frame`. Commands NVENC to encode P-frame off older reference. |
| **0x0302**<br>IDR Keyframe Request | SCTP 0<br>Reliable | Demands immediate full IDR keyframe from NVENC. |
| **0x0312**<br>Recovery Mode | SCTP 0<br>Reliable | Switches stream to intra-refresh slice waves. |
| **0x0315 / 0x0316**<br>Policer Adaptation | SCTP 6<br>Partially reliable | 0x0315: Loss histogram by frame size. 0x0316: Requests server reduce MTU / packet size. |
| **0x0317**<br>RTP NACK Request | SCTP 6<br>Partially reliable | 10-byte records with 64-bit masks: `u16 base_seq`, `u64 mask` (covering 65 sequences per record). |
| **0x0318**<br>Max Bitrate Ceiling | SCTP 0<br>Reliable | `u32 stream`, `u32 max_kbps` (1,000 to 100,000 kbps). Caps host NVENC bitrate ceiling. |
| **0x0320 / 0x0321**<br>Window / System State | SCTP 0<br>Reliable | Reports window focus, active state, and frame number. Unlocks host cursor updates. |
| **0x0300 / 0x030b**<br>Disconnect | SCTP 0<br>Reliable | Graceful session termination. |

---
