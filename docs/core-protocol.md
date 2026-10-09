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

## Authentication

Auth responses and `auth.session.changed` events share one envelope. `session` is either null
or an allowlisted object containing `user` and `provider`. Credentials and issuing-client
details stay private to the core. The envelope also carries the core-process account
`generation`, `persistence`, `refresh`, `warnings` and `deviceIdentity`. A token refresh does
not change the account generation.

`auth.device.start` returns `attemptId`, `userCode`, `verificationUri`,
`verificationUriComplete`, `expiresAt` and `intervalSeconds`. Poll, complete and cancel use
`attemptId` only. The core retains the device grant and enforces the deadline, one in-flight
poll and cumulative five-second `slow_down` increments. Pending poll replies include
`retryAfterMs`. Cancellation before the completion commit fence prevents persistence and
publication; once the fence is entered the login is committed even if the response is
cancelled, and `auth.session.get` reconciles it. Logout or account replacement invalidates
pending login work.

Sessions are saved as JSON, one file per account under `data_dir/fallback-sessions/<sha256 user>.json`
with private file permissions. The core never uses an OS keychain. `local-file` is the normal
persistence state, `memory-only` never writes a file, `migration-pending` means a recoverable
legacy source remains, and `unavailable` means restoration failed. Warnings identify deferred
cleanup without containing credentials.

Logout always ends local auth ownership and reports `remoteRevoke` separately from
`localCleanup`. Revocation is a best-effort DELETE of the selected client grant with an access
bearer; all-account revocation has a five-second total network budget. `auth.accounts.list`
returns the saved accounts and `activeUserId`; `auth.accounts.switch` and
`auth.accounts.remove` act on a saved `userId`.

The core persists one versioned 64-hex device identity. Corrupt or unwritable identity storage
is not replaced: new login is blocked and restoration reports an unavailable device identity.

## Provider routing and account scope

`auth.providers.list` returns `providers`, `defaultProviderIdpId`, `generation` and
`discovery: {state, message, retryAfterMs}`. Successful discovery is fresh for 15 minutes. A
failed refresh keeps the last known providers and reports `state: "degraded"`. Without a
discovered list the response includes the restored provider and an explicit NVIDIA fallback,
which is not authoritative discovery. Retries wait at least 30 seconds, and HTTP 429
`Retry-After` can extend the wait up to one hour. An unknown `providerIdpId` fails with
`provider_unavailable`.

Authenticated reads and session results include `scope: {generation, providerIdpId, userId}`.
Core reads capture private ServiceId credentials and reject obsolete account or provider
generations before publishing. HTTP 401 permits one renewal and one replay of a safe read for
the same owner. HTTP 403 does not renew. Create and claim never replay after an ambiguous
response. Authenticated HTTP clients do not follow redirects.

`settings.set` for `region` also requires the current `providerIdpId`. The core updates
`region`, `regionProviderIdpId` and the `providerRegions` map together; the metadata fields
cannot be written separately. A region override must occur in the current provider's
server-info list, otherwise the request falls back once to the provider base without deleting
the saved preference. Provider and region bases must be HTTPS NVIDIA-grid names without
userinfo or nonstandard ports. `network.regions.list` returns `regions` and `vpcId` for the
signed-in provider. `network.regions.ping` takes `{regions:[{url}]}` (at most 32) and returns
a per-region TCP `pingMs`.

## Settings

`settings.get` returns `{settings}`. The store holds only what the session path reads:
`resolution`, `fps`, `maxBitrateMbps`, `saveBandwidth`, `codec`, `fallbackCodec`,
`decoderPreference`, `nativeVideoBackend`, `colorQuality`, `enableHdr`, `enableCloudGsync`,
`nativeCloudGsyncMode`, `enableL4S`, `enablePersistingInGameSettings`, `identifyAsSteamDeck`,
`region`, `regionProviderIdpId`, `providerRegions`, `gameLanguage`, `keyboardLayout`,
`networkTest`, `sessionProxyEnabled` and `sessionProxyUrl`. Unknown keys found in an existing
`settings.json` are kept on disk and never exposed. `settings.set({key, value})` normalizes the
value and answers with `{key, value}`, plus `changes` when a color-quality change repairs an
incompatible explicit codec. `settings.reset` restores the defaults.

