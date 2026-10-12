# GeForce NOW macOS Client Architecture: Deep Reverse-Engineering Reference

This document compiles the reverse-engineered architecture, protocol mechanics, algorithms, and timing rules of NVIDIA's official GeForce NOW macOS client (v2.0.88.129, arm64 `libBifrost2.dylib` and `libGeronimo.dylib`). It details every component responsible for stream smoothness, input responsiveness, and fault tolerance.

---

## 1. Threading and QoS Architecture

GFN avoids thread contention and priority inversion by isolating each stage of the media pipeline onto explicit thread boundaries, assigning Apple Silicon Quality of Service (QoS) classes via `pthread_set_qos_class_self_np`:

- **RtpPacket RCV Thread** (`USER_INTERACTIVE` / 0x21): Dedicated nonblocking UDP receive loop. Handles datagram reception, SRTP decryption, sequence validation, reorder min-heap insertion, and NACK gap tracking. Pinned to macOS Performance cores.
- **Stream Thread** (`DEFAULT` / 0x15): Pulls ordered packets from the receive deque. Executes bandwidth estimation, per-group Cauchy Reed-Solomon FEC decoding, depacketizing into 1 MiB frame buffers, and decode unit handoff.
- **VideoDecode Thread** (`USER_INTERACTIVE` / 0x21): Blocks on a condition variable waiting for complete access units from a bounded queue (cap 15 to 40 frames). Dispatches synchronous VideoToolbox decode calls without sleeping.
- **Render / Presentation Thread** (`USER_INTERACTIVE` via `CVDisplayLink` / `CAMetalDisplayLink`): Runs the Metal compute passes (NIS / Catmull-Rom), YUV-to-RGB color matrix transformation, overlay blending, and swapchain present.
- **QoS Communication Thread** (`DEFAULT` / 0x15): 50 ms timed event loop aggregating telemetry, loss histograms, bandwidth estimation, and capability reports into a single 1071-byte SCTP message.
- **NACK Poll Timer** (`CNvTimer` at 4 ms): High-resolution timer polling `NackTracker` to batch and dispatch 0x0317 retransmission requests.
- **Audio Source & Render Threads** (`USER_INTERACTIVE` / 0x21): Independent socket receive, Reed-Solomon audio FEC, timestamp jitter buffer, and Opus CELT decoding.

---

## 2. Packet Reception and Reordering

### Socket Configuration
- Nonblocking UDP socket (`O_NONBLOCK`) with 1 MiB socket receive buffer (`SO_RCVBUF = 0x100000`).
- Receives one datagram per syscall via `recvfrom` (or `recvmsg` with control buffer when parsing `IP_RECVTOS`). Does not use `recvmmsg`.
- Socket poll timeout is clamped to `SO_RCVTIMEO` (8 ms), serving as the idle tick that enforces queue wait deadlines when incoming traffic stalls.

### Reorder Min-Heap and the FEC Group Shortcut
Incoming datagrams are classified by sequence number and RTP timestamp:
- **In-Order Arrival**: Passed immediately to the ready deque (`addPacketToQueue`).
- **Out-of-Order Arrival**: Placed in a min-heap ordered by RTP timestamp and sequence number (or sequence number alone for retransmissions).
- **Hold Deadlines**:
  - Without NACK: Gaps hold subsequent packets for at most 8 ms (`rtpQueueMaxDurationMs`).
  - With NACK enabled: Gaps hold subsequent packets for at most 52 ms (`dejitterBufferLengthMs`, expanded to 68 ms in dynamic streaming mode 2).
  - Retransmission frame wait: Retransmitted packets are held for at most 68 ms (or 84 ms in mode 2) from the arrival of their frame's first packet.
- **The FEC Group Shortcut**: The receive thread maintains `RtpPacketCounter`, a 4x64 table tracking received sequence masks per FEC group. The instant the count of distinct packets received for an FEC group equals the group's source packet count, `assemblable()` evaluates to true. The queue immediately flushes the group to the ready deque, releasing the hole without waiting out the 52 ms timeout. The missing packet is subsequently reconstructed by Reed-Solomon on the Stream Thread.

