# OpenNOW core protocol

`opennow-core` is the GeForce NOW protocol client. It signs in to NVIDIA, keeps credentials,
books and manages cloud seats through CloudMatch, and prepares the session context the native
streamer consumes. It speaks newline-delimited UTF-8 JSON on standard input and output.
Standard error is reserved for redacted diagnostics. A line is limited to 1 MiB. Unknown,
malformed or oversized protocol messages terminate the connection instead of leaving the
caller in an ambiguous state.

Everything that only served the old desktop shell (catalog, store and library browsing,
artwork, subscription and account-link screens, profile PINs, media library, updater, push
invalidation, Discord, telemetry and the legacy streamer subprocess launcher) is gone.

## Handshake

The first request is always:

```json
{"type":"request","id":"1","method":"core.hello","params":{"protocolVersion":6}}
```

The core answers with the same protocol version, its `coreVersion` and a `capabilities` list
(`settings`, `gfn.deviceAuth`, `gfn.providers`, `gfn.regions`, `gfn.cloudmatch`, `sessionProxy`,
`nativeStreamer.v7`, `nativeStreamer.ownedNvstNegotiation`,
`redactedDiagnostics`). Any other protocol version is rejected with `incompatible_protocol`.
Protocol 6 removed every method that is not listed under Methods. The native streamer
protocol remains 7.

## Envelopes

```json
{"type":"request","id":"42","method":"settings.get","params":{}}
{"type":"response","id":"42","ok":true,"result":{"settings":{}}}
{"type":"response","id":"42","ok":false,"error":{"code":"invalid_setting","message":"..."}}
{"type":"event","name":"settings.changed","payload":{"key":"fps","value":120}}
{"type":"cancel","id":"42"}
{"type":"ack","id":"42"}
```

Request IDs are unique within a core process. The core admits at most eight RPC workers, with
at most four background workers (`network.regions.ping`); excess requests receive `busy`, as do
duplicate active IDs. Cancelled requests suppress their response. Retry loops stop at
cooperative checkpoints. An already-running blocking HTTP, DNS or TCP operation is not
forcibly interrupted and keeps its own timeout. Mutations already dispatched are not rolled
back.

A data directory (`--data-dir`, `OPENNOW_DATA_DIR`, or the XDG config directory) is owned by one
core process at a time through `core.lock`. A second core on the same directory exits with
`The OpenNOW data directory is already in use`.

## Methods

- `core.hello`
- `app.status`
- `settings.get`, `settings.set`, `settings.reset`
- `auth.providers.list`
- `auth.session.get`
- `auth.device.start`, `auth.device.poll`, `auth.device.complete`, `auth.device.cancel`
- `auth.logout`
- `auth.accounts.logoutAll`, `auth.accounts.list`, `auth.accounts.switch`, `auth.accounts.remove`
- `network.regions.list`, `network.regions.ping`
- `session.create`, `session.poll`, `session.stop`, `session.active.get`
- `session.remote.list`, `session.claim`, `session.ad.report`
- `streamer.prepare`
- `diagnostics.snapshot`

Events: `settings.changed`, `settings.reset`, `auth.session.changed`, `session.changed`,
`session.cleanup.pending`.


## Other core docs

- `core-auth.md`: device login, logout, saved accounts, provider routing and account scope.
- `core-settings.md`: the settings store, language values and the HDR opt-in.
- `core-sessions.md`: seat create, poll, stop, claim, discovery, error codes and `streamer.prepare`.

## Diagnostics

`diagnostics.snapshot` returns the bounded, redacted entry list the core records for every RPC
(method, outcome and duration), session hand-off and streamer preparation failure. Tokens,
URLs, e-mail addresses and local user paths are redacted. The log rotates at 5 MiB.

## Stderr lines (POPSW-21)

Everything the core and the streamer library print to stderr is one leveled line,
`[rasbari:<level>] <name>: <text>`, where `<level>` is debug, info, warn or error, `<name>` is a fixed
snake_case event name and `<text>` holds the changing facts as `key=value` pairs, ending in `next=...`
when a failure has a fallback. On a POP the core's stderr is `core.log` and the streamer's is `v2pop.log`
(the streamer runs inside `v2pop`), and the POP controller reads both. The JSON on stdout is not touched.
Abhi's rules of 2026-10-24 (`rasbari-platform/docs/logging-quality.md`) decided which lines exist: an
action or a state change (device authorization started and answered, saved session revoked, ICE state
changed, DTLS handshake completed, data channel opened, a send failing), never one per packet or message.
The old per-message and per-interval narration (cursor wire dumps, control and receiver statistics, NACK
sends, first-frame hex) is now debug and only printed with `OPENNOW_NVST_TRACE=1`; the raw input packet
bytes that used to follow "native input rx" and "encoded input tx" are gone, since keys a player types must
never reach a log. A line printed more than 10 times in one second is printed 10 times and then replaced by
one `<name>: repeated N more times in 1 s` line. The code is `native/opennow-core/src/rlog.rs` and
`native/opennow-streamer/crates/opennow-streamer-protocol/src/rlog.rs` (the same format as the protocol
repo's `v2/src/rlog.c`); a test in each fails if any other file prints to stderr by hand, and
`native/opennow-core/tests/core_log_levels.rs` runs the core and checks every stderr line has a level.
The streamer's counted line for a burst at the very end of a run is written by the next line after it,
since the library has no exit hook.
