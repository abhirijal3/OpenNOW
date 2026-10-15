# AGENTS.md

Scope note: Abhi's global `~/.claude/CLAUDE.md` rules (no code comments, dev-base gate,
mobile handoff, etc.) are written for Maptician's JS/TS work. They do not apply in this
repo. This file is the shared source of project rules for all agents working here.

## Shared context: Claude Code, Codex, Gemini, and Antigravity

`AGENTS.md` is the single source of truth for project rules and context. Codex discovers
it directly; `CLAUDE.md` and `GEMINI.md` import it (`@AGENTS.md`). Antigravity loads it
via workspace rules. Edit shared rules here, never maintain separate copies. Follow the
current user's instructions and the host's higher-priority rules. Agent-specific tools,
permissions, and private memories are not shared across these files.

At the start of work, read the current checkpoint at the top of `docs/where-we-left-off.md`.
Verify the checkout, branch, HEAD, and working tree before relying on a checkpoint.
Historical sections are evidence of earlier work, not current instructions.

Before pausing, handing off, or ending a turn that changes project state, update the
current checkpoint in `docs/where-we-left-off.md`. Include the date and agent, objective,
completed and unfinished work, branch/commit, uncommitted changes, exact validation
commands/results and log paths, open questions, standing user constraints, running
processes, and the next steps in order. Keep that section concise; link detailed reports.
Never describe uncommitted work as committed or simulated tests as live validation.
A pause remains a pause until the user resumes the work; saving a handoff is not permission
to continue implementation. Ask about unresolved product decisions instead of guessing.

Keep one shared handoff file, not separate Claude/Codex/Gemini status files.

## What this branch is

OpenNOW's GFN client, being turned into a library (`libopennow_gfn.a`) that our streaming
protocol embeds as its "GFN client" role. The desktop app is gone. What's left is Rust:

- `native/opennow-core/` signs in to NVIDIA, refreshes tokens, stores credentials and runs the
  CloudMatch session lifecycle (create, poll, stop, claim), then builds the session context the
  streamer consumes. It runs as a child process speaking newline-delimited JSON
  (`docs/core-protocol.md`) until the library's C ABI replaces that.