---

## 3. Retransmission (NACK) Engine

### Detection and Timing
Retransmission operates across three decoupled components: the client's `NackTracker`, the client's reorder queue, and the server's 2048-slot packet history ring.
1. **Initial Grace Period**: A gap is recorded with `created = now`. The tracker enforces an initial wait of 1.0 ms (`rtpNackInitialWaitTimeMs`) before dispatching a NACK, preventing spurious requests on slight packet reordering.
2. **Polling Frequency**: A 4 ms timer invokes `GetNackBatch()`.
3. **Deadline-Aware Retry Budget**: The maximum retry count is dynamically calculated per packet as:
   $$\text{maxRetries} = \min\left(3, \left\lfloor \frac{52\text{ ms}}{\text{RTT}} \right\rfloor\right)$$
   Retries are spaced by $\text{RTT} + 4.0\text{ ms}$ with no exponential backoff. If an RTT is 20 ms, at most 2 attempts are permitted. If RTT is 40 ms, only 1 attempt is permitted.
4. **Feasibility Toggle (Command 0x020b)**:
   NACK is enabled only when $\text{RTT} < 48\text{ ms}$ (52 ms minus 4 ms hysteresis) and switches off when $\text{RTT} \ge 52\text{ ms}$.

### Wire Format (Command 0x0317)
Dispatched immediately over partially reliable SCTP channel 1 (up to 2 SCTP retransmissions):
- `+0`: Version (u8, value 2).
- `+1`: Stream index (u8).
- `+2`: Record count $N$ (u8, 1 to 64).
- `+3 + 10i`: Base sequence number (u16 LE).
- `+5 + 10i`: Missing packet bitmask (u64 LE). Bit $k-1$ set indicates sequence $\text{base} + k$ is missing ($k \in [1, 64]$).
Each 10-byte record covers up to 65 consecutive missing sequence numbers.

### Server Retransmit Path
The server receives 0x0317, looks up the sequence numbers in its 2048-slot circular history ring, rewrites the RTP payload type to 106 (`PT 106`), updates the timestamp to current send time, and sends the packets immediately over UDP, bypassing the normal video pacer.

---

## 4. VideoToolbox Decoding and Reference Concealment

### Decoder Setup
- Implemented in `VTDecoder` wrapping `VTDecompressionSession`.
- Attributes: Hardware acceleration enabled, empty `kCVPixelBufferIOSurfacePropertiesKey`, `kCVPixelBufferMetalCompatibilityKey = YES`, and pixel format `420v` (or `x420` for 10-bit HDR).
- Does not use `kVTDecompressionPropertyKey_RealTime`.
- Execution is strictly **synchronous**: `VTDecompressionSessionDecodeFrame` is invoked with flags 0. The output callback runs inline on the decode thread, storing the resulting `CVImageBufferRef` directly into an output slot on the decoder object with zero intermediate copies.

### Seamless Resolution Switching
When the server adjusts stream resolution, GFN does not destroy or recreate the `VTDecompressionSession`. Instead, it configures a format description with a clean aperture rectangle. When resolution changes, the decoder session continues running, and the client updates the clean aperture dimensions (`ImageBufferSetCleanRect`). The Metal renderer reads the clean aperture and updates texture sampling coordinates accordingly.

### Bitstream Concealment and Recovery Gate
- If a frame or slice is missing, `NvBitstreamConcealer` synthesizes dummy skip-style frames into the bitstream, ensuring VideoToolbox's reference frame buffer remains continuous and preventing hardware decode error -12909.
- If VideoToolbox returns a decode error, GFN does not feed subsequent dependent P-frames:
  1. Increments consecutive failure counter (10 consecutive failures terminates session).
  2. Issues Reference Frame Invalidation (RFI, 0x0301) or IDR request (0x0302) to the server.
  3. Flushes the compressed queue back to the last valid keyframe.
  4. Activates a recovering gate that discards all incoming non-recovery frames until a valid recovery frame arrives.

