# Where we left off

## Current checkpoint — 2026-10-13 (Codex, player timing bridge locally validated)

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

## Previous checkpoint — 2026-10-12 (Antigravity, Client-Side Cursor Confine Level & End-to-End Cursor Pipeline)

1. OpenNOW (`nvst_input.rs`): Updated `remote_cursor_tracking(true)` (`0x030d`, `COMMAND_TRACK_REMOTE_CURSOR_IMAGE`) to send level `2` (`REMOTE_CURSOR_TRACKING_CONFINE`, `0d03010002`) instead of `1`, matching `libGeronimo` (`2 - (useLocalCursorWithConfine == 0)`). Also lowered `COMMAND_BITMAP_CURSOR` (`0x0110`) minimum payload length in `server_cursor_messages` from 8 bytes to 6 bytes so 6-byte `count = 0` cached bitmap cursor switches without a position tail are forwarded instead of dropped. All 34 `nvst_input` tests pass.
2. protocol (`v2/src/popcursor.{h,c}`, `v2/src/v2features.c`, `v2/tools/v2pop.c`, `v2/tests/test_popcursor.c`, `v2/tests/test_features.c`): Added `pop_cursor_queue_t` so bitmap cursor definitions (`count > 0`) arriving before `on_feature_poll` runs are never overwritten by intermediate system/cached-reference cursor events; modified `v2_pop_cursor_bitmap` to immediately queue `CUR_KIND_SHAPE` blobs into `p->msg` when new bitmap shapes are bound; and updated `pop_cursor_queue_push` coalescing to preserve `has_pos`/`x`/`y` (when neither update is hidden) and `has_confine`/`confine` across consecutive cursor updates in the same poll batch. `test_popcursor` passes all 162 checks; `test_features` passes all 3579 checks.
3. moonlight-qt (`streamtypes.h`, `v2transport.{h,cpp}`, `clientcursor.{h,cpp}`, `cursormanager.{h,cpp}`, `sdlinputhandler.h`, `input.cpp`, `session.cpp`, `tests/test_v2transport.cpp`, `tests/test_cursormanager.cpp`): Forwarded GFN system cursor IDs (`v->sys_id` 1..16) and host pointer `scale_pct` (`v->scale_pct`) from `V2Transport::onCursorThunk` into `CursorManager`, mapped system cursors 1..12 to `SDL_CreateSystemCursor` and 13..16 to `SDL_GetDefaultCursor()` (matching `SDLMouseCursorDecoder`), passed `currentScalePct()` into `CursorManager::applyShape`, replaced single-shape `m_LastCursorShape` reset-on-switch with a 32-entry `m_SentCursorShapes` ring so rapid bitmap/system/hidden cursor switches reuse cached shapes without round-trip re-requests, and moved `SDL_WarpMouseInWindow` onto the SDL main thread right after exiting relative mouse mode. `streamclient_tests "[cursormanager],[v2transport],[clientcursor]"` passes 53 cases (2,686 assertions), and both `build-dev` and `build-noproto` compile cleanly.

opennow-library HEAD 2f0dcda still dirty/uncommitted; prior work preserved. No new
allocation, purchase, access change, agent/watch/push. Rebuilt `libopennow_gfn.a` on OVH
(`/home/ubuntu/OpenNOW-live-20261012-latest`) and `v2pop` (`/home/ubuntu/protocol-live-20261012-latest/v2/bin/v2pop`),
then resumed the same Illinois GFN seat `8ace22c3-3bf7-459e-aa9b-89156a14fd02` (`66.22.135.140`, L40-12).
Intentionally running: OVH driver `350266`, `opennow-core` `350267`, `v2pop` `350347`; laptop `streamclient` `59088`, `caffeinate` `59089`.
Live `v2pop.log` verified `id=1000 count=4150` and client connection; laptop `client.log` verified `pointer: drawn by this client` and `pointer: the host shows it, absolute mouse` at 60 fps.

## Historical checkpoint — 2026-10-12 (Antigravity, non-blocking Metal render, SRTP ROC sync, decoder watchdog diagnosis)

Antigravity verified Virginia GFN session via OVH POP (40.160.38.161) to the macOS laptop client.
1. Implemented non-blocking Metal render dispatch in Moonlight client (vt_metal.mm), dropping CPU render time from 4 ms to 0.2–0.3 ms with 0 display drops.
2. Implemented mid-stream SRTP ROC auto-detection in protocol/v2/src/srtp.c, enabling instant mid-stream reconnection without key mismatch errors.
3. Traced hardware decoder behavior during Wi-Fi channel hops: verified FEC rebuilt 532 frames and retransmission recovered 2,457 packets across 54k frames. Diagnosed VideoToolbox -12909 error on lost frame predictions tripping Moonlight's 20-frame watchdog, confirming client must emit 0x0301 RFI and hold intermediate P-frames to prevent panic resets.
4. No background tasks or active processes remain running.

