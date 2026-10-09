#include "opennow_gfn.h"

#include <errno.h>
#include <inttypes.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define STATS_INTERVAL_US 1000000ull
#define INPUT_INTERVAL_US 1000000ull
#define ANTI_AFK_INTERVAL_US 60000000ull
#define PATH_BYTES 1024
#define STOP_REASON_BYTES 256
#define FRAME_SLOTS 4096u
#define GS_HEADER_END 32u
#define GS_FRAME_OFFSET 20u
#define GS_FLAGS_OFFSET 24u
#define GS_FEC_OFFSET 28u
#define GS_LAST_PACKET 3u
#define GS_ONLY_PACKET 6u

typedef struct {
    FILE *video;
    FILE *audio;
    FILE *events;
    pthread_mutex_t events_lock;
    atomic_uint_fast64_t video_packets;
    atomic_uint_fast64_t video_bytes;
    atomic_uint_fast64_t access_units;
    atomic_uint_fast64_t keyframes;
    atomic_uint_fast64_t audio_packets;
    atomic_uint_fast64_t audio_bytes;
    atomic_uint_fast64_t events_seen;
    atomic_uint_fast64_t parity_packets;
    atomic_uint_fast64_t sequence_gaps;
    atomic_uint_fast64_t latency_sum_us;
    atomic_uint_fast64_t latency_max_us;
    atomic_uint_fast64_t latency_count;
    atomic_uint_fast64_t frame_done_us[FRAME_SLOTS];
    atomic_uint_fast32_t frame_done_id[FRAME_SLOTS];
    uint32_t next_sequence;
    int have_sequence;
    atomic_int connected;
    atomic_int ended;
    char stop_reason[STOP_REASON_BYTES];
} collector_t;

static volatile sig_atomic_t interrupted = 0;

static void on_signal(int sig) {
    (void)sig;
    interrupted = 1;
}

static uint64_t now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000ull + (uint64_t)ts.tv_nsec / 1000ull;
}

static void write_record(FILE *file, uint64_t stamp, const uint8_t *data, size_t len) {
    const uint32_t length = (uint32_t)len;
    if (fwrite(&stamp, sizeof(stamp), 1, file) != 1 || fwrite(&length, sizeof(length), 1, file) != 1 ||
        fwrite(data, 1, len, file) != len) {
        fprintf(stderr, "collector: write failed: %s\n", strerror(errno));
    }
}

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

static void on_video_packet(void *user, const uint8_t *data, size_t len) {
    collector_t *c = user;
    const uint64_t arrived = now_us();
    atomic_fetch_add(&c->video_packets, 1);
    atomic_fetch_add(&c->video_bytes, len);
    if (len >= GS_HEADER_END) {
        const uint32_t sequence = (uint32_t)data[2] << 8 | data[3];
        const uint32_t ahead = (sequence - c->next_sequence) & 0xffffu;
        if (!c->have_sequence || ahead < 0x8000u) {
            if (c->have_sequence) {
                atomic_fetch_add(&c->sequence_gaps, ahead);
            }
            c->next_sequence = (sequence + 1u) & 0xffffu;
            c->have_sequence = 1;
        }
        const uint32_t frame = le32(data + GS_FRAME_OFFSET);
        const uint32_t flags = le32(data + GS_FLAGS_OFFSET) & 0xfu;
        const uint32_t fec = le32(data + GS_FEC_OFFSET);
        const uint32_t index = (fec >> 12) & 0x3ffu;
        const uint32_t sources = (fec >> 22) & 0x3ffu;
        if (sources > 0 && index >= sources) {
            atomic_fetch_add(&c->parity_packets, 1);
        } else if (flags == GS_LAST_PACKET || flags == GS_ONLY_PACKET) {
            atomic_store(&c->frame_done_us[frame % FRAME_SLOTS], arrived);
            atomic_store(&c->frame_done_id[frame % FRAME_SLOTS], frame);
        }
    }
    write_record(c->video, arrived, data, len);
}

static void record_latency(collector_t *c, uint32_t frame) {
    const uint64_t done = atomic_load(&c->frame_done_us[frame % FRAME_SLOTS]);
    if (done == 0 || atomic_load(&c->frame_done_id[frame % FRAME_SLOTS]) != frame) {
        return;
    }
    const uint64_t now = now_us();
    const uint64_t latency = now > done ? now - done : 0;
    atomic_fetch_add(&c->latency_sum_us, latency);
    atomic_fetch_add(&c->latency_count, 1);
    uint64_t seen = atomic_load(&c->latency_max_us);
    while (latency > seen && !atomic_compare_exchange_weak(&c->latency_max_us, &seen, latency)) {
    }
}

