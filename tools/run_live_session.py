#!/usr/bin/env python3
import argparse
import json
import os
import signal
import secrets
import subprocess
import sys
import threading
import time

READY_STATUSES = (2, 3)

def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)

class CoreClient:
    def __init__(self, core_path, data_dir):
        self.proc = subprocess.Popen(
            [core_path, "--data-dir", data_dir],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1
        )
        self.replies = {}
        self.lock = threading.Condition()
        self.next_id = 0
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        for line in self.proc.stdout:
            try:
                msg = json.loads(line)
                if msg.get("type") == "response":
                    with self.lock:
                        self.replies[msg["id"]] = msg
                        self.lock.notify_all()
            except Exception:
                pass

    def send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params=None, timeout=120):
        self.next_id += 1
        req_id = str(self.next_id)
        self.send({"type": "request", "id": req_id, "method": method, "params": params or {}})
        deadline = time.time() + timeout
        with self.lock:
            while req_id not in self.replies:
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise RuntimeError(f"{method} timed out")
                self.lock.wait(remaining)
            reply = self.replies.pop(req_id)
        if not reply.get("ok"):
            raise RuntimeError(f"{method} failed: {json.dumps(reply.get('error'))}")
        return req_id, reply.get("result")

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)

def session_duration_active(seconds, elapsed):
    return seconds == 0 or elapsed < seconds

def session_ready(session):
    return session.get("status") in READY_STATUSES and not session.get("resumePending", False)

def acquire_session(core, resume_id, app_id, title, scope):
    if resume_id:
        _, discovered = core.call("session.remote.list")
        if not any(s.get("sessionId") == resume_id for s in discovered.get("sessions", [])):
            raise RuntimeError("requested existing GFN seat was not discovered; refusing a new allocation")
        _, claimed = core.call("session.claim", {"sessionId": resume_id})
        return claimed["session"]
    request_id, created = core.call("session.create", {
        "appId": app_id, "variantId": app_id, "scope": scope, "title": title
    })
    core.send({"type": "ack", "id": request_id})
    return created["session"]

