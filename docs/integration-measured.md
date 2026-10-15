# Library cost, measured

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
