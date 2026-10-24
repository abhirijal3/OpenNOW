# Findings

Bugs noticed, failure scenarios that deserve a test, and improvements worth making, written by any agent working here (core value in AGENTS.md). Newest at the bottom under a dated heading. Delete an entry once it is fixed or turned into a decision, and say where it went.

### 2026-10-15

- `docs/session-management-future.md` says to keep `session.claim` out of customer flows. Abhi chose on 2026-10-15 to use seat claim to rescue a session whose Mumbai POP died (rasbari-platform `docs/notes/decisions.md`, round 1), so that line needs correcting.

### 2026-10-19 (Codex, explicit clipboard text)

- The existing native Unicode submit_text queue was not exported to C, so protocol production paste could not reach it. Added text/text_available ABI and bounded modifier-release-before-text submission (PLAYER-13), tested without GFN. A real host may disable Unicode input via CustomImeConfig; readiness is not capability proof. The private clipboard custom-message route and reliable multi-POP paste remain open.


### 2026-10-20: D2 actual negotiated packet size

Fixed the measured-size path so an explicit invalid measurement errors rather than falling back to an oversized default. ANNOUNCE uses the advertised tag size; an additive C ABI returns the actual whole GCM8 video datagram for the POP's key message. Raw encrypted relay now rejects profiles the existing player wire cannot represent, while ordinary receivers retain support. Rust workspace 548 tests and release GFN library build pass; no live NVIDIA negotiation occurred in this chat. Details: `protocol-integration.md`, D2 section.

### October 23, 2026: negotiated metadata cleared during successful start

The POP's video-size getter returned zero after successful start because start assigned native_video_datagram_bytes/profile/timing before stop_media_resources cleared them. Metadata is now assigned after cleanup. A local UDP regression with the session 14 route budget (1195 bytes) proves the 1192-byte negotiated size, profile and timing survive start and clear on stop. Mac and Linux regressions pass; no NVIDIA access is needed. Protocol's controller also isolates pruning permission failures that previously suppressed start_failed.

### October 24, 2026: the discovery fallback on the Mumbai POP is the designed one

Platform fix list item 18 read the line `provider: <code> discovery endpoint is unreachable; using fallback https://latam-west.dig.geforcenow.nvidiagrid.net/` on the Mumbai staging POP as a failed lookup. It is `effective_provider_url_with` in `native/opennow-core/src/gfn.rs` doing what it was written for: the account's login provider is the Digevo alliance partner (idp `IsvVBA3Aj8KZ7gwwuRUhB6-tOF2o2F1wncD-XjYv100`), whose discovery advertises only `prod.dig.geforcenow.nvidiagrid.net`, a geo-steered name that does not resolve from outside the partner's footprint, so the core uses the LATAM West endpoint (routing test `routing_tests.rs` around line 1485 covers it). The later region lookup still found the India region and a Mumbai rig, so nothing was lost; only the first discovery host differs. No code change. What stays worth doing, not done: the line goes to stderr as free text, so a POP sees it only by reading the log. If the controller ever needs to know which provider endpoint a session used, make it a named event next to the other `provider:` lines; and check at the next live session that the fallback does not move a Mumbai account to a LATAM rig (it did not on 2026-10-23: serverId and region were India).