- `native/opennow-streamer/` runs the stream: RTSPS setup, the Mjolnir/ICE/DTLS/SCTP transport,
  SRTP, depacketizing, input, QoS and recovery. It decodes nothing and draws nothing: whole video
  access units (tagged with GFN's frame number), Opus packets and cursor events go to the embedder,
  and input comes back in.

Only GFN protocol code belongs here. No decoding, rendering, audio playback, capture or UI.

The only target is Ubuntu Linux on our servers. macOS and Windows code paths are dead weight and
go as the code they live in is reworked; don't add new ones.

How the protocol's POP uses this library, the session steps, measured cost and what is left:
`docs/protocol-integration.md`.

## Cross-repo context and reading order

What the library has to do, and the rest of the GFN story, lives in the protocol repo:
`~/Documents/Rasbari/protocol`.

Start by reading `~/Documents/Rasbari/protocol/docs/v2/README.md`. It is the live index of the
v2 design and every GFN document, and it gains a line whenever something new is written, so
re-read it at the start of each task instead of trusting what you read earlier. Read those
files where they are; never copy them into this repo, because they keep changing.

Read first, in order:
1. `~/Documents/Rasbari/protocol/docs/v2/design/architecture.md` (indexed by `docs/v2/DESIGN.md`), the "Four pieces" section: what the GFN client role
   does and the levers it exposes (frames out tagged with GFN's frame number, audio, cursor, input in,
   the bandwidth field of the 0x0207 report, 0x0301 invalidation, 0x0302 IDR, 0x0318 max bitrate).
2. `~/Documents/Rasbari/protocol/docs/v2/gfn-barebones.md`: GFN's send and receive path at its simplest.
3. `~/Documents/Rasbari/protocol/docs/gfn-protocol.md` when you need how a mechanism works, and the
   line-cited files under `~/Documents/Rasbari/protocol/gfn-ghidra/` (server/, client/, and the
   dumps/*.md guides) when you need proof. Never read a dumps/*.c file top to bottom: use its .md guide
   and grep.
4. `docs/protocol-integration.md` in this repo: keep it current whenever the library's API, behavior
   or measured cost changes.

Don't edit anything in the protocol repo except one thing. When you settle something the protocol
side depends on (the library's API, its threading, what it hands over and when, how each lever is
applied), write it in `~/Documents/Rasbari/protocol/docs/v2/opennow-library.md` and add a line for it to
`docs/v2/README.md`. That is how this side learns what you decided.

## Sibling repos (all under `~/Documents/Rasbari`)

`~/Documents/Rasbari/protocol` links this library into its POP (`v2/Makefile`, `OPENNOW_DIR`, default
`../../OpenNOW`). `~/Documents/Rasbari/moonlight-qt` is the player at the other end of the protocol and
never touches this repo directly; it reaches GFN only through the protocol's v2 client. Chain: GFN, then
this library inside `v2pop`, then v2 UDP, then Moonlight. Keep the three side by side.

## Writing docs (HARD RULE)

Every doc written or changed gets an entry in its folder's index (`docs/README.md`)
in the same turn: the file path plus one short plain line saying what it covers and when to read it.
Write the line so an agent can decide from the index alone whether to open the file, and so it reads the
one doc it needs and nothing extra. A doc with no index line does not exist for the next agent. Keep docs
narrow: one topic per file, split a file that grows to cover two.

## Standing user rules

- Do the work in the main session. Spawn agents/workflows only with Abhi's explicit
  agreement to a bounded plan (count, duration and reading scope); never nest spawning.
  On a stop/pause, cancel and verify every descendant and agent-owned background task
  before reporting stopped. Do not start a watcher or scheduled continuation unasked.
- Ask before changing server access/authentication: SSH password/root login, firewall,
  users or login restrictions. State the concrete change and how to undo it.
- Before creating/renting billed resources, show the exact provider, hardware, region
  and hourly price and obtain approval for that purchase.
- Explain mechanisms or measured results through concrete representative examples
  with actual numbers. Never invent measurements to illustrate a point.
- A live session needs a real NVIDIA/GFN account. Static reverse engineering does not
  establish live negotiated behavior. No live GFN session or capture until Abhi authorizes it.

## Priorities

Performance and reliability first. Behavior stays predictable under load and through failures:
session restarts, reconnects, partial streams, lost packets. Pick correctness over convenience.

## Streamer rules

- Sender-authored packet, frame, timestamp and stream identifiers are protocol data. Keep them end
  to end through transport, queues, feedback and diagnostics; never swap in local counters. GFN's
  frame number is what the protocol side keys on.
- Receive, depacketize, assemble and input each sit on an explicit thread boundary. Nothing blocks
  the receive thread.
- Queues are bounded and observable. Define what happens on overflow, discontinuity, missing
  references and late frames. Recovery asks for a valid reference frame (0x0301 or 0x0302) and never
  passes on corrupt data silently.
- Feedback to GFN (frame acks, loss reports, the 0x0207 QoS report, keyframe requests, max bitrate)
  uses negotiated or sender-provided values and the documented wire formats. Check changes against
  captured diagnostics and focused protocol tests, not timing guesses.
- Diagnostics keep the receive, assembly, hand-off and feedback stages distinguishable.

## Boundaries

- Values crossing the core's JSON protocol or a C ABI are serializable, bounded and explicitly
  typed. When one changes, update every producer, consumer, fixture and version check together.
- Keep provider/alliance behavior, stable device IDs, persisted account and session compatibility,
  and recovery semantics intact when refactoring.
- No platform handles or graphics objects in anything the library hands out.

## Maintainability

- Before adding code, look for an owner that already does it. Duplicate logic is a smell.
- Deleting is welcome. Refactors keep behavior the same unless a change is asked for.
- Small typed helpers over broad utility modules.
- Release pressed keys, buttons and controller state on every shutdown and failure path.

## Build and check

- Core: `cargo test --manifest-path native/opennow-core/Cargo.toml`
- Streamer: run the affected crate's tests first, then
  `cargo test --manifest-path native/opennow-streamer/Cargo.toml --workspace`.
- Don't call something done while a relevant test fails. Report the command and the failure.
- A live session needs a real NVIDIA/GFN account. Until the library has its own driver, nothing on
  this branch runs one; `main` still has the full app for side-by-side checks.
