# Library cost, measured

November 5, 2026 scope correction: the October 8 low-bitrate/menu measurements below are historical. The prediction that Linux would use under 1% is not a current capacity estimate. The November 4 OVH 20-Mbps live profile measured the main OpenNOW receive threads at 2.78% + 1.18% = 3.96% of one core, versus 4.57% in the forwarding/server loop and 9.03% total process CPU. Thread attribution includes kernel work and cross-library calls.

The same saved live v2pop memory snapshot has RSS 24976 KiB (24.4 MiB), PSS 19756 KiB (19.3 MiB), and nine threads. This is the whole native process, including the embedded OpenNOW streaming library, protocol code, stacks and resident buffers. It excludes the separate opennow-core session-management helper and kernel socket buffers. No current v2pop/core was running when checked on November 5; no new GFN session was started. Therefore complete per-session physical memory has not yet been measured. The 1.3-MB heap below must not be used as either current whole-process or whole-session memory. Raw evidence: /Users/abhirijal/.local/share/rasbari/private-exports/pop-cpu-2026-11-04/deep-132605/memory.txt and sibling moonlight-qt/docs/live-profile-2026-11-04.md.

Measured 2026-10-08 against GFN India (L40S in Mumbai) from an M1 Mac. Later builds are not re-measured here.

| | Library CPU (one core) | Notes |
|---|---|---|
| Start of the day | about 8% | 1 ms bundle polling, 250 us event polling, 40 NAT pings/s |
| Wakeup fixes | 6.4% | |
| Header-only mode | 1.5% | bundle 0.93%, video 0.43%, events 0.03% |

Most of the remaining 1.5% is macOS's unconnected `sendto` (about 95 us a call); the same call on Linux
measured 1.4 to 3.9 us, so Linux should land well under 1% per session. Live heap is about 1.3 MB per
session. GFN accepted it: every frame acked (4,106 in 70 s at 60 fps), QoS reports at about 19 a second.
The stream itself on a menu screen: 180 to 400 video packets a second, 1.3 to 3.8 Mbps, 40% of video
packets GFN parity, 200 Opus packets a second.
