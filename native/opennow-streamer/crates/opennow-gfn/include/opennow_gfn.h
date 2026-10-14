#ifndef OPENNOW_GFN_H
#define OPENNOW_GFN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OPENNOW_GFN_MEDIA_VIDEO 0u
#define OPENNOW_GFN_MEDIA_AUDIO 1u

typedef struct OpennowGfn OpennowGfn;

typedef struct {
    uint32_t kind;
    const uint8_t *data;
    size_t len;
    const uint8_t *codec;
    size_t codec_len;
    uint32_t frame_index;
    uint8_t has_frame_index;
    uint8_t keyframe;
    uint64_t rtp_timestamp;
    uint32_t clock_rate_hz;
    uint64_t received_at_us;
} OpennowGfnMedia;

typedef struct {
    uint8_t controller_id;
    uint16_t bitmap;
    uint16_t buttons;
    uint8_t left_trigger;
    uint8_t right_trigger;
    int16_t left_stick_x;
    int16_t left_stick_y;
    int16_t right_stick_x;
    int16_t right_stick_y;
} OpennowGfnGamepad;

typedef void (*OpennowGfnBytesFn)(void *user, const uint8_t *data, size_t len);
typedef void (*OpennowGfnMediaFn)(void *user, const OpennowGfnMedia *media);

typedef struct {
    void *user;
    OpennowGfnBytesFn on_video_packet;
    OpennowGfnMediaFn on_media;
    OpennowGfnBytesFn on_event;
} OpennowGfnCallbacks;

/**
 * Creates an idle GFN client. Any callback may be NULL.
 *
 * on_video_packet gets every GFN video datagram exactly as it arrived on the
 * video socket (encrypted, GS header in the clear), on the library's video
 * receive thread. The bytes are valid only during the call. Setting
 * on_video_packet puts the library in header-only mode: it never decrypts,
 * repairs or assembles video, and only reads the clear RTP and GS headers to
 * send GFN its frame acknowledgements, timing and QoS feedback.
 *
 * on_media gets the library's own output: Opus audio only when on_video_packet
 * is set, otherwise also whole video access units with GFN's frame number.
 * on_event gets every engine event and every
 * command response as UTF-8 JSON (not NUL-terminated). Both run on library
 * threads; pointers are valid only during the call.
 *
 * Returns NULL when callbacks is NULL.
 */
OpennowGfn *opennow_gfn_create(const OpennowGfnCallbacks *callbacks);

/**
 * Runs one engine command, given as JSON: {"id","type",...}. Types are hello
 * (protocolVersion 7), nvst-bind, nvst-unbind, nvst-send, start (context: the
 * session context from opennow-core's streamer.prepare), anti-afk-pulse,
 * request-idr (active NVST session), stop and shutdown. Responses arrive through on_event. Blocks until the
 * command finishes; start can take seconds.
 *
 * Returns 0 when the command ran, -1 for a NULL handle or malformed JSON.
 */
int32_t opennow_gfn_command(OpennowGfn *gfn, const uint8_t *json, size_t len);

/* Install before start. Receives bounded complete native control commands,
 * including headers and versioned tails, on the bundle receive thread. */
/* Raw original audio RTP plus derived receive keys: profile 0=AES256-GCM16,
 * 1=AES128-GCM16 (upper key bytes zero). Callback pointers last only for the call. */
typedef void (*OpennowGfnAudioFn)(void *user, const uint8_t *bytes, size_t len, const uint8_t *key, const uint8_t *salt, uint8_t profile);
int32_t opennow_gfn_set_audio_callback(OpennowGfn *gfn, OpennowGfnAudioFn callback, void *user);
int32_t opennow_gfn_set_control_callback(OpennowGfn *gfn, OpennowGfnBytesFn callback, void *user);
/* Active-session ownership transition. Discards feedback from the previous owner. */
int32_t opennow_gfn_set_player_feedback(OpennowGfn *gfn, uint8_t enabled);
/* Non-blocking enqueue onto active negotiated control channels; -1 on unavailable,
 * malformed, unsupported, or full. Does not use the private custom channels. */
int32_t opennow_gfn_send_control(OpennowGfn *gfn, const uint8_t *bytes, size_t len);
/* Zero means no fresh RTT measurement. Queue lifetime is 1..84000 us; the negotiated SCTP role retains its own reliability policy. */
/* Non-secret 40-byte v2 player-profile wire image; -1 if no active profile. */
int32_t opennow_gfn_player_timing(OpennowGfn *gfn, uint8_t *out, size_t cap);
int32_t opennow_gfn_player_profile(OpennowGfn *gfn, uint8_t *out, size_t cap);
uint32_t opennow_gfn_upstream_rtt_us(OpennowGfn *gfn);
int32_t opennow_gfn_send_control_deadline(OpennowGfn *gfn, const uint8_t *bytes, size_t len, uint32_t lifetime_us);

/**
 * Queues input for the active session. Safe from any thread. Returns 0, or -1
 * for a NULL handle or pad.
 */
/* Availability: 1 available, 0 unavailable, -1 invalid/busy. Enable/Opus: 0 accepted, -1 rejected. */
int32_t opennow_gfn_microphone_available(OpennowGfn *);
int32_t opennow_gfn_microphone_enable(OpennowGfn *, uint8_t);
int32_t opennow_gfn_microphone_opus(OpennowGfn *, const uint8_t *, size_t, uint32_t);

/* Timed variants preserve the player's original monotonic microsecond clock. */
int32_t opennow_gfn_key_at(OpennowGfn *, uint16_t, uint16_t, uint8_t, uint64_t);
int32_t opennow_gfn_mouse_move_at(OpennowGfn *, int16_t, int16_t, uint64_t);
int32_t opennow_gfn_mouse_abs_at(OpennowGfn *, uint16_t, uint16_t, uint16_t, uint16_t, uint64_t);
int32_t opennow_gfn_mouse_button_at(OpennowGfn *, uint8_t, uint8_t, uint64_t);
int32_t opennow_gfn_mouse_wheel_at(OpennowGfn *, int16_t, int16_t, uint64_t);
int32_t opennow_gfn_gamepad_at(OpennowGfn *, const OpennowGfnGamepad *, uint64_t);

int32_t opennow_gfn_key(OpennowGfn *gfn, uint16_t virtual_key, uint16_t modifiers, uint8_t pressed);
int32_t opennow_gfn_mouse_move(OpennowGfn *gfn, int16_t delta_x, int16_t delta_y);
int32_t opennow_gfn_mouse_abs(OpennowGfn *gfn, uint16_t x, uint16_t y, uint16_t width, uint16_t height);
int32_t opennow_gfn_mouse_button(OpennowGfn *gfn, uint8_t button, uint8_t pressed);
int32_t opennow_gfn_mouse_wheel(OpennowGfn *gfn, int16_t delta_x, int16_t delta_y);
int32_t opennow_gfn_gamepad(OpennowGfn *gfn, const OpennowGfnGamepad *pad);

/**
 * Stops any session, waits for the library's threads, and frees the handle.
 * No callback runs after it returns. NULL is ignored.
 */
void opennow_gfn_destroy(OpennowGfn *gfn);

#ifdef __cplusplus
}
#endif

#endif
