# macOS client: mouse and audio

Part of the macOS client reference (`gfn-macos-client-architecture.md`): reverse-engineered from `libBifrost2.dylib` and `libGeronimo.dylib` of v2.0.88.129.

## Client-Side Mouse Architecture

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

## Audio Architecture

- **Codec**: Opus running in CELT-only mode (`mode 1002`, `RESTRICTED_LOWDELAY`) at 48 kHz stereo. In-band FEC and DTX are disabled.
- **Packet Duration**: 5 ms packet duration (200 packets per second).
- **Packet-Level FEC**: Cauchy Reed-Solomon RS(6, 4) forward error correction (4 data shards + 2 parity shards = 50% redundancy). Any 2 lost packets out of 6 (a 30 ms window) are reconstructed immediately without retransmission.
- **Jitter Buffer**: Client maintains `TimestampAudioBuffer` targeting 60 to 80 ms latency. Employs burst-detection logic that expands buffer headroom during jitter spikes and smoothly shrinks during stability, falling back to Opus Packet Loss Concealment (PLC) during severe underruns.
- **Transport**: Audio runs over an independent UDP socket and RTP sink (default port 48000), isolated from the video pacer.
