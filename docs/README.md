# OpenNOW docs: start here

Index of this repo's docs. Read the one line that matches your task, open only that file. Every new or
changed doc gets a line here in the same turn. The protocol side's index is
`~/Documents/Rasbari/protocol/docs/v2/README.md`.

## Current state

- `where-we-left-off.md`: the current checkpoint, what is done and open. Read first when resuming work.
- `history.md`: decisions, real numbers and lessons from retired checkpoints and plans (2026-10-08 to 2026-10-12). Read to learn why something is the way it is.

## The library and the POP

- `protocol-integration.md`: how the protocol's POP builds and links `libopennow_gfn.a`, the session steps, what comes out and goes in, threads. Read for any C ABI or POP-facing change.
- `integration-input-timing-mic.md`: timed input, microphone, player feedback and control, timing and profile calls, raw audio and control taps, key export. Read when touching those C calls.
- `integration-measured.md`: library CPU, memory and stream rates from the first live run (2026-10-08). Read when judging cost per session.
- `integration-open-work.md`: what is still open on the library and POP side and the open decisions. Read before planning the next step.
- `same-seat-handover.md`: the live driver `tools/run_live_session.py` and reclaiming an official-client seat. Read when running or changing a live session.

## Seat booking (opennow-core)

- `core-protocol.md`: the JSON-over-stdin/stdout interface: handshake, envelopes, method list, diagnostics. Read first for anything in `opennow-core`.
- `core-auth.md`: device login, logout, saved accounts, provider routing and account scope. Read when touching sign-in.
- `core-settings.md`: the settings store, language values and the HDR opt-in. Read when touching settings or HDR.
- `core-sessions.md`: seat create, poll, stop, claim, discovery, error codes (including unknown create outcome and capacity), seat setup steps, `streamer.prepare`. Read when touching session booking.
- `session-management-future.md`: planned work for seat handling across customers on company GFN accounts, plus review follow-ups. Read before designing session or account changes.

## GFN wire behavior

- `gfn-wire-catalog.md`: every GFN packet on each leg (media, downlink and uplink control) with its wire layout. Read to look up what a message is. Our current gaps are in protocol's `docs/v2/packet-map.md`.
- `mjolnir-nack-v2.md`: how the Mjolnir video route sends retransmission requests (command 0x0317). Read when touching video loss recovery.
- `native-gamepad-input.md`: how controller snapshots become GFN input packets (0x020d). Read for gamepad work.
- `gfn-macos-client-architecture.md`: entry point for the reverse-engineered NVIDIA macOS client (v2.0.88.129) reference, split by area below. Read first, then open one file.
- `macos-client-network.md`: threads and QoS layout, packet reception and reordering, the NACK engine (0x0317). Read for receive-path and retransmission rules.
- `macos-client-video.md`: VideoToolbox decode, reference concealment, Metal rendering and filters. Read for decode and presentation behavior.
- `macos-client-input-audio.md`: client-side mouse handling and the audio pipeline. Read for input feel or audio buffering.
- `macos-client-congestion.md`: bandwidth estimation, telemetry, policer detection, DSCP and ECN, heartbeat and liveness. Read for QoS reporting and rate behavior.
- `official-client-baseline.md`: what NVIDIA's Windows client logged in one 2026-08-30 session (NACK numbers, recovery ladder, audio buffer, input types). Read for evidence when tuning against the official client.
