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
