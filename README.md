# OpenNOW (GFN client library work)

This branch turns OpenNOW's GeForce NOW client into a library: it holds an ordinary GFN session and
hands out GFN's video access units, Opus audio and cursor messages, takes input in, and exposes GFN's
rate and recovery levers. It has no UI and targets Ubuntu Linux servers only. The desktop app lives on upstream `main`.

- `native/opennow-core/`: login, CloudMatch session booking, the session context.
- `native/opennow-streamer/`: RTSPS setup, the NVST transport, depacketizing, input, QoS and recovery.

```
cargo test --manifest-path native/opennow-core/Cargo.toml
cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace
```

OpenNOW is not affiliated with, endorsed by, or sponsored by NVIDIA. NVIDIA and GeForce NOW are
trademarks of NVIDIA Corporation.

MIT License, see [LICENSE](LICENSE).
