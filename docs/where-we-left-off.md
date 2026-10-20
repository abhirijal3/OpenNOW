# Where we left off

October 20, 2026 (Codex, integration push): Abhi requested verifying/pushing the combined Rasbari work and deploying Cloudflare staging. OpenNOW code 7001071f was already on origin/v2.1; this commit retains the D2/integration documentation follow-up. Streamer workspace tests and release opennow-gfn build pass (/tmp/rasbari-integration-opennow.log). No core, production, POP install or live session change. Cross-repo audit: sibling rasbari-platform/docs/integration-staging-2026-10-20.md. Preserve newer edits from other chats.

October 20, 2026 (Codex, D1/D2 DONE; Abhi's compile/commit/push handoff): Abhi explicitly instructed: "add to shared checkpoints that you are done and the agent should compile/push your work as well." The integration/staging agent should include this chat's remaining D2 follow-up in its current compile, commit and push pass across protocol, rasbari-platform, moonlight-qt and OpenNOW. Preserve the other chats' newer changes and compile the current combined working trees, including untracked D2 files; do not rebuild only an older committed snapshot. Include the repair-envelope headroom calculation, corrected integration/native fixtures, `v2/tests/test_packetsize_repair.c`, refreshed player generated contracts/goldens and D2 docs. Rebuild matching OpenNOW and v2pop together; the required negotiated-size getter is already in OpenNOW 7001071f. Check workflows before pushes under the standing CI rules. Handle any rollout/live test under that agent's existing user authorization.

D1/D2 implementation and local validation are finished; no additional coding task is waiting on this chat. Detailed implementation, commands and logs: protocol `docs/v2/mtu-packet-sizing.md`, and the completed D2 checkpoint below. Platform root tests/typecheck 20/20; Rust 548; native all tests plus 63,602 smoke checks; packet-size 525 and repair-envelope 20 checks; full native perf passes (new guard 0.002 us versus 0.1 us); player 41 cases/349 assertions. Full controller has 661 passes, three pre-existing unrelated end-code/tag failures and one skip. No D2 task-owned process remains. Before this handoff, this chat made no commit, push, deploy or live GFN run; some initial D2 code was committed by other chats, while the listed follow-up and docs remain local. This handoff now explicitly authorizes the integration agent to compile, commit and push that remaining work as well.

October 20, 2026 (Codex, D2 implemented and locally validated on top of the shared changes): Abhi approved all parts of D2 on October 19 and explicitly required layering on the other chats' work on October 20. Implemented measured MTU persistence through ping refreshes and saved session legs, an appended optional POP-start limit on normal starts/seat claims, pre-booking size/capability refusal, prepared-context and CLI propagation, tag-aware ANNOUNCE, actual negotiated-size C ABI/key-message publication, and oversized-packet guards. Layered on the newer direct-repair implementation: reserve worst-case repair-row metadata, player encryption and trunk overhead, with encrypted boundary regressions. No live resizing or recalibration-policy change. Full flow, arithmetic, exact test commands/logs and restrictions: sibling `protocol/docs/v2/mtu-packet-sizing.md` (inside protocol, `docs/v2/mtu-packet-sizing.md`).

Validation: root platform `pnpm test` passes 20/20 tasks plus trace, and `pnpm typecheck` passes 20/20; final focused orchestrator units/start/claim tests pass 27, contracts pass 2766 with floors unchanged, play-rules D2 refresh regression passes. OpenNOW streamer workspace passes 548 and release opennow-gfn build passes. `make -C protocol/v2 -j6 all test build/rel/test_perf_packetsize` passes every native unit and 63,602 smoke checks; encrypted packet/key-message size 525 checks, worst-case repair envelopes 20. `make -C protocol/v2 -j6 perf` passes: new guard 0.002 us against 0.1 us. Player build and contracts/transport/calibration selection pass 41 cases/349 assertions. D2 controller tests pass 5 and typecheck passes. Full controller: 665 tests, 661 pass, 3 existing failures (two end-reason table mismatches plus unchanged test_gfnsession.c rule-tag format), 1 skip; no D2 failure. The earlier session-health timing failure passes both alone and in the final full run. Logs and full commands are in the linked report, including `/tmp/rasbari-d2-{native-validation,native-perf,platform-all-tests,controller-all-tests,player-tests}.log`.

State verified: protocol v2.2 68f27d0, platform main 14bb2b2, OpenNOW v2.1 7001071f, player v2.2 5bc288a4. Other chats committed the initial D2/native/contract/getter work during this task; this chat did not commit or push. Current local D2 follow-up remains uncommitted: repair-headroom calculation and test fixes, controller child metadata fixture, native size fixtures/repair-envelope test, refreshed player generated contracts/goldens and documentation. No reset/revert/checkout or unrelated source edit; all newer routing/repair work preserved. No task-owned process remains. No D2 deployment, install or live GFN seat by this chat. Final app inventory remains only /Applications/Rasbari.app and /Applications/Rasbari Staging.app. No Windows/minimum-OS runtime proof or live NVIDIA acceptance of smaller ANNOUNCE sizes is claimed. Raw encrypted relay explicitly requires the existing GCM8 profile; ordinary OpenNOW receivers retain other profiles. Next: review local D2 follow-up with the current shared branches; leave the unrelated controller end-code/tag failures to their owning work; deploy or run a live GFN proof only when requested.

## Current checkpoint: 2026-10-19 (Codex, clipboard-text adapter)

On v2.1 HEAD 50257d7f, local/uncommitted: native Unicode text and readiness C ABI,
atomic eight-modifier-release-before-text queueing, validation/busy/capacity and
session-cancellation regressions (PLAYER-13). Protocol's single-POP production
callback now invokes this ABI. Streamer workspace 545 tests pass; release library
build passes. Logs `/tmp/rasbari-clipboard-opennow-{workspace,build}.log`.
Details/limits `protocol-integration.md`; no core changes, live GFN session,
deployment, push or commit. Protocol unit executables pass; full suite found the already-documented trunk
metadata smoke timing flake (63465 checks), and one existing-binary rerun passes
63428 checks; logs `/tmp/rasbari-clipboard-protocol-{all-test,smoke-rerun}.log`. Existing branches/dirty work preserved. Remaining: private clipboard
custom-message API, reliable multi-POP paste forwarding, live host acceptance,
and ordinary Windows Ctrl+V client interception. No OpenNOW background process
remains. Final player focused checks: 134 cases/912 assertions pass; full evidence and
remaining unrelated contracts-sync failure are in protocol checkpoint. Next:
review changes; user-driven live acceptance only with authorization.

## Previous checkpoint: 2026-10-15 (Claude Code, v2.1 closed)

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