static void on_media(void *user, const OpennowGfnMedia *media) {
    collector_t *c = user;
    if (media->kind == OPENNOW_GFN_MEDIA_AUDIO) {
        atomic_fetch_add(&c->audio_packets, 1);
        atomic_fetch_add(&c->audio_bytes, media->len);
        write_record(c->audio, media->rtp_timestamp, media->data, media->len);
        return;
    }
    atomic_fetch_add(&c->access_units, 1);
    if (media->has_frame_index) {
        record_latency(c, media->frame_index);
    }
    if (media->keyframe) {
        atomic_fetch_add(&c->keyframes, 1);
    }
}

static int contains(const uint8_t *data, size_t len, const char *needle) {
    const size_t n = strlen(needle);
    for (size_t i = 0; n <= len && i <= len - n; i++) {
        if (memcmp(data + i, needle, n) == 0) {
            return 1;
        }
    }
    return 0;
}

static void on_event(void *user, const uint8_t *data, size_t len) {
    collector_t *c = user;
    atomic_fetch_add(&c->events_seen, 1);
    pthread_mutex_lock(&c->events_lock);
    fwrite(data, 1, len, c->events);
    fputc('\n', c->events);
    fflush(c->events);
    if (contains(data, len, "\"termination\"") && !atomic_load(&c->ended)) {
        const size_t n = len < STOP_REASON_BYTES - 1 ? len : STOP_REASON_BYTES - 1;
        memcpy(c->stop_reason, data, n);
        c->stop_reason[n] = '\0';
        atomic_store(&c->ended, 1);
    }
    pthread_mutex_unlock(&c->events_lock);
    if (contains(data, len, "\"type\":\"start\"") && contains(data, len, "\"ok\"")) {
        atomic_store(&c->connected, 1);
    }
}

static char *read_file(const char *path) {
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        return NULL;
    }
    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return NULL;
    }
    const long size = ftell(file);
    rewind(file);
    char *text = size >= 0 ? malloc((size_t)size + 1u) : NULL;
    if (text == NULL || fread(text, 1, (size_t)size, file) != (size_t)size) {
        free(text);
        fclose(file);
        return NULL;
    }
    text[size] = '\0';
    fclose(file);
    return text;
}

static int command(OpennowGfn *gfn, const char *json) {
    return opennow_gfn_command(gfn, (const uint8_t *)json, strlen(json));
}

static FILE *open_output(const char *dir, const char *name, const char *mode) {
    char path[PATH_BYTES];
    if (snprintf(path, sizeof(path), "%s/%s", dir, name) >= (int)sizeof(path)) {
        return NULL;
    }
    return fopen(path, mode);
}

typedef struct {
    uint64_t bytes;
    uint64_t packets;
    uint64_t access_units;
    uint64_t audio;
    uint64_t cpu_us;
    uint64_t at_us;
} snapshot_t;

