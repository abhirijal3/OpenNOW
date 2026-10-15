# Core: authentication and account scope

Part of the `opennow-core` interface; the handshake, envelopes and method list are in `core-protocol.md`.

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
