#!/usr/bin/env node
// Reads a frame trace written by opennow_streamer_protocol::frame_trace and
// prints where every frame's time went: network arrival, FEC and NACK repair,
// assembly, the decoder queue, VideoToolbox, and the render thread.
//
// Usage: node tools/frame-trace/report.mjs [trace.csv]
// With no argument it reads the newest trace in the default macOS folder.
// Writes <trace>.report.txt, <trace>.timeline.csv (one row per second) and
// <trace>.frames.csv (one row per frame) next to the trace.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';

const DEFAULT_DIR = path.join(
  os.homedir(),
  'Library/Application Support/OpenNOW/diagnostics/frame-traces',
);
const DEADLINES = [1, 2, 3, 4, 5];
const RTP_CLOCK = 90000;
const RTP_WRAP = 2 ** 32;
const LOSS_HORIZON = 4096;
const SILENCE_US = 50000;
const STALL_CONTEXT_US = 500000;

function newestTrace() {
  const files = fs
    .readdirSync(DEFAULT_DIR)
    .filter((name) => /^frame-trace-\d+\.csv$/.test(name))
    .map((name) => path.join(DEFAULT_DIR, name))
    .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  if (files.length === 0) throw new Error(`no traces in ${DEFAULT_DIR}`);
  return files[0];
}

function pct(sorted, q) {
  if (sorted.length === 0) return NaN;
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1));
  return sorted[i];
}

function summary(values) {
  const sorted = Float64Array.from(values).sort();
  return {
    n: sorted.length,
    p50: pct(sorted, 0.5),
    p95: pct(sorted, 0.95),
    p99: pct(sorted, 0.99),
    max: sorted.length ? sorted[sorted.length - 1] : NaN,
  };
}

const fmt = (value, digits = 1) => (Number.isFinite(value) ? value.toFixed(digits) : '-');
const fmtSummary = (s, unit = 'ms', digits = 1) =>
  `p50 ${fmt(s.p50, digits)} / p95 ${fmt(s.p95, digits)} / p99 ${fmt(s.p99, digits)} / max ${fmt(s.max, digits)} ${unit} (n=${s.n})`;

class RtpUnwrapper {
  constructor() {
    this.last = null;
    this.cycles = 0;
  }
  unwrap(ts) {
    if (this.last !== null) {
      if (ts < this.last && this.last - ts > RTP_WRAP / 2) this.cycles += 1;
      else if (ts > this.last && ts - this.last > RTP_WRAP / 2) return ts + (this.cycles - 1) * RTP_WRAP;
    }
    this.last = ts;
    return ts + this.cycles * RTP_WRAP;
  }
}

// Counts RTP indices never seen. A gap stays pending until it is LOSS_HORIZON
// packets behind the newest, so a late or retransmitted packet still fills it.
class LossCounter {
  constructor() {
    this.highest = null;
    this.pending = new Set();
    this.lost = 0;
    this.duplicates = 0;
    this.expected = 0;
  }
  observe(index) {
    if (this.highest === null) {
      this.highest = index;
      this.expected = 1;
      return;
    }
    if (index > this.highest) {
      for (let missing = this.highest + 1; missing < index; missing += 1) this.pending.add(missing);
      this.expected += index - this.highest;
      this.highest = index;
      if (this.pending.size > LOSS_HORIZON) this.settle(this.highest - LOSS_HORIZON);
    } else if (!this.pending.delete(index)) {
      this.duplicates += 1;
    }
  }
  settle(below) {
    for (const missing of this.pending) {
      if (missing < below) {
        this.pending.delete(missing);
        this.lost += 1;
      }
    }
  }
  finish() {
    this.lost += this.pending.size;
    this.pending.clear();
  }
}

function second(perSecond, t) {
  const key = Math.floor(t / 1e6);
  let row = perSecond.get(key);
  if (!row) {
    row = {
      bytes: 0, dataBytes: 0, fecBytes: 0, packets: 0, fecPackets: 0, retx: 0, reordered: 0,
      fecRecovered: 0, nackRequested: 0, nackSent: 0, keyframeRequests: 0, assembled: 0,
      presented: 0, drops: 0, ping: [], estKbps: [], lossPer10k: [],
    };
    perSecond.set(key, row);
  }
  return row;
}