static uint64_t cpu_time_us(long *max_rss) {
    struct rusage usage;
    if (getrusage(RUSAGE_SELF, &usage) != 0) {
        return 0;
    }
    *max_rss = usage.ru_maxrss;
    return (uint64_t)(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1000000ull +
           (uint64_t)(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec);
}

static void print_stats(collector_t *c, uint64_t elapsed_us, snapshot_t *last) {
    long max_rss = 0;
    snapshot_t now = {
        .bytes = atomic_load(&c->video_bytes),
        .packets = atomic_load(&c->video_packets),
        .access_units = atomic_load(&c->access_units),
        .audio = atomic_load(&c->audio_packets),
        .cpu_us = cpu_time_us(&max_rss),
        .at_us = now_us(),
    };
    const double seconds = last->at_us > 0 ? (double)(now.at_us - last->at_us) / 1e6 : 1.0;
    const uint64_t count = atomic_exchange(&c->latency_count, 0);
    const uint64_t sum = atomic_exchange(&c->latency_sum_us, 0);
    const uint64_t max = atomic_exchange(&c->latency_max_us, 0);
    const double cpu_percent = seconds > 0 ? (double)(now.cpu_us - last->cpu_us) / (seconds * 1e4) : 0.0;
    const double packets = (double)(now.packets - last->packets);
    printf("t=%5" PRIu64 "s video %4.0f pkt/s %6.2f Mbps %3.0f fps | audio %3.0f pkt/s | parity %" PRIu64
           " gaps %" PRIu64 " keyframes %" PRIu64 " | lib %5.2f%% cpu %5.2f us/pkt rss %ld | assemble avg %.0f max %" PRIu64
           " us\n",
           elapsed_us / 1000000ull, packets / seconds, (double)(now.bytes - last->bytes) * 8.0 / 1e6 / seconds,
           (double)(now.access_units - last->access_units) / seconds, (double)(now.audio - last->audio) / seconds,
           (uint64_t)atomic_load(&c->parity_packets), (uint64_t)atomic_load(&c->sequence_gaps),
           (uint64_t)atomic_load(&c->keyframes), cpu_percent,
           packets > 0 ? (double)(now.cpu_us - last->cpu_us) / packets : 0.0, max_rss,
           count > 0 ? (double)sum / (double)count : 0.0, max);
    fflush(stdout);
    *last = now;
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: %s <context.json> <output-dir> <seconds>\n", argv[0]);
        return 2;
    }
    const char *out_dir = argv[2];
    const uint64_t run_us = strtoull(argv[3], NULL, 10) * 1000000ull;
    char *context = read_file(argv[1]);
    if (context == NULL) {
        fprintf(stderr, "collector: cannot read %s\n", argv[1]);
        return 1;
    }
    mkdir(out_dir, 0755);
    collector_t c;
    memset(&c, 0, sizeof(c));
    pthread_mutex_init(&c.events_lock, NULL);
    c.video = open_output(out_dir, "video.bin", "wb");
    c.audio = open_output(out_dir, "audio.bin", "wb");
    c.events = open_output(out_dir, "events.jsonl", "w");
    if (c.video == NULL || c.audio == NULL || c.events == NULL) {
        fprintf(stderr, "collector: cannot open outputs in %s\n", out_dir);
        return 1;
    }
    signal(SIGINT, on_signal);
    signal(SIGTERM, on_signal);

    OpennowGfnCallbacks callbacks = {
        .user = &c,
        .on_video_packet = on_video_packet,
        .on_media = on_media,
        .on_event = on_event,
    };
    OpennowGfn *gfn = opennow_gfn_create(&callbacks);
    if (gfn == NULL || command(gfn, "{\"id\":\"hello\",\"type\":\"hello\",\"protocolVersion\":7}") != 0) {
        fprintf(stderr, "collector: library would not start\n");
        return 1;
    }
    const size_t start_len = strlen(context) + 64u;
    char *start = malloc(start_len);
    if (start == NULL) {
        return 1;
    }
    snprintf(start, start_len, "{\"id\":\"start\",\"type\":\"start\",\"context\":%s}", context);
    free(context);
    printf("collector: starting stream\n");
    fflush(stdout);
    const uint64_t began = now_us();
    if (command(gfn, start) != 0) {
        fprintf(stderr, "collector: start command was rejected\n");
    }
    free(start);
    printf("collector: start returned after %.1f s, connected=%d\n", (double)(now_us() - began) / 1e6,
           atomic_load(&c.connected));
    fflush(stdout);

    uint64_t next_stats = now_us() + STATS_INTERVAL_US;
    uint64_t next_input = now_us() + INPUT_INTERVAL_US;
    uint64_t next_afk = now_us() + ANTI_AFK_INTERVAL_US;
    snapshot_t last;
    memset(&last, 0, sizeof(last));
    int16_t nudge = 1;
    while (!interrupted && !atomic_load(&c.ended) && (run_us == 0 || now_us() - began < run_us)) {
        const uint64_t now = now_us();
        if (now >= next_input) {
            opennow_gfn_mouse_move(gfn, nudge, 0);
            nudge = (int16_t)-nudge;
            next_input = now + INPUT_INTERVAL_US;
        }
        if (now >= next_afk) {
            command(gfn, "{\"id\":\"afk\",\"type\":\"anti-afk-pulse\"}");
            next_afk = now + ANTI_AFK_INTERVAL_US;
        }
        if (now >= next_stats) {
            print_stats(&c, now - began, &last);
            next_stats = now + STATS_INTERVAL_US;
        }
        const struct timespec tick = {0, 50L * 1000L * 1000L};
        nanosleep(&tick, NULL);
    }
    if (atomic_load(&c.ended)) {
        printf("collector: stream ended: %s\n", c.stop_reason);
    }
    command(gfn, "{\"id\":\"stop\",\"type\":\"stop\",\"reason\":\"collector-finished\"}");
    opennow_gfn_destroy(gfn);
    print_stats(&c, now_us() - began, &last);
    fclose(c.video);
    fclose(c.audio);
    fclose(c.events);
    pthread_mutex_destroy(&c.events_lock);
    return 0;
}