---

## 5. Client-Side Mouse Architecture

GFN never renders the mouse pointer into the video stream. The pointer is drawn directly by macOS as a hardware `NSCursor`.

### Server-to-Client Messages
- **System Cursor (Command 0x010f)**:
  Carries a 32-bit ID (0 to 12). `SDLMouseCursorDecoder` maps this ID to native macOS cursors: Arrow, I-Beam, Wait, Crosshair, Hand, and Resize cursors. ID 0 signifies the host has hidden the cursor.
- **Bitmap Cursor (Command 0x0110)**:
  Carries an image ID, 32-bit BMP payload, hotspot offsets ($x, y$), normalized position ($0..65535$), and server DPI scale.
  - The client parses the BMP in `CreateColorCursor`, converts pixels to `NSBitmapImageRep` in `ABGR8888`, premultiplies alpha for AppKit, and constructs an `NSCursor`.
  - Sizing rule: $k = \max(1, \lfloor \text{scalePct} / 100 \rfloor)$. The `NSImage` is assigned size $(w/k, h/k)$ in points. On Retina displays, macOS automatically renders a 32-point image using 64 physical pixels. Sizing is governed by client/host DPI, completely independent of window scaling.
  - Caching: Bitmaps are cached by ID in an `unordered_map<u32, CursorInfo>` for the entire session. Subsequent switches to a cached shape send a 6-byte message with length 0.

### Mode Switching and Warp-on-Unhide
- When the host hides the cursor (ID 0), the client locks the local pointer into relative mode (`SDLWindow::lockCursor`).
- When the host unhides the cursor, the message includes normalized coordinates ($x, y$). The client computes the window position, executes a local cursor warp (`pendingWarp`), and only then unlocks and displays the OS cursor, eliminating cursor jump and desync.
- If a game warps its cursor internally without user motion (detected when cursor moves $>150\text{ ms}$ after the last client input, 3 times, 30 ms apart), the server detects the untracked movement and synchronizes the coordinates to the client.

### Client Input Capture and Server Replay
- **Client Capture**: Mouse motion and gamepad sticks are batched on a 4 ms timer (max 250 packets/s). Sub-movements preserve individual microsecond steady-clock timestamps (up to 20 sub-moves per packet).
- **Immediate Flush**: Mouse button clicks, wheel events, and keyboard scancodes bypass the 4 ms batching timer and flush immediately in strict FIFO order.
- **Server-Side Replay (`rxcore.dll`)**: The host injects mouse movements through a smoothing replay thread. It computes two 128-sample moving averages of client arrival intervals to match the client's native sampling rate. Movements are replayed using a 1 ms multimedia timer (`timeBeginPeriod(1)`) and microsecond spin loops. When network jitter creates an input queue backlog, it smoothly scales replay spacing from 1,000 $\mu$s down to 500 $\mu$s rather than dumping all events at once.
- **Window Focus Hygiene**: On window blur (`SDL_WINDOWEVENT_FOCUS_LOST`), the client immediately synthesizes and transmits key-up, mouse-up, and gamepad-release packets for all held controls.

---

## 6. Metal Rendering and Post-Processing Filters

### Layer Configuration
- SDL Metal view rendering to physical backing pixels (`drawableSize = SDL_GL_GetDrawableSize`).
- Uses a 2-drawable swapchain on VRR / ProMotion displays (`maximumDrawableCount = 2`) gated by a matching dispatch semaphore to prevent queue buildup.
- Layer colorspace is left unmanaged (`nil`) for SDR, routing BT.709 color straight to display native gamut. HDR streams set `BGR10A2Unorm` with Apple's `CAEDRMetadata`.

