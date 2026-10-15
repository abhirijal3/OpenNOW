# Core: settings and HDR

Part of the `opennow-core` interface; the handshake, envelopes and method list are in `core-protocol.md`.

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
