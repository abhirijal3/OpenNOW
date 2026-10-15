# GeForce NOW macOS client: reverse-engineered reference

This document compiles the reverse-engineered architecture, protocol mechanics, algorithms, and timing rules of NVIDIA's official GeForce NOW macOS client (v2.0.88.129, arm64 `libBifrost2.dylib` and `libGeronimo.dylib`). It details every component responsible for stream smoothness, input responsiveness, and fault tolerance.

Static reverse engineering only; nothing here is checked against a live session or against the binaries from this
repo. For the longer v2.1 write-up and line-cited proof see `~/Documents/Rasbari/protocol/docs/v2/gfn-macos-native-client.md`
and `gfn-ghidra/`. Pick the file for your area:

- `macos-client-network.md`: thread and QoS layout, packet reception and reordering, the NACK engine (command 0x0317).
- `macos-client-video.md`: VideoToolbox decoding, reference concealment, Metal rendering and filters.
- `macos-client-input-audio.md`: client-side mouse handling and the audio pipeline.
- `macos-client-congestion.md`: bandwidth estimation, telemetry, policer detection, DSCP and ECN, heartbeat and liveness.