def main():
    parser = argparse.ArgumentParser(description="Live GFN Session + v2pop Relay Orchestrator")
    parser.add_argument("--app-id", default="9029111", help="Default: 9029111 (Dota 2)")
    parser.add_argument("--title", default="Dota 2")
    parser.add_argument("--region", default="virginia")
    parser.add_argument("--resume-session-id", help="Claim this exact existing seat without allocating another")
    parser.add_argument("--port", type=int, default=29878)
    parser.add_argument("--seconds", type=int, default=0, help="Optional test duration; 0 runs until stopped")
    parser.add_argument("--log-dir", default="/home/ubuntu/protocol-live-20261011/logs")
    parser.add_argument("--core-bin", default="/home/ubuntu/opennow-core")
    parser.add_argument("--v2pop-bin", default="/home/ubuntu/v2test/v2/bin/v2pop")
    parser.add_argument("--data-dir", default="/home/ubuntu/.local/share/opennow-collector")
    parser.add_argument("--relay-key", default=secrets.token_hex(32))
    parser.add_argument("--relay-key-file", help="Read the control key from a private file")
    parser.add_argument("--gfn-key", default=secrets.token_hex(32))
    parser.add_argument("--gfn-salt", default="000000000000000000000001")
    parser.add_argument("--codec", default="h264", help="Video codec (h264 or h265)")
    parser.add_argument("--width", type=int, default=1920, help="Stream width")
    parser.add_argument("--height", type=int, default=1080, help="Stream height")
    parser.add_argument("--bitrate-mbps", type=int, default=75, help="Max bitrate in Mbps")
    args = parser.parse_args()
    if args.relay_key_file:
        with open(args.relay_key_file) as f:
            args.relay_key = f.read().strip()
    for name, key in (("relay", args.relay_key), ("GFN", args.gfn_key)):
        if len(key) != 64 or any(c not in "0123456789abcdefABCDEF" for c in key):
            parser.error(f"{name} key must be 64 hex characters")

    os.makedirs(args.log_dir, mode=0o700, exist_ok=True)
    audio_key = secrets.token_hex(32)
    exit_code = 0
    core = CoreClient(args.core_bin, args.data_dir)
    session = None
    pop_proc = None

    def cleanup(sig=None, frame=None):
        nonlocal pop_proc, session, exit_code
        log("shutting down...")
        if pop_proc is not None:
            log("terminating v2pop...")
            pop_proc.terminate()
            try:
                pop_proc.wait(timeout=5)
            except Exception:
                pop_proc.kill()
                pop_proc.wait(timeout=5)
        if session is not None:
            try:
                log(f"releasing GFN seat {session.get('sessionId')}...")
                core.call("session.stop", {"sessionId": session["sessionId"]}, timeout=30)
                log("GFN seat successfully released.")
            except Exception as e:
                log(f"could not stop session: {e}")
        core.close()
        sys.exit(exit_code)

    signal.signal(signal.SIGINT, cleanup)
    signal.signal(signal.SIGTERM, cleanup)

    try:
        log("starting core handshake...")
        core.call("core.hello", {"protocolVersion": 6})
        _, auth = core.call("auth.session.get")
        if not auth.get("session"):
            raise RuntimeError("not signed in to NVIDIA")
        user = auth["session"]["user"].get("displayName") or auth["session"]["user"].get("userId")
        provider_idp = auth["session"]["provider"]["idpId"]
        log(f"authenticated as {user}")

        # Pick Virginia region
        _, regions = core.call("network.regions.list")
        target_reg = None
        for r in regions.get("regions", []):
            if args.region.lower() in r.get("name", "").lower() or args.region.lower() in r.get("url", "").lower():
                target_reg = r
                break
        if not target_reg:
            raise RuntimeError(f"could not find region matching {args.region}")
        log(f"selected region: {target_reg['name']} ({target_reg['url']})")
        core.call("settings.set", {"key": "region", "value": target_reg["url"], "providerIdpId": provider_idp})

        # Configure codec, resolution, and bitrate
        codec_val = "h265" if args.codec.lower() in ("h265", "hevc") else "h264"
        core.call("settings.set", {"key": "codec", "value": codec_val, "providerIdpId": provider_idp})
        log(f"configured codec: {codec_val}")
        if args.width and args.height:
            res_val = f"{args.width}x{args.height}"
            core.call("settings.set", {"key": "resolution", "value": res_val, "providerIdpId": provider_idp})
            log(f"configured resolution: {res_val}")
        if args.bitrate_mbps:
            core.call("settings.set", {"key": "maxBitrateMbps", "value": args.bitrate_mbps, "providerIdpId": provider_idp})
            log(f"configured max bitrate: {args.bitrate_mbps} Mbps")

        # Book GFN session
        scope = auth.get("scope") or {
            "generation": auth["generation"],
            "userId": auth["session"]["user"]["userId"],
            "providerIdpId": provider_idp,
        }
        log(f"{'claiming existing' if args.resume_session_id else 'booking'} session for {args.title} (appId: {args.app_id})...")
        session = acquire_session(core, args.resume_session_id, args.app_id, args.title, scope)
        log(f"seat booked: sessionId={session.get('sessionId')} status={session.get('status')}")

        while not session_ready(session):
            log(f"waiting for seat: status={session.get('status')} queue={session.get('queuePosition')} step={session.get('seatSetupStep')}")
            if session.get("phase") == "finished":
                raise RuntimeError(f"session finished prematurely: {session.get('termination')}")
            time.sleep(2)
            _, polled = core.call("session.poll", session)
            session = polled.get("session") or polled

        log(f"GFN seat ready on server: {session.get('serverIp')} ({session.get('gpuType')}, zone: {session.get('zone')})")

        # Prepare streamer context
        log("preparing streamer session context...")
        _, prep = core.call("streamer.prepare", {"session": session})
        context = prep["context"]

        # Configure predetermined media keys
        if "settings" not in context:
            context["settings"] = {}
        context["settings"]["srtpAesKeyHex"] = args.gfn_key
        context["settings"]["srtpKeyId"] = 1

        context_file = os.path.join(args.log_dir, "context.json")
        os.umask(0o077)
        with open(context_file, "w") as f:
            json.dump(context, f, indent=2)
        log(f"wrote streamer context to {context_file}")

        # Launch v2pop relay server
        v2pop_cmd = [
            args.v2pop_bin,
            "--key", args.relay_key,
            "--gfn-key", args.gfn_key,
            "--gfn-salt", args.gfn_salt,
            "--ssrc", "1",
            "--gfn-audio-key", audio_key,
            "--gfn-audio-salt", args.gfn_salt,
            "--audio-ssrc", "2",
            "--port", str(args.port),
            "--codec", args.codec,
            "--context", context_file,
            "--sim", "0"
        ]
        log(f"launching v2pop: {args.v2pop_bin} (media/control keys redacted)")
        log_file = open(os.path.join(args.log_dir, "v2pop.log"), "w")
        pop_proc = subprocess.Popen(v2pop_cmd, stdout=log_file, stderr=subprocess.STDOUT)
        log(f"v2pop started with PID {pop_proc.pid}, listening on UDP port {args.port}")
        log(f"relay server ready for client connections at 40.160.38.161:{args.port}!")

        start_time = time.time()
        while session_duration_active(args.seconds, time.time() - start_time):
            ret = pop_proc.poll()
            if ret is not None:
                exit_code = 1
                log(f"v2pop exited prematurely with code {ret}")
                break
            time.sleep(1)

    except Exception as e:
        exit_code = 1
        log(f"ERROR: {e}")
    finally:
        cleanup()

if __name__ == "__main__":
    main()