`gameLanguage` and `keyboardLayout` are independent. Game language identifiers keep their exact
case, separators, script and numeric-region subtags (ASCII alphabetic first subtag of 2 to 8
bytes, further alphanumeric subtags of 1 to 8 bytes separated by `_` or `-`, 64 bytes in total);
`auto` and `system` are rejected. Corrupt stored values are replaced by `en_US` and `en-US`
before create, resume or claim requests.

`nativeHdrSupported` and `nativeHdrDisplay` are transient runtime capabilities, never settings.
`settings.set` rejects them and the loader discards legacy copies.

## Sessions

`session.create` allocates a fresh CloudMatch seat. The caller supplies everything CloudMatch
needs; the core performs no catalog lookup. Parameters:

- `appId` (required): the numeric launch ID.
- `variantId` (required): the selected variant, numeric and equal to `appId`. Anything else is
  `invalid_params`, before any network request.
- `scope` (required): the current `{generation, userId, providerIdpId}`. A mismatch is
  `stale_account`.
- `title` (optional string): sent as `internalTitle`.
- `accountLinked` (optional boolean, default false): sent as `accountLinked`.
- `supportsInGameSettingsPersistence` (optional boolean, default false): enables
  `enablePersistingInGameSettings` together with the setting of the same name.
- `zone`, `streamingBaseUrl` (optional): an explicit dynamic NVIDIA queue route. The pair must
  agree and match `NP-<id>-<nn>`; otherwise the saved provider region is used.
- `appLaunchMode`, `networkTest`, `maxEntitledFps` (optional): forwarded to the CloudMatch body.
- `runtimeCapabilities` (optional): the streamer's decode capabilities. When present the core
  resolves Auto codec and color against them and rejects profiles the streamer cannot decode
  before allocating a seat. Without it HDR is rejected and the saved settings are used as is.

An active seat blocks another create with `session_update_busy`. The result is
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

## Streamer preparation

`streamer.prepare({session, runtimeCapabilities?})` validates that the owned session is ready
(status 2 or 3, an `rtsps://` endpoint present), checks the accepted profile against the
runtime capabilities and returns `{protocolVersion: 7, context, session}`. `context` carries
`session`, the normalized `settings` (codec resolved to a concrete name, `transportMode` fixed
to `nvst`, HDR and color taken from the accepted profile), an empty `shortcuts` object and
`surface: null`. Incomplete or unsupported accepted color is rejected before attachment, and an
accepted HDR session needs 10-bit HEVC (4:2:0 or 4:4:4) or AV1 4:2:0 and current runtime
capabilities.

## HDR

`enableHdr` is a persisted opt-in, default false. The caller adds `nativeHdrSupported` (the
actual stream output's current HDR state) and optionally `nativeHdrDisplay` (`minimumNits`,
`maximumNits`, and optionally `maximumFullFrameNits` with `redX/redY`, `greenX/greenY`,
`blueX/blueY`, `whiteX/whiteY`) to `runtimeCapabilities` on each `session.create` and
`streamer.prepare`. Missing, false or malformed support denies HDR. HDR needs a hardware HEVC
or AV1 decoder whose `colorQualities` include the requested ten-bit profile; explicit H.264 and
software decoding fail before a seat is allocated. CloudMatch receives `sdrHdrMode=1` only for
a validated HDR request. The display snapshot is validated again by the core and dropped as a
whole when invalid; without one, HDR requests use the fixed requested-content defaults of
1000 nit maximum, 0 minimum and 400 nit frame average. Snapshots are never persisted or
replayed.

## Diagnostics

`diagnostics.snapshot` returns the bounded, redacted entry list the core records for every RPC
(method, outcome and duration), session hand-off and streamer preparation failure. Tokens,
URLs, e-mail addresses and local user paths are redacted. The log rotates at 5 MiB.
