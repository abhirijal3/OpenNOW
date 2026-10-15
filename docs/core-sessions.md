# Core: sessions

Part of the `opennow-core` interface; the handshake, envelopes and method list are in `core-protocol.md`.

## Seats

`session.create` allocates a fresh CloudMatch seat. The caller supplies everything CloudMatch
needs; the core performs no catalog lookup. Parameters:

- `appId` (required): the numeric launch ID.
- `variantId` (required): the selected variant, numeric and equal to `appId`. Anything else is
  `invalid_params`, before any network request.
- `scope` (required): the current `{generation, userId, providerIdpId}`. A mismatch is
  `stale_account`.
- `title` (optional string): sent as `internalTitle`.
- `accountLinked` (optional boolean, default false): sent as `accountLinked`.
- `supportsInGameSettingsPersistence` (optional boolean): accepted and ignored. Create always sends
  `enablePersistingInGameSettings: false` so no seat inherits the previous user's in-game state.
- `zone`, `streamingBaseUrl` (optional): an explicit dynamic NVIDIA queue route. The pair must
  agree and match `NP-<id>-<nn>`; otherwise the saved provider region is used.
- `appLaunchMode`, `networkTest`, `maxEntitledFps` (optional): forwarded to the CloudMatch body.
- `runtimeCapabilities` (optional): the streamer's decode capabilities. When present the core
  resolves Auto codec and color against them and rejects profiles the streamer cannot decode
  before allocating a seat. Without it HDR is rejected and the saved settings are used as is.

An active seat blocks another create with `session_update_busy`. A create whose outcome cannot
be read (the request was sent and the connection dropped or timed out, a 2xx body that is not
JSON, or success without a session ID) fails with `session_create_outcome_unknown`: a seat may
exist. Every further create then fails with the same code until the account is checked.
`session.remote.list` completing with no seat for that app clears the block; so does a
`session.stop` of the seat it found. A refused connection never reached CloudMatch and stays
`network_error`. The block is held in memory only. An HTTP 500 or status 4 carrying
`8A960000` (CloudMatch capacity exhausted or the VM still deprovisioning) fails with
`session_capacity_unavailable`, which means no seat was created and a later retry is safe. The result is
`{session, scope}`. A successful create must be acknowledged with `{"type":"ack","id":"<id>"}`
within ten seconds; an unacknowledged fresh allocation is deleted by the CloudMatch owner. A
create response is not also broadcast as `session.changed`. Cleanup failure retains the seat
and reports `session_cleanup_pending` (also as the `session.cleanup.pending` event); the failed
cleanup record in `pending-session-cleanup.json` contains no tokens.

`session.poll`, `session.stop`, `session.claim`, `session.ad.report`, `session.active.get` and
`session.remote.list` operate on the seat owned by the current account and provider. Another
seat or owner fails with `session_owner_mismatch`. `session.poll` with `recoveryMode: true`
does not broadcast `session.changed`. Raw CloudMatch status 7 is `phase: "finished"` with a
`termination` object (`source: "cloudmatch-session-status"`, `resumable: false`); an exact-seat
GET that returns 404 yields `session: null` with a top-level termination
(`source: "cloudmatch-http"`, `httpStatus: 404`).

`session.active.get` without hints is a local lookup. After a core restart a caller can pass
`{sessionId, ownerScope: {userId, providerIdpId}}` to rediscover the exact seat through
authenticated, core-owned routes and restore control without RESUME, allocation or transport
changes. The session ID must be 1 to 256 ASCII letters, digits, hyphens or underscores. A
missing match fails with `session_discovery_failed`; local absence is never termination.

`session.remote.list` checks the selected endpoint and the regions CloudMatch advertises, at
most four concurrent requests, three seconds each, 12 seconds in total and 32 regional
endpoints. If nothing is found but a region failed or could not be checked, it fails with
`session_discovery_failed` instead of returning an empty list. Callers must not create a new
session after a discovery failure.

Negotiated profiles keep normalized `bitDepth` (8 or 10), `chromaFormat` (0 or 1) and
per-component `*Source` fields (`request`, `finalized`, `server`, `unreported`). Same-seat
partial updates keep previously known components.

## Seat setup steps

The core passes the server's `seatSetupInfo.seatSetupStep` through as `seatSetupStep` next to `queuePosition`.
Steps do not form a fixed sequence and give no percentage; unknown values mean neither done nor failed.

| Step | Meaning |
| --- | --- |
| 0 | Connecting |
| 1 | Waiting for an available rig (`queuePosition` when supplied) |
| 2, 3, 4 | Configuring the rig |
| 5 | Cleaning up the previous session |
| 6 | Waiting for cloud storage |
| missing or other | Preparing the game |

A positive `queuePosition` keeps the queue meaning unless the server reports step 5 or 6. These numbers are
not the streamer's own session-setup states; do not mix the two.

## Streamer preparation

`streamer.prepare({session, runtimeCapabilities?})` validates that the owned session is ready
(status 2 or 3, an `rtsps://` endpoint present), checks the accepted profile against the
runtime capabilities and returns `{protocolVersion: 7, context, session}`. `context` carries
`session`, the normalized `settings` (codec resolved to a concrete name, `transportMode` fixed
to `nvst`, HDR and color taken from the accepted profile), an empty `shortcuts` object and
`surface: null`. Incomplete or unsupported accepted color is rejected before attachment, and an
accepted HDR session needs 10-bit HEVC (4:2:0 or 4:4:4) or AV1 4:2:0 and current runtime
capabilities.
