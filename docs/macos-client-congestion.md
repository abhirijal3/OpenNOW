# macOS client: congestion control, telemetry and liveness

Part of the macOS client reference (`gfn-macos-client-architecture.md`): reverse-engineered from `libBifrost2.dylib` and `libGeronimo.dylib` of v2.0.88.129.

## Congestion Control, Telemetry, and Traffic Shaping

- **Periodic Feedback (Command 0x0207)**: Dispatched every 50 ms in a 1071-byte SCTP bundle carrying:
  - Estimated bottleneck bandwidth (BWE).
  - Round-trip delay (RTT) and one-way delay (OWD).
  - Packet loss ratio and jitter.
  - Client decoder and renderer sustainable framerate caps.
- **Policer Detection (Command 0x0315 / 0x0316)**: The client builds a histogram of packet loss correlated with frame sizes. When network token-bucket shapers drop packets on large frame bursts, the client identifies the threshold and issues command 0x0316, instructing the server to reduce MTU or slice frames into smaller UDP datagrams.
- **Reference Invalidation (Command 0x0301)**: When frame loss occurs, the client invalidates the lost frame range. NVENC references a prior acknowledged reference frame, producing a standard-size P-frame and avoiding IDR bandwidth spikes.
- **DSCP / ECN**: Supports DSCP traffic marking (CS1, CS5, CS7) and ECN ECT(1) codepoints.
- **Heartbeat & Liveness**: 3-second heartbeat ping over the control channel. If upstream media packets cease for 8 seconds, the client transitions to a recovery/reconnecting state instead of hanging the presentation window.