## Historical checkpoint — 2026-10-11 (Antigravity, Virginia HEVC session, cursor and codec coordination verified)

Antigravity verified a live GFN Virginia session via OVH POP (40.160.38.161) to the macOS laptop client.
Settings coordinated with opennow-core: 1920x1200 resolution, HEVC codec, 75 Mbps max bitrate cap.
Fixed cursor coordination: v2pop extracts GFN cursor-update events and notifies v2 client when the host
hides cursor (e.g. Dota 2), preventing double cursors. Added --codec support to v2pop and run_live_session.py
to propagate HEVC to the protocol client. Direct presentation enabled in Moonlight Metal renderer,
achieving 60 fps presentation with 0 dropped frames.

## Historical checkpoint — 2026-10-11 (Antigravity, live India session & client exit verified)

Antigravity resumed live OVH integration per Abhi's request. India booking succeeded on
retry (17:17:11 UTC, session 2b66886e-5793-44f9-bb66-9cb39681c432, L40S rig). Prior 500
INTERNAL_ERROR_STATUS 8A960000 was identified as CloudMatch cluster capacity exhaustion
and VM deprovisioning cooldown in Mumbai during Sunday evening peak hours (~9:40-9:55 PM IST).
Live stream delivered 2,282 frames at 60 FPS (2.66 ms decode time) and 7,600+ audio packets
to the laptop client over 40 seconds. On remote shutdown, GFN seat was cleanly released and
the Mac client automatically caught Disconnected and exited cleanly (PID 73913 reaped).
Zero seats held; no task-owned processes remain running.

## Historical checkpoint — 2026-10-11 (Codex, resumed live OVH integration)

Abhi resumed Gemini's interrupted OVH POP / GFN Virginia / laptop client run. The
previous paused/clean checkpoint is historical. This checkout remains opennow-library,
HEAD 2bcab378947d7fa7fd38a38bc13031e1e2c62b73, dirty and uncommitted. Preserve the
pre-existing core/RTSP/collector edits and shared context wrappers. Codex added the
request-idr command using the active NVST feedback handle (cleared on stop), documented
the actual C contract, and repaired the untracked live driver with unlimited duration,
private key/log handling and seat/core cleanup. Regression tests cover command states
and driver duration/reaping. Automatic C key/SSRC/profile export, other control levers,
and raw encrypted audio remain unfinished; see docs/protocol-integration.md.

Validation: cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace
passed 521 tests; release build passed locally and on OVH. OVH core tests passed 135.
python3 -m unittest discover -s tools -p test_run_live_session.py passed 3, py_compile
passed. Local logs are /Users/abhirijal/Documents/GitHub/protocol/v2/out/live-ovh-2026-10-11/;
remote test/build logs are /home/ubuntu/protocol-live-20261011/logs/. Protocol v2 tests,
perf and OVH sanitizer socket smoke passed; Moonlight passed 401/401 tests. Detailed
commands and evidence: /Users/abhirijal/Documents/GitHub/protocol/docs/v2/results/gfn-ovh-2026-10-11.md.

Current runtime at 16:12 UTC: the India source stopped video at 15:58:18 UTC and
OpenNOW later emitted nvst-recovery-exhausted (8.004 s upstream silence), status stopped.
Abhi saw an idle warning after AFK; client kept the last picture after its 8 s video
timeout. Old POP input events/commands/keepalive pulses were zero; inactive input is
supported but the transport error alone does not prove the provider's termination cause.

Stopped old OVH driver 279355 (seat released 16:09:55 UTC), core 279356, POP 279432;
new India create attempts 16:10:19/16:10:49/16:11:56 each failed HTTP 500
INTERNAL_ERROR_STATUS 8A960000. Authenticated session.remote.list reported sessions: [].
Failed drivers 280391/280531/280789 exited with cores reaped. No source session/POP or
launcher remains running. Laptop PID 58155 was stopped for the client disconnect-exit fix; no client remains running.
Moonlight now closes on Disconnected through normal main-thread cleanup, validated
402/402 dev and 398/398 standalone tests plus whole-process socket/startup checks. Detailed saved
old/failure logs and commands are in the shared protocol report. No OpenNOW source/build change, new source tests, region/account/game change, vault
transfer, purchase, watcher or push in the client-fix turn.

