# Session management: future work

Written 2026-10-14. Customers stream through our company's GFN accounts, and every session starts on a fresh
seat. This file lists what is not built yet. What is built is in `docs/core-sessions.md`.

## Already in the core

Create is acknowledged within 10 s or the seat is deleted. A failed delete is kept in
`pending-session-cleanup.json` and blocks the next create. An active seat blocks another create. Create and
claim never replay after an ambiguous reply. Created seats always send `enablePersistingInGameSettings: false`.
A create whose outcome is unreadable fails with `session_create_outcome_unknown` and blocks further creates until
`session.remote.list` finds no seat for that app or `session.stop` removes the one it found. Capacity
exhaustion (`8A960000`) is `session_capacity_unavailable`, safe to retry.

## Still to do in the library

1. Persist the unknown-create block. It is in memory today, so a core restart forgets it. CloudMatch's
   per-device session limit is the only backstop after a restart. Write an intent record before the POST and
   reconcile it at startup. Check whether CloudMatch has any idempotency handle; none is known.
2. One stable outcome list for the POP and orchestrator. Today the codes are spread over `cloudmatch.rs`,
   `main.rs` and the streamer: auth failure, auth ok but create failed, capacity, queue timeout, session or
   hour limit, unknown outcome, upstream ended, POP lost. Each needs a retryable flag and a customer-visible flag.
3. Seat lifecycle events: queue position, remaining time before the hour limit, idle warning, termination
   cause (status 7, exact-seat 404, `nvst-recovery-exhausted`). Separate transport silence from provider
   termination by polling the seat in recovery mode: still running means reconnect, otherwise end it.
4. Startup reaper for orphan seats no booking record claims. It must never touch seats from the official client.
5. Verified cleanup: delete is idempotent and confirmed through `session.remote.list`.

## Still to do in the POP and orchestrator

The POP side of this list is the POP controller (`protocol/docs/v2/pop-controller.md`), and the orchestrator side is `rasbari-platform/docs/play-plan.md` (both decided 2026-10-15).

1. Store booking id to seat id at the create ack, outside the POP. If a POP dies the seat keeps running and
   counting down, so another POP adopts it (`session.active.get` with hints, then claim) or deletes it.
2. Library heartbeat: session id, seat status, last-frame age, so loss of a POP is visible.
3. Retry policy lives in the POP: bounded attempts, backoff with jitter, a cooldown between delete and create,
   region fallback. The library only reports.
4. Keep `session.claim` out of customer flows. It exists for same-seat handover from the official client.

## Prior-user exposure

Decision: fresh seat every time, never reuse across customers. Open questions:

1. The game can launch before cleanup runs. Hold video back from the customer until the orchestrator says go.
   The library already owns the frame path, so the POP can withhold frames.
2. Whether GFN already gives a clean VM per session is unproven. A deprovisioning cooldown suggests it does.
   A live probe (launch, change something, end, relaunch, look at the first I-frame) would settle it. That needs
   Abhi's authorization for a live session.
3. A first-I-frame check against known clean screens, plus driving logout through the existing input path, is
   only workable for a short whitelist of games. Risks: per-game fragility, UI changes, accidental purchases.

## Review follow-ups on the v2.1 commit (5b0f301b)

1. `player_profile_overrides` (`nvst_rtsp.rs`) fails the whole session prepare on one non-integer SDP value, and
   `nvst_profile.rs` rejects out-of-range values. The other agent made both strict on purpose. Decide whether a
   bad override should fall back to the default and log instead.
2. `opennow_gfn.h`: the "Queues input", "Zero means no fresh RTT" and "Install before start" comments sit above the
   wrong declarations.
3. `opennow_gfn_set_audio_callback`, `set_control_callback` and `set_player_feedback` block on the engine lock while
   every other new call uses `try_lock`.
4. New transport modules are dense and not rustfmt style, and the `pub mod` lines sit after the tests module.
