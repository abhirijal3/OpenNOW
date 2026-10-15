# Live driver and same-seat handover

`tools/run_live_session.py` is the live orchestration driver for one GFN session through the POP. It has no
duration limit by default, accepts a private `--relay-key-file`, writes private context and POP logs under
`--log-dir`, and on SIGTERM stops and reaps the POP, releases the GFN seat and reaps the core.
`tools/test_run_live_session.py` covers duration and core cleanup (`python3 -m unittest discover -s tools -p test_run_live_session.py`).

## Reclaiming an official-client seat (2026-10-12)

`tools/run_live_session.py --resume-session-id ID` (with the usual region, codec, resolution and key-file
options) reclaims a game the official client is running. The launcher lists seats with `session.remote.list`,
requires the exact ID, calls `session.claim`, and polls until ready with `resumePending` cleared before
`streamer.prepare`. A discovery or claim failure never allocates a substitute seat. Normal termination still
releases the seat, so a stale launcher must not run its cleanup while an official client owns that game.

Customer flows never use claim: every customer starts on a fresh seat (`session-management-future.md`).
