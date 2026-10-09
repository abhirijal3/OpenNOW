#!/usr/bin/env python3
import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
CORE = os.path.join(REPO, "native", "opennow-core", "target", "release", "opennow-core")
COLLECTOR = os.path.join(HERE, "build", "collector")
READY_STATUSES = (2, 3)


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


class Core:
    def __init__(self, data_dir):
        self.process = subprocess.Popen(
            [CORE, "--data-dir", data_dir],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.next_id = 0
        self.replies = {}
        self.events = queue.Queue()
        self.lock = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.process.stdout:
            message = json.loads(line)
            if message.get("type") == "response":
                with self.lock:
                    self.replies[message["id"]] = message
                    self.lock.notify_all()
            else:
                self.events.put(message)

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def call(self, method, params=None, timeout=180):
        self.next_id += 1
        request_id = str(self.next_id)
        self.send({"type": "request", "id": request_id, "method": method, "params": params or {}})
        deadline = time.time() + timeout
        with self.lock:
            while request_id not in self.replies:
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise RuntimeError(f"{method} timed out")
                self.lock.wait(remaining)
            reply = self.replies.pop(request_id)
        if not reply.get("ok"):
            raise RuntimeError(f"{method} failed: {json.dumps(reply.get('error'))}")
        return request_id, reply["result"]

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=10)


def sign_in(core, provider_code):
    _, auth = core.call("auth.session.get")
    if auth.get("session"):
        return auth
    _, providers = core.call("auth.providers.list")
    provider = next(
        (p for p in providers["providers"] if p["code"].lower() == provider_code.lower()),
        None,
    )
    if provider is None:
        raise RuntimeError(f"no provider {provider_code}")
    _, attempt = core.call("auth.device.start", {"providerIdpId": provider["idpId"]})
    log(f"SIGN IN: open {attempt.get('verificationUriComplete') or attempt['verificationUri']}")
    log(f"SIGN IN: code {attempt['userCode']}")
    while True:
        _, poll = core.call("auth.device.poll", {"attemptId": attempt["attemptId"]})
        if poll["status"] == "authorized":
            break
        if poll["status"] not in ("pending", "slow_down"):
            raise RuntimeError(f"sign-in ended: {json.dumps(poll)}")
        time.sleep(max(poll.get("retryAfterMs", 5000), 1000) / 1000)
    _, auth = core.call("auth.device.complete", {"attemptId": attempt["attemptId"], "staySignedIn": True})
    return auth


def pick_region(core, auth, region_match):
    _, regions = core.call("network.regions.list")
    names = [f"{r.get('name')} ({r.get('url')})" for r in regions.get("regions", [])]
    log("regions: " + "; ".join(names))
    match = [r for r in regions.get("regions", []) if region_match.lower() in json.dumps(r).lower()]
    if not match:
        raise RuntimeError(f"no region matches {region_match!r}")
    region = match[0]
    core.call(
        "settings.set",
        {"key": "region", "value": region["url"], "providerIdpId": auth["session"]["provider"]["idpId"]},
    )
    log(f"region: {region.get('name')} {region['url']}")


def book(core, auth, app_id, title):
    scope = auth.get("scope") or {
        "generation": auth["generation"],
        "userId": auth["session"]["user"]["userId"],
        "providerIdpId": auth["session"]["provider"]["idpId"],
    }
    request_id, created = core.call(
        "session.create",
        {"appId": app_id, "variantId": app_id, "scope": scope, "title": title},
    )
    core.send({"type": "ack", "id": request_id})
    session = created["session"]
    log(f"booked session {session.get('sessionId')} status {session.get('status')}")
    last = None
    while session.get("status") not in READY_STATUSES:
        state = (session.get("status"), session.get("queuePosition"), session.get("seatSetupStep"), session.get("adState"))
        if state != last:
            log(f"waiting: status {state[0]} queue {state[1]} setup {state[2]} ads {json.dumps(state[3])}")
            last = state
        if session.get("phase") == "finished":
            raise RuntimeError(f"session finished before it was ready: {json.dumps(session.get('termination'))}")
        time.sleep(2)
        _, polled = core.call("session.poll", session)
        session = polled.get("session") or polled
    log(f"seat ready on {session.get('serverIp')} ({session.get('gpuType')}, {session.get('zone')})")
    return session


def main():
    parser = argparse.ArgumentParser(description="Book a GFN seat and record its stream headlessly.")
    parser.add_argument("--app-id", default="9029111")
    parser.add_argument("--title", default="Dota 2")
    parser.add_argument("--provider", default="NVIDIA")
    parser.add_argument("--region", default="india")
    parser.add_argument("--seconds", type=int, default=120)
    parser.add_argument("--data-dir", default=os.path.expanduser("~/.local/share/opennow-collector"))
    parser.add_argument("--out", default=os.path.join(HERE, "runs", time.strftime("%Y%m%d-%H%M%S")))
    args = parser.parse_args()
    os.makedirs(args.data_dir, exist_ok=True)
    os.makedirs(args.out, exist_ok=True)
    core = Core(args.data_dir)
    session = None
    try:
        core.call("core.hello", {"protocolVersion": 6})
        auth = sign_in(core, args.provider)
        log(f"signed in as {auth['session']['user'].get('displayName') or auth['session']['user'].get('userId')}")
        pick_region(core, auth, args.region)
        session = book(core, auth, args.app_id, args.title)
        _, prepared = core.call("streamer.prepare", {"session": session})
        context_path = os.path.join(args.out, "context.json")
        with open(context_path, "w") as handle:
            json.dump(prepared["context"], handle)
        log(f"collecting for {args.seconds} s into {args.out}")
        subprocess.run([COLLECTOR, context_path, args.out, str(args.seconds)], check=False)
    finally:
        if session is not None:
            try:
                core.call("session.stop", {"sessionId": session["sessionId"]}, timeout=60)
                log("seat released")
            except Exception as error:
                log(f"could not release the seat: {error}")
        core.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        log(f"FAILED: {error}")
        sys.exit(1)
