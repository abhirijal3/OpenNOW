# macOS client: threads, packet reception and NACK

Part of the macOS client reference (`gfn-macos-client-architecture.md`): reverse-engineered from `libBifrost2.dylib` and `libGeronimo.dylib` of v2.0.88.129.

## Threading and QoS Architecture

GFN avoids thread contention and priority inversion by isolating each stage of the media pipeline onto explicit thread boundaries, assigning Apple Silicon Quality of Service (QoS) classes via `pthread_set_qos_class_self_np`:

- **RtpPacket RCV Thread** (`USER_INTERACTIVE` / 0x21): Dedicated nonblocking UDP receive loop. Handles datagram reception, SRTP decryption, sequence validation, reorder min-heap insertion, and NACK gap tracking. Pinned to macOS Performance cores.
- **Stream Thread** (`DEFAULT` / 0x15): Pulls ordered packets from the receive deque. Executes bandwidth estimation, per-group Cauchy Reed-Solomon FEC decoding, depacketizing into 1 MiB frame buffers, and decode unit handoff.
- **VideoDecode Thread** (`USER_INTERACTIVE` / 0x21): Blocks on a condition variable waiting for complete access units from a bounded queue (cap 15 to 40 frames). Dispatches synchronous VideoToolbox decode calls without sleeping.
- **Render / Presentation Thread** (`USER_INTERACTIVE` via `CVDisplayLink` / `CAMetalDisplayLink`): Runs the Metal compute passes (NIS / Catmull-Rom), YUV-to-RGB color matrix transformation, overlay blending, and swapchain present.
- **QoS Communication Thread** (`DEFAULT` / 0x15): 50 ms timed event loop aggregating telemetry, loss histograms, bandwidth estimation, and capability reports into a single 1071-byte SCTP message.
- **NACK Poll Timer** (`CNvTimer` at 4 ms): High-resolution timer polling `NackTracker` to batch and dispatch 0x0317 retransmission requests.
- **Audio Source & Render Threads** (`USER_INTERACTIVE` / 0x21): Independent socket receive, Reed-Solomon audio FEC, timestamp jitter buffer, and Opus CELT decoding.

## Packet Reception and Reordering

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

## Retransmission (NACK) Engine

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