Next: verify provisioning/account state before booking again, keep India and existing
key/port, verify actual live input after successful restart. Address active-idle pulse
policy and missing propagation of engine-stopped state with dedicated regression tests;
do not disguise the source failure by pinning timeouts. All task-owned test processes are stopped. Private logs/keys remain excluded from Git.

## Historical checkpoint — paused 2026-10-10 (Antigravity)

Abhi paused work and requested a unified multi-agent context architecture across
Claude Code, Codex, Gemini, and Antigravity. That structure is now in place in this repo,
mirroring the setup established in `~/Documents/GitHub/protocol`.

Workspace: `/Users/abhirijal/Documents/GitHub/OpenNOW`, branch `opennow-library`, HEAD `2bcab378`
(`Add the protocol integration guide and link it from both repos`). Working tree is clean.

### What this branch is and current implementation state

Turning OpenNOW's GFN client into a headless library (`libopennow_gfn.a`) that our custom
v2 streaming protocol POP embeds as its GFN client role.
The protocol repo is at `~/Documents/GitHub/protocol`.

1. **Desktop UI removal complete (Phases 1-5 from `.plans/opennow-library-remove-ui-plan.md` landed)**:
   - Deleted `opennow-qt/`, `playnite/`, `locales/`, `crowdin.yml`, Qt CI workflows, FFI crate
     `opennow-streamer-ffi`, desktop tools (acceptance-verify, license-report), and legacy contract tests.
   - Net 868 files and ~145,000 lines removed.
2. **Rust test suites baseline**:
   - `native/opennow-core`: 705 unit & integration tests pass (`cargo test --manifest-path native/opennow-core/Cargo.toml`).
   - `native/opennow-streamer`: 809 workspace tests pass (`cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace`).
3. **Header-only video forwarding mode implemented & verified**:
   - Setting `on_video_packet` callback tap puts streamer into header-only mode: video is never decrypted,
     FEC-repaired, reordered, or assembled locally. Raw packets go directly to C.
   - Live measurement on GFN India (Mumbai L40S, 2026-10-08): 1.5% single-core CPU on M1 Mac
     (bundle 0.93%, video 0.43%, events 0.03%), ~1.3 MB live heap. GFN accepted it with all 4,106 frames
     acked at 60 fps and QoS reports at ~19/s.

### Next steps in order (from `docs/protocol-integration.md` and `.plans/gfn-forwarder-plan.md`)

1. **Export video SRTP keys to C**:
   - Expose the video SRTP key, salt, SSRC, and profile in `include/opennow_gfn.h` and `crates/opennow-gfn`.
   - The protocol v2 player needs these to restore GFN's header and decrypt pass-through packets.
2. **C control levers**:
   - Add C API functions for bandwidth estimate field in 0x0207 QoS report (with utilization kept low),
     0x0318 max bitrate, 0x0301 frame range invalidation, and 0x0302 keyframe requests.
3. **Audio pass-through check**:
   - Check if str0m can expose DTLS-SRTP keys for pass-through encrypted audio; otherwise keep handing
     decrypted Opus packets to C.
4. **Wire into protocol v2**:
   - Connect `libopennow_gfn.a` into `protocol/v2/src/server.c` in place of `gfnsim`.
5. **Multi-account & Linux capacity validation**:
   - Test multiple sessions on Ubuntu server; measure CPU and memory footprint under multi-session load.

### Shared agent rules & context conventions

- `AGENTS.md` is the single source of truth for all project rules, reading lists, and constraints.
- `CLAUDE.md` and `GEMINI.md` are thin entry stubs importing `@AGENTS.md`. Codex reads `AGENTS.md` directly.
- **On start**: Every agent reads the current checkpoint at the top of this file first.
- **On pause / handoff**: Every agent updates the top checkpoint in this file before stopping.
- Keep one shared handoff file here. Never create private, competing per-agent status files.
- Integration guide: `docs/protocol-integration.md` (keep updated when API or measured costs change).
- Protocol side contract: `~/Documents/GitHub/protocol/docs/v2/opennow-library.md`.

---

## Historical checkpoints

### Phase 1 UI removal complete (2026-10-08, Claude Code)
- UI and Qt infrastructure removed. 868 files changed, 144,924 lines deleted.
- Both test suites passing. Baseline documented in `.plans/opennow-library-remove-ui-plan.md`.
- `docs/protocol-integration.md` authored and committed (`2bcab378`).
