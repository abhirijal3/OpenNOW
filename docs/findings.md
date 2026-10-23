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
