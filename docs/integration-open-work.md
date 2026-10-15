# Integration: what is still open

Done and no longer listed here: video key export to the POP, the rate and invalidation levers, encrypted audio
pass-through, raw input and cursor pass-through, timed input, microphone, player timing and profile.

## Library and POP

1. A Linux capacity test: several sessions on one Ubuntu box, CPU and RAM per session. One NVIDIA account
   holds one seat, so it needs several accounts.
2. Many accounts in `opennow-core`: it holds one active account per process today.
3. Later, only if measurement asks for it: one event loop for every session instead of threads per session.
4. Live validation of the v2.1 work (timed input, microphone, player feedback, profile, timing). It is covered
   by tests but has not run end to end in a live session.
5. Session management gaps (outcome list, lifecycle events, POP loss, cleanliness gate):
   `session-management-future.md`.

## Open decisions

- The library asks GFN for `x-nv-vqos[0].drc.enable:0`, so GFN never changes resolution, only bitrate.
  Whether the middleman should allow resolution changes is open (Abhi, 2026-10-08).
- DESIGN.md's "Four pieces" text still says whole access units while the code is pass-through.
- One loop for many sessions against v1's one process per session.
- The terms-of-use question about relaying a GFN session.
