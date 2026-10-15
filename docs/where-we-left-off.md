# Where we left off

## Current checkpoint: 2026-10-15 (Claude Code, v2.1 closed)

v2.1 committed and tagged on branch v2.1, after this morning's live run on OVH Illinois proved it (three
sessions, the longest 825 s with 49,502 of 49,508 frames shown and no timeout; logs in the protocol repo at
`v2/out/live-ovh-20261015-illinois/`). The OpenNOW source on the server (`/home/ubuntu/OpenNOW-live-20261015-illinois`)
matched this checkout file for file. Tests before the commit: core 284 + 3 + 1 pass, streamer workspace 542 pass.
The commit includes the earlier uncommitted session-create changes and docs cleanup described below. Next is
v2.2 POP routing, designed in the protocol repo's `docs/v2/design/pop-routing.md`.

## Previous checkpoint: 2026-10-14 (Claude Code, v2.1 review, session edge cases, docs cleanup)

Branch v2.1, HEAD 5b0f301b ("v2.1 WIP", committed, not validated end to end; the checkpoint below that
calls its edits uncommitted at HEAD 2f0dcda is out of date). Reviewed that commit: tests pass, findings and
follow-ups are in `docs/session-management-future.md`. Uncommitted: the GitHub to Rasbari path rename in four
docs (earlier agent), plus this session's `cloudmatch.rs` and `gfn/routing_tests.rs` changes: create always sends
`enablePersistingInGameSettings: false` (fresh seat every time, customers use company accounts), an unreadable
create outcome fails `session_create_outcome_unknown` and blocks further creates until discovery or stop settles
it, and capacity exhaustion `8A960000` is `session_capacity_unavailable`. Contract in `docs/core-sessions.md`.
`cargo test --manifest-path native/opennow-core/Cargo.toml`: 284 + 3 + 1 pass. Streamer workspace untouched
(6 + 137 + 56 + 17 + 325 pass). No live session, no process left running. Docs
cleanup (docs only, uncommitted): deleted the Qt-era streamer comparison, the two finished `.plans`, the
blank-video and session-start notes; compacted old checkpoints into `history.md` and the comparison into
`official-client-baseline.md`; split `core-protocol.md`, `protocol-integration.md` and the macOS client
reference into small files; index is `docs/README.md`; four protocol docs were repointed. Next: the items in
`docs/session-management-future.md`, starting with persisting the unknown-create block.

## Previous checkpoint — 2026-10-13 (Codex, player timing bridge locally validated)

Abhi authorized protocol section-7 implementation with tests/benchmarks and a
Windows static audit. This repo adds cached server-authoritative 0x0112 config,
`opennow_gfn_player_timing` C ABI/startup seed, reliable/partial control forwarding,
and checked player-owned 0x0203-v3 partial-channel routing. No invented DJB
command/version-5 payload or synthetic GPU/display report. Latest server enable/
disable overrides startup seeds and persists across player takeover; new session
feedback clears old config. Header/source/paced metadata stays inside video bytes.
Contracts updated in `docs/protocol-integration.md` and protocol's
`docs/v2/opennow-library.md` / `packet-map.md`; complete report/index in protocol.

Branch v2.1, HEAD 2f0dcda15248c0a7c7ca7c25160440607fe88efc; all edits uncommitted.
Preserve concurrent timed-input/microphone/cursor/media/key-export work. Cursor
truncation fixture now permits the already-supported six-byte cached reference.
`cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace` passes
541 tests; `cargo build --manifest-path native/opennow-streamer/Cargo.toml -p
opennow-gfn --release` passes. Logs in protocol's
`v2/out/gfn-frametiming-20261013/{test-opennow-final,build-opennow-final}.log`.
Protocol C unit/socket tests and OVH ASan/UBSan/leak tests cover bridge routing;
local synthetic/decoded benchmarks are not live negotiation or official parity.
Final player focused tests 2278 / 58, CTest 498/498, and Linux FFmpeg-owner/trace
sanitizer checks 374 / 39 pass; raw logs/report remain in protocol.

No task-owned process/agent/watch/automation remains; no new live GFN session,
service restart, deployment/push/CI, billed resource or access change. Concurrent input completion is recorded below. Abhi chose to run the prepared
Windows runner. Next: physical results, separately live-authorized official A/B,
then decide supported timing-version/backend/default gates from evidence. Native
processing/GFN negotiation and timing still require a real session to validate.

### Concurrent input/microphone bridge — 2026-10-13 (Codex, locally complete)

Six timed input ABI calls retain producer microseconds through Rust/NVST;
negotiated mic availability, enable and opaque Opus ABI use the existing bounded
RTP/SRTP source queue. New player generations re-arm/clear the queue before the
POP acknowledges them. Malformed bitmap notifications require a normalized
cursor before capture handoff. Contracts: docs/protocol-integration.md and
protocol/docs/v2/opennow-library.md. All edits uncommitted at the same v2.1 HEAD.
`cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace`
passes 6 ABI + 137 core + 325 transport; release opennow-gfn build passes. Canonical
shared checkpoint and full tests/limits are protocol/docs/where-we-left-off.md
and protocol/docs/v2/gfn-input-five-implementation-2026-10-13.md; logs in
protocol/v2/out/gfn-input-five-20261013/. No input-owned processes remain; no
new live session, service/access change, deployment/push/CI. Physical/live parity
is unmeasured. Preserve the separate playback checkpoint above.

Older checkpoints (2026-10-08 to 2026-10-12) are compacted into `history.md`.
