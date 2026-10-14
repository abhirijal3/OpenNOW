# Where we left off

## Current checkpoint — 2026-10-14 (Codex, v2.0 repair WIP paused)

Abhi requested Markdown handoff because usage is nearly exhausted. Work paused.
Authoritative shared state: /Users/abhirijal/Documents/GitHub/protocol/docs/v2/results/gfn-v20-recovery-handoff-2026-10-14.md.
Current branch codex/v2.0-recovery-fix remains dirty/uncommitted; original baseline
and v2.1 preserved. Native reference invalidation hook implemented/tests pass,
but live still times out. Loss-only QoS prototype is diagnostic/default-disabled.
Latest tests/results/running cleanup and ordered next steps are in shared handoff.
All live seats/protocol processes stopped, last seat released15:41:07EDT.
No push/default promotion; do not restart work/live sessions until Abhi resumes.

## Previous checkpoint — 2026-10-12 (Codex, exact-seat resume tested live; custom Dota active)

Abhi authorized custom client takeover after protocol validation. Added/tested
--resume-session-id in tools/run_live_session.py: authenticated discovery must find exact
seat, session.claim, fresh polling until resumePending clears; no allocation fallback.
Eight Python tests and py_compile passed (tools/test_run_live_session.py). No Rust/ABI
change/cargo rerun. Launcher deployed to existing OVH checkout and tested live.

Old stale driver/core/POP 308811/308812/308834 killed without session.stop, preserving
Dota seat fbbb5a16-91fe-41cf-84cf-b1a4208b5e79 from the official client. RESUME 12:25:57
UTC, ready 12:26:03, new driver/core/POP 328087/328088/328113. Rebuilt custom laptop
client 86017 shows Dota at 1920x1200 HEVC/60 fps. Source/client stay active and log.
Stop current driver via SIGTERM 328087 when ending custom seat; no stale driver remains.

Official current logs show 4150-byte bitmap cursor/Local cursor:1 at 60 fps. Native
COMMAND_BITMAP_CURSOR currently has normalized=None and POP forwards small system
updates only. Bitmap bridge remains unfinished; initial mimicRemoteCursor:0 shared
by both clients is not a verified explanation. Protocol fixed stale short-tail FEC
padding with failing-before/passing-after regression and ASan/perf. Full OVH suite
passed, client 403/403; local smoke timing/count failures also occur without the fix.
Limited new live sample: 6365 complete frames, 86 rebuilt shards, no missing or reset
over ~106 s. Exact attribution and complete official-client smoothing remain open.

opennow-library HEAD 2bcab378947d7fa7fd38a38bc13031e1e2c62b73 still dirty/uncommitted;
prior work preserved. No new allocation, purchase, access change, agent/watch/push.
Remote current logs: /home/ubuntu/protocol-live-20261011/logs/virginia-takeover-20261012-122555/;
local protocol/v2/out/takeover-ovh-2026-10-12-082455/. Detailed evidence, validation,
limitations and next steps: protocol/docs/v2/results/gfn-cursor-recovery-2026-10-12.md
and current protocol checkpoint. Next: native bitmap forwarding/ownership and precise
recovery comparison; leave intentional custom stream running until Abhi stops it.

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
