# AGENTS.md

## What this branch is

OpenNOW's GFN client, being turned into a library that our streaming protocol embeds as its "GFN
client" role. The desktop app is gone. What's left is Rust:

- `native/opennow-core/` signs in to NVIDIA, refreshes tokens, stores credentials and runs the
  CloudMatch session lifecycle (create, poll, stop, claim), then builds the session context the
  streamer consumes. It runs as a child process speaking newline-delimited JSON
  (`docs/core-protocol.md`) until the library's C ABI replaces that.
- `native/opennow-streamer/` runs the stream: RTSPS setup, the Mjolnir/ICE/DTLS/SCTP transport,
  SRTP, depacketizing, input, QoS and recovery. It decodes nothing and draws nothing: whole video
  access units (tagged with GFN's frame number), Opus packets and cursor events go to the embedder,
  and input comes back in.

Only GFN protocol code belongs here. No decoding, rendering, audio playback, capture or UI.

The only target is Ubuntu Linux on our servers. macOS and Windows code paths are dead weight and
go as the code they live in is reworked; don't add new ones.

What the library has to do, and the rest of the GFN story, lives in the protocol repo. Start at
`~/Documents/GitHub/protocol/docs/v2/README.md` (see `CLAUDE.local.md` if you are Claude).

## Priorities

Performance and reliability first. Behavior stays predictable under load and through failures:
session restarts, reconnects, partial streams, lost packets. Pick correctness over convenience.

## Streamer rules

- Sender-authored packet, frame, timestamp and stream identifiers are protocol data. Keep them end
  to end through transport, queues, feedback and diagnostics; never swap in local counters. GFN's
  frame number is what the protocol side keys on.
- Receive, depacketize, assemble and input each sit on an explicit thread boundary. Nothing blocks
  the receive thread.
- Queues are bounded and observable. Define what happens on overflow, discontinuity, missing
  references and late frames. Recovery asks for a valid reference frame (0x0301 or 0x0302) and never
  passes on corrupt data silently.
- Feedback to GFN (frame acks, loss reports, the 0x0207 QoS report, keyframe requests, max bitrate)
  uses negotiated or sender-provided values and the documented wire formats. Check changes against
  captured diagnostics and focused protocol tests, not timing guesses.
- Diagnostics keep the receive, assembly, hand-off and feedback stages distinguishable.

## Boundaries

- Values crossing the core's JSON protocol or a C ABI are serializable, bounded and explicitly
  typed. When one changes, update every producer, consumer, fixture and version check together.
- Keep provider/alliance behavior, stable device IDs, persisted account and session compatibility,
  and recovery semantics intact when refactoring.
- No platform handles or graphics objects in anything the library hands out.

## Maintainability

- Before adding code, look for an owner that already does it. Duplicate logic is a smell.
- Deleting is welcome. Refactors keep behavior the same unless a change is asked for.
- Small typed helpers over broad utility modules.
- Release pressed keys, buttons and controller state on every shutdown and failure path.

## Build and check

- Core: `cargo test --manifest-path native/opennow-core/Cargo.toml`
- Streamer: run the affected crate's tests first, then
  `cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace`.
- Don't call something done while a relevant test fails. Report the command and the failure.
- A live session needs a real NVIDIA/GFN account. Until the library has its own driver, nothing on
  this branch runs one; `main` still has the full app for side-by-side checks.
