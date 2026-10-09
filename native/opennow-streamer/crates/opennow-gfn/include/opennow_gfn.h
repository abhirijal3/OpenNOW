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
 * receive thread, before the library processes it. The bytes are valid only
 * during the call.
 *
 * on_media gets the library's own output: whole video access units with GFN's
 * frame number, and Opus audio. on_event gets every engine event and every
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
 * stop and shutdown. Responses arrive through on_event. Blocks until the
 * command finishes; start can take seconds.
 *
 * Returns 0 when the command ran, -1 for a NULL handle or malformed JSON.
 */
int32_t opennow_gfn_command(OpennowGfn *gfn, const uint8_t *json, size_t len);

/**
 * Queues input for the active session. Safe from any thread. Returns 0, or -1
 * for a NULL handle or pad.
 */
int32_t opennow_gfn_key(OpennowGfn *gfn, uint16_t virtual_key, uint16_t modifiers, uint8_t pressed);
int32_t opennow_gfn_mouse_move(OpennowGfn *gfn, int16_t delta_x, int16_t delta_y);
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