async function main() {
  const tracePath = process.argv[2] ?? newestTrace();
  const input = readline.createInterface({ input: fs.createReadStream(tracePath), crlfDelay: Infinity });

  let unixAtOrigin = 0;
  let label = '';
  let firstT = Infinity;
  let lastT = 0;
  const sdp = { describe: [], announce: [] };
  const frames = new Map();
  const rtpToFrame = new Map();
  const perSecond = new Map();
  const loss = new LossCounter();
  const fecPercents = new Map();
  const events = [];
  const queueDepthAtPush = [];
  const queueOutcomes = new Map();
  const decodeStatus = new Map();
  const qos = { ping: [], est: [], queueDelay: [], jitter: [] };
  const silences = [];
  const presented = [];
  let lastPacketT = null;
  let decodedReplaced = 0;
  let traceDropped = 0;
  const counts = {
    packets: 0, dataPackets: 0, fecPackets: 0, bytes: 0, dataBytes: 0, fecBytes: 0, retx: 0,
    reordered: 0, fecRecovered: 0, nackRequested: 0, nackSent: 0, nackSendFailed: 0,
    keyframeRequests: 0, assembled: 0, keyframes: 0, desync: 0,
  };

  const frameOf = (index) => {
    let frame = frames.get(index);
    if (!frame) {
      frame = { first: Infinity, last: 0, data: 0, fec: 0, retx: 0, recovered: 0 };
      frames.set(index, frame);
    }
    return frame;
  };

  for await (const line of input) {
    const f = line.split(',');
    const type = f[0];
    const t = Number(f[1]);
    if (type === 'H') {
      unixAtOrigin = Number(f[2]);
      label = f.slice(4).join(',');
      continue;
    }
    if (type === 'SDP') {
      (sdp[f[2]] ?? (sdp[f[2]] = [])).push(f.slice(3).join(','));
      continue;
    }
    if (Number.isFinite(t)) {
      firstT = Math.min(firstT, t);
      lastT = Math.max(lastT, t);
    }
    switch (type) {
      case 'P': {
        const index = Number(f[2]);
        const percent = Number(f[8]);
        const isFec = percent !== 0 && Number(f[9]) >= Number(f[10]);
        const len = Number(f[11]);
        const retx = f[12] === '1';
        const reordered = f[13] === '1';
        const row = second(perSecond, t);
        counts.packets += 1;
        counts.bytes += len;
        row.packets += 1;
        row.bytes += len;
        if (isFec) {
          counts.fecPackets += 1;
          counts.fecBytes += len;
          row.fecPackets += 1;
          row.fecBytes += len;
        } else {
          counts.dataPackets += 1;
          counts.dataBytes += len;
          row.dataBytes += len;
        }
        if (retx) {
          counts.retx += 1;
          row.retx += 1;
        }
        if (reordered) {
          counts.reordered += 1;
          row.reordered += 1;
        }
        fecPercents.set(percent, (fecPercents.get(percent) ?? 0) + 1);
        loss.observe(index);
        const frame = frameOf(Number(f[3]));
        frame.first = Math.min(frame.first, t);
        frame.last = Math.max(frame.last, t);
        if (isFec) frame.fec += 1;
        else frame.data += 1;
        if (retx) frame.retx += 1;
        if (lastPacketT !== null && t - lastPacketT > SILENCE_US) silences.push({ t: lastPacketT, gapMs: (t - lastPacketT) / 1000 });
        lastPacketT = t;
        break;
      }
      case 'R':
        frameOf(Number(f[3])).recovered += 1;
        break;
      case 'FR':
        counts.fecRecovered += Number(f[2]);
        second(perSecond, t).fecRecovered += Number(f[2]);
        break;
      case 'F': {
        const index = Number(f[2]);
        const frame = frameOf(index);
        frame.assembled = t;
        frame.rtp = Number(f[3]);
        frame.key = f[4] === '1';
        frame.bytes = Number(f[6]);
        rtpToFrame.set(frame.rtp, index);
        counts.assembled += 1;
        if (frame.key) counts.keyframes += 1;
        second(perSecond, t).assembled += 1;
        break;
      }
      case 'NQ': {
        const asked = Number(f[3]) - Number(f[2]) + 1;
        counts.nackRequested += asked;
        second(perSecond, t).nackRequested += asked;
        break;
      }
      case 'NS':
        if (f[4] === '1') {
          counts.nackSent += 1;
          second(perSecond, t).nackSent += 1;
        } else counts.nackSendFailed += 1;
        events.push({ t, what: `NACK ${f[4] === '1' ? 'sent' : 'NOT sent'} for ${Number(f[3]) - Number(f[2]) + 1} packet(s)` });
        break;
      case 'K':
        counts.keyframeRequests += 1;
        second(perSecond, t).keyframeRequests += 1;
        events.push({ t, what: 'keyframe requested' });
        break;
      case 'Q': {
        const ping = Number(f[12]);
        const row = second(perSecond, t);
        if (ping >= 0) {
          qos.ping.push(ping);
          row.ping.push(ping);
        }
        qos.est.push(Number(f[5]));
        qos.queueDelay.push(Number(f[7]) / 1000);
        qos.jitter.push(Number(f[8]) / 1000);
        row.estKbps.push(Number(f[5]));
        row.lossPer10k.push(Number(f[4]));
        break;
      }
      case 'X':
      case 'G':
        second(perSecond, t).drops += 1;
        events.push({ t, what: `${type === 'G' ? 'gave up waiting for packets' : 'drop'}: ${f.slice(2).join(',')}` });
        break;
      case 'VQ': {
        const outcome = f[6];
        queueOutcomes.set(outcome, (queueOutcomes.get(outcome) ?? 0) + 1);
        queueDepthAtPush.push(Number(f[7]));
        const index = Number(f[2]);
        if (index >= 0) frameOf(index).queued = t;
        if (outcome !== 'queued') events.push({ t, what: `decoder queue ${outcome}, ${f[8]} frame(s) dropped` });
        break;
      }
      case 'VO': {
        const index = Number(f[2]);
        if (index >= 0) frameOf(index).popped = t;
        break;
      }
      case 'VS': {
        const index = Number(f[2]);
        if (index >= 0) frameOf(index).accepted = t;
        break;
      }
      case 'VX':
        counts.desync += 1;
        events.push({ t, what: `decoder desync: ${f.slice(2).join(',')}` });
        break;
      case 'VD': {
        const status = Number(f[4]);
        decodeStatus.set(status, (decodeStatus.get(status) ?? 0) + 1);
        const timescale = Number(f[3]);
        const rtp = timescale === RTP_CLOCK ? Number(f[2]) : Math.round((Number(f[2]) * RTP_CLOCK) / timescale);
        const index = rtpToFrame.get(rtp % RTP_WRAP);
        if (index !== undefined) frameOf(index).decoded = t;
        break;
      }
      case 'VR':
        decodedReplaced += 1;
        break;
      case 'VP': {
        const rtp = Math.round((Number(f[2]) / 1e9) * RTP_CLOCK) % RTP_WRAP;
        const index =
          rtpToFrame.get(rtp) ??
          rtpToFrame.get((rtp + 1) % RTP_WRAP) ??
          rtpToFrame.get((rtp + RTP_WRAP - 1) % RTP_WRAP);
        if (index !== undefined) frameOf(index).presented = t;
        presented.push(t);
        second(perSecond, t).presented += 1;
        break;
      }
      case 'W':
        traceDropped += Number(f[2]);
        break;
      default:
        break;
    }
  }
  loss.finish();

  const durationSec = (lastT - firstT) / 1e6;
  const wall = (t) =>
    new Date((unixAtOrigin + t) / 1000).toLocaleTimeString('en-US', {
      hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3,
    });
  const out = [];
  const say = (text = '') => out.push(text);

  say(`Trace ${path.basename(tracePath)}${label ? ` (${label})` : ''}`);
  say(`Started ${new Date((unixAtOrigin + firstT) / 1000).toLocaleString()}, ${fmt(durationSec, 1)} s of data.`);
  if (traceDropped > 0) say(`WARNING: the trace queue dropped ${traceDropped} lines; the numbers below undercount.`);

  const interesting = /bitrate|fec|nack|fps|framerate|resolution|width|height|packetsize|keyframe|vbv|codec|dynamicstreaming|pacing|djb|jitter|latency|bwe|drc|repair/i;
  for (const direction of ['describe', 'announce']) {
    const lines = (sdp[direction] ?? []).filter((l) => interesting.test(l));
    if (lines.length === 0) continue;
    say();
    say(direction === 'describe' ? 'What the server offered (DESCRIBE)' : 'What the client asked for (ANNOUNCE)');
    for (const l of lines) say(`  ${l}`);
  }

  const frameIndices = [...frames.keys()].filter((index) => frames.get(index).first !== Infinity).sort((a, b) => a - b);
  const assembled = frameIndices.map((index) => frames.get(index)).filter((frame) => frame.assembled !== undefined);
  const unwrapper = new RtpUnwrapper();
  for (const frame of assembled) frame.rtpUnwrapped = unwrapper.unwrap(frame.rtp);
  const rtpDeltas = [];
  for (let i = 1; i < assembled.length; i += 1) {
    const delta = assembled[i].rtpUnwrapped - assembled[i - 1].rtpUnwrapped;
    if (delta > 0) rtpDeltas.push(delta);
  }
  const intervalUs = (summary(rtpDeltas).p50 / RTP_CLOCK) * 1e6;

  say();
  say('Network');
  say(`  ${counts.packets} video packets: ${counts.dataPackets} data, ${counts.fecPackets} FEC (${fmt((100 * counts.fecPackets) / Math.max(1, counts.dataPackets))}% of data).`);
  say(`  Received ${fmt((counts.bytes * 8) / durationSec / 1e6, 2)} Mbps on average: ${fmt((counts.dataBytes * 8) / durationSec / 1e6, 2)} data + ${fmt((counts.fecBytes * 8) / durationSec / 1e6, 2)} FEC.`);
  say(`  FEC percent the server stamped on packets: ${[...fecPercents.entries()].sort((a, b) => b[1] - a[1]).map(([p, n]) => `${p}% x${n}`).join(', ')}`);
  say(`  Lost on the wire (RTP index never seen): ${loss.lost} of ${loss.expected} (${fmt((100 * loss.lost) / Math.max(1, loss.expected), 3)}%). Out of order: ${counts.reordered}. Duplicates: ${loss.duplicates}.`);
  say(`  Repaired by FEC: ${counts.fecRecovered} packets. NACK: ${counts.nackRequested} packets asked for, ${counts.nackSent} requests sent (${counts.nackSendFailed} could not be sent), ${counts.retx} retransmissions arrived and were used.`);
  say(`  Keyframe requests sent: ${counts.keyframeRequests}. Keyframes received: ${counts.keyframes}.`);
  say(`  Ping ${fmtSummary(summary(qos.ping))}`);
  say(`  Bandwidth estimate the client reported upstream: ${fmtSummary(summary(qos.est.map((k) => k / 1000)), 'Mbps', 2)}`);
  say(`  Queue delay the client reported: ${fmtSummary(summary(qos.queueDelay))}`);
  say(`  Jitter the client reported: ${fmtSummary(summary(qos.jitter))}`);
  say(`  Silences over 50 ms with no video packet at all: ${silences.length}${silences.length ? `, longest ${fmt(Math.max(...silences.map((s) => s.gapMs)), 0)} ms` : ''}.`);

  let anchor = Infinity;
  for (const frame of assembled) {
    frame.slot = ((frame.rtpUnwrapped - assembled[0].rtpUnwrapped) / RTP_CLOCK) * 1e6;
    anchor = Math.min(anchor, frame.first - frame.slot);
  }
  const deadlineOf = (frame) => frame.slot + anchor;
  const firstIndex = frameIndices[0];
  const lastIndex = frameIndices[frameIndices.length - 1];
  const expectedFrames = frameIndices.length ? lastIndex - firstIndex + 1 : 0;

  const bucket = (stage) => {
    const within = DEADLINES.map(() => 0);
    let later = 0;
    let missing = 0;
    for (let index = firstIndex; index <= lastIndex; index += 1) {
      const frame = frames.get(index);
      const t = frame?.[stage];
      if (frame === undefined || t === undefined || frame.slot === undefined) {
        missing += 1;
        continue;
      }
      const lateness = t - deadlineOf(frame);
      DEADLINES.forEach((k, i) => {
        if (lateness <= k * intervalUs) within[i] += 1;
      });
      if (lateness > DEADLINES[DEADLINES.length - 1] * intervalUs) later += 1;
    }
    return { within, later, missing };
  };

  say();
  say(`Frames against their deadlines (${fmt(1e6 / intervalUs, 2)} fps, one frame = ${fmt(intervalUs / 1000, 2)} ms)`);
  say('  A frame\'s slot is its RTP capture time, anchored at the earliest any frame\'s first packet arrived');
  say('  relative to its slot. "Within 1" means it would have made a 1-frame playout buffer. Encoder time');
  say('  variation on the server counts against the frame, the same as network delay.');
  say(`  Frame numbers ${firstIndex}..${lastIndex}: ${expectedFrames} sent, ${assembled.length} assembled, ${expectedFrames - assembled.length} never assembled.`);
  for (const [stage, name] of [
    ['assembled', 'assembled from packets'],
    ['accepted', 'accepted by the decoder'],
    ['decoded', 'decoded by VideoToolbox'],
    ['presented', 'picked for display'],
  ]) {
    const b = bucket(stage);
    const cells = DEADLINES.map((k, i) => `${k}: ${fmt((100 * b.within[i]) / expectedFrames, 2)}%`).join('  ');
    say(`  ${name.padEnd(24)} within ${cells}  later: ${fmt((100 * b.later) / expectedFrames, 2)}%  never: ${fmt((100 * b.missing) / expectedFrames, 2)}%`);
  }

  say(`  First to last packet of a frame: ${fmtSummary(summary(assembled.map((f) => (f.last - f.first) / 1000)))}`);
  say(`  First packet to assembled:       ${fmtSummary(summary(assembled.map((f) => (f.assembled - f.first) / 1000)))}`);
  say(`  Assembled after its slot:        ${fmtSummary(summary(assembled.map((f) => (f.assembled - deadlineOf(f)) / 1000)))}`);
  const keyframes = assembled.filter((f) => f.key);
  if (keyframes.length) {
    say(`  Keyframes: ${keyframes.length}, size ${fmtSummary(summary(keyframes.map((k) => k.bytes / 1000)), 'KB', 0)}`);
    say(`  Keyframes assembled after slot: ${fmtSummary(summary(keyframes.map((k) => (k.assembled - deadlineOf(k)) / 1000)))}`);
  }
  say(`  Other frames' size: ${fmtSummary(summary(assembled.filter((f) => !f.key).map((p) => p.bytes / 1000)), 'KB', 1)}`);

  const stageGap = (from, to) =>
    summary(assembled.filter((f) => f[from] !== undefined && f[to] !== undefined).map((f) => (f[to] - f[from]) / 1000));
  say();
  say('Buffering');
  say(`  Frames waiting in the decoder queue when a new one arrives: ${fmtSummary(summary(queueDepthAtPush), 'frames', 0)}`);
  say(`  What the decoder queue did with arriving frames: ${[...queueOutcomes.entries()].map(([o, n]) => `${o} ${n}`).join(', ')}`);
  say(`  Assembled -> queued:             ${fmtSummary(stageGap('assembled', 'queued'))}`);
  say(`  Queued -> taken by decoder:      ${fmtSummary(stageGap('queued', 'popped'))}`);
  say(`  Taken -> accepted by VT:         ${fmtSummary(stageGap('popped', 'accepted'))}`);
  say(`  Accepted -> decoded (VT time):   ${fmtSummary(stageGap('accepted', 'decoded'))}`);
  say(`  Decoded -> picked for display:   ${fmtSummary(stageGap('decoded', 'presented'))}`);
  say(`  Assembled -> picked for display: ${fmtSummary(stageGap('assembled', 'presented'))}`);
  say(`  Decoded frames overwritten before display: ${decodedReplaced}. Decoder desyncs: ${counts.desync}. Decode status: ${[...decodeStatus.entries()].map(([s, n]) => `${s}: ${n}`).join(', ')}`);

  const gaps = [];
  for (let i = 1; i < presented.length; i += 1) {
    const gapMs = (presented[i] - presented[i - 1]) / 1000;
    if (gapMs > (2 * intervalUs) / 1000) gaps.push({ t: presented[i - 1], end: presented[i], gapMs });
  }
  const over = (ms) => gaps.filter((g) => g.gapMs > ms).length;
  events.sort((a, b) => a.t - b.t);
  say();
  say('Stalls (gaps between frames picked for display)');
  say(`  ${presented.length} frames displayed, ${fmt(presented.length / durationSec, 2)} per second on average.`);
  say(`  Gaps over 2 frames: ${gaps.length}. Over 67 ms: ${over(67)}. Over 250 ms: ${over(250)}. Over 1 s: ${over(1000)}.`);
  const worst = [...gaps].sort((a, b) => b.gapMs - a.gapMs).slice(0, 15).sort((a, b) => a.t - b.t);
  for (const gap of worst) {
    const silence = silences.find((s) => s.t >= gap.t - STALL_CONTEXT_US && s.t <= gap.end);
    say(`  ${wall(gap.t)}  ${fmt(gap.gapMs, 0)} ms without a new frame.${silence ? ` No video packets at all for ${fmt(silence.gapMs, 0)} ms of it.` : ''}`);
    for (const e of events.filter((e) => e.t >= gap.t - STALL_CONTEXT_US && e.t <= gap.end).slice(0, 8)) {
      say(`      ${wall(e.t)} ${e.what}`);
    }
  }

  const report = out.join('\n');
  console.log(report);
  fs.writeFileSync(`${tracePath}.report.txt`, `${report}\n`);

  const mean = (values) => (values.length ? values.reduce((a, b) => a + b, 0) / values.length : NaN);
  const timeline = [
    'second,wall,rx_mbps,data_mbps,fec_mbps,packets,fec_packets,retx,reordered,fec_recovered,nack_requested,nack_sent,keyframe_requests,frames_assembled,frames_displayed,drops,ping_ms,est_mbps,loss_pct',
  ];
  for (const s of [...perSecond.keys()].sort((a, b) => a - b)) {
    const row = perSecond.get(s);
    timeline.push([
      s, wall(s * 1e6), fmt((row.bytes * 8) / 1e6, 3), fmt((row.dataBytes * 8) / 1e6, 3), fmt((row.fecBytes * 8) / 1e6, 3),
      row.packets, row.fecPackets, row.retx, row.reordered, row.fecRecovered, row.nackRequested, row.nackSent,
      row.keyframeRequests, row.assembled, row.presented, row.drops, fmt(mean(row.ping), 2),
      fmt(mean(row.estKbps) / 1000, 2), fmt(mean(row.lossPer10k) / 100, 3),
    ].join(','));
  }
  fs.writeFileSync(`${tracePath}.timeline.csv`, `${timeline.join('\n')}\n`);

  const frameRows = [
    'frame,key,bytes,data_packets,fec_packets,retransmits,fec_recovered,first_packet_ms,assembled_late_ms,accepted_late_ms,decoded_late_ms,displayed_late_ms',
  ];
  for (let index = firstIndex; index <= lastIndex; index += 1) {
    const frame = frames.get(index);
    if (!frame || frame.slot === undefined) {
      frameRows.push(`${index},,,${frame?.data ?? 0},${frame?.fec ?? 0},${frame?.retx ?? 0},${frame?.recovered ?? 0},,,,,`);
      continue;
    }
    const late = (t) => (t === undefined ? '' : fmt((t - deadlineOf(frame)) / 1000, 3));
    frameRows.push([
      index, frame.key ? 1 : 0, frame.bytes, frame.data, frame.fec, frame.retx, frame.recovered,
      fmt(frame.first / 1000, 3), late(frame.assembled), late(frame.accepted), late(frame.decoded), late(frame.presented),
    ].join(','));
  }
  fs.writeFileSync(`${tracePath}.frames.csv`, `${frameRows.join('\n')}\n`);
  console.log(`\nAlso wrote ${path.basename(tracePath)}.report.txt, .timeline.csv and .frames.csv next to the trace.`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