### Filter Hierarchy
- **1:1 Scale Bypass**: When source dimensions match drawable dimensions, all sharpening and scaling passes are bypassed. A single fragment shader executes YUV-to-RGB color matrix transformation straight into the drawable.
- **Upscaling 1.0x to 2.0x**: Executes NVIDIA Image Scaling 1.0.x (`CS_NvScaler`) compute kernel at sharpness 0.27, 0.58, or 0.82.
- **Upscaling > 2.0x**: Executes native-resolution NIS sharpening (`CS_NvSharpen`), followed by a 9-tap Catmull-Rom bicubic upscale pass.
- **Enhanced (Sharpness Off)**: Executes 9-tap Catmull-Rom bicubic filtering directly.

### Automatic GPU-Time Benchmark Gate
The client benchmarks rendering cost over 100 frames by measuring the 95th percentile GPU execution time (`GPUEndTime - GPUStartTime`):
$$\text{Threshold} = 0.8 \times \left(\frac{1,000,000}{\text{MaxFPS}}\right) \mu\text{s}$$
At 120 FPS, the budget is 6,667 $\mu$s. If p95 GPU time exceeds this threshold, the renderer automatically ratchets down the filter fallback ladder:
$$\text{NIS Scaler} \longrightarrow \text{Catmull-Rom} \longrightarrow \text{Shepherd6} \longrightarrow \text{Shepherd3} \longrightarrow \text{Hardware Bilinear}$$

### Presentation Timing
Submitted command buffers present using:
- `presentDrawable:atTime:` when targeting VRR / ProMotion displays.
- `presentDrawable:afterMinimumDuration:` on fixed-refresh displays to ensure cinematic pacing without compositing queue bloat.

---

## 7. Audio Architecture

- **Codec**: Opus running in CELT-only mode (`mode 1002`, `RESTRICTED_LOWDELAY`) at 48 kHz stereo. In-band FEC and DTX are disabled.
- **Packet Duration**: 5 ms packet duration (200 packets per second).
- **Packet-Level FEC**: Cauchy Reed-Solomon RS(6, 4) forward error correction (4 data shards + 2 parity shards = 50% redundancy). Any 2 lost packets out of 6 (a 30 ms window) are reconstructed immediately without retransmission.
- **Jitter Buffer**: Client maintains `TimestampAudioBuffer` targeting 60 to 80 ms latency. Employs burst-detection logic that expands buffer headroom during jitter spikes and smoothly shrinks during stability, falling back to Opus Packet Loss Concealment (PLC) during severe underruns.
- **Transport**: Audio runs over an independent UDP socket and RTP sink (default port 48000), isolated from the video pacer.

---

## 8. Congestion Control, Telemetry, and Traffic Shaping

- **Periodic Feedback (Command 0x0207)**: Dispatched every 50 ms in a 1071-byte SCTP bundle carrying:
  - Estimated bottleneck bandwidth (BWE).
  - Round-trip delay (RTT) and one-way delay (OWD).
  - Packet loss ratio and jitter.
  - Client decoder and renderer sustainable framerate caps.
- **Policer Detection (Command 0x0315 / 0x0316)**: The client builds a histogram of packet loss correlated with frame sizes. When network token-bucket shapers drop packets on large frame bursts, the client identifies the threshold and issues command 0x0316, instructing the server to reduce MTU or slice frames into smaller UDP datagrams.
- **Reference Invalidation (Command 0x0301)**: When frame loss occurs, the client invalidates the lost frame range. NVENC references a prior acknowledged reference frame, producing a standard-size P-frame and avoiding IDR bandwidth spikes.
- **DSCP / ECN**: Supports DSCP traffic marking (CS1, CS5, CS7) and ECN ECT(1) codepoints.
- **Heartbeat & Liveness**: 3-second heartbeat ping over the control channel. If upstream media packets cease for 8 seconds, the client transitions to a recovery/reconnecting state instead of hanging the presentation window.
