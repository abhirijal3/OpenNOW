//! Player-owned native feedback, queued for the active negotiated SCTP channels.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CAPACITY: usize = 64;
const MAX_COMMAND: usize = 768;
pub const PARTIAL_LIFETIME: Duration = Duration::from_millis(52);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRoute { Reliable, Partial }

pub fn native_route(bytes: &[u8]) -> Result<NativeRoute, &'static str> {
    if bytes.len() < 4 || bytes.len() > MAX_COMMAND {
        return Err("invalid native command size");
    }
    let code = u16::from_le_bytes([bytes[0], bytes[1]]);
    let len = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    if len != bytes.len() - 4 { return Err("truncated native command"); }
    match (code, len) {
        (0x0202, 124) if bytes[4..8] == [4,0,0,0] => Ok(NativeRoute::Reliable),
        (0x0207, 52) if bytes[4..8] == [7,0,0,0] => Ok(NativeRoute::Partial),
        (0x0203, 48..=528) if bytes[4..8] == [3,0,0,0] &&
            bytes[16..20] == [0,0,0,0] &&
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()) > 0 &&
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()) <= 16 &&
            len == 16 + 32 * u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize => Ok(NativeRoute::Partial),
        (0x0204, 102) if bytes[4..8] == [1,0,9,0] => Ok(NativeRoute::Partial),
        (0x020c, 16) => Ok(NativeRoute::Partial),
        (0x020b, 12) if bytes[4..6] == [0,0] && bytes[12] <= 1 && bytes[13..16] == [0,0,0] => Ok(NativeRoute::Reliable),
        (0x030f, 8) if bytes[4..8] == [1,0,0,0] => Ok(NativeRoute::Reliable),
        (0x0317, 13..=643) if bytes[4] == 2 && bytes[6] != 0 &&
            usize::from(bytes[6]) <= 64 && len == 3 + 10 * usize::from(bytes[6]) => Ok(NativeRoute::Partial),
        (0x0301, 24) | (0x0302, 2) | (0x0312, 16) => Ok(NativeRoute::Reliable),
        _ => Err("unsupported native player command"),
    }
}

#[derive(Debug)]
struct QueuedControl { bytes: Vec<u8>, route: NativeRoute, queued: Instant, lifetime: Duration }

#[derive(Debug, Default)]
pub struct PlayerFeedback {
    active: AtomicBool,
    generation: AtomicU64,
    queue: Mutex<VecDeque<QueuedControl>>,
    server_timing: Mutex<Option<[u8;12]>>,
}

impl PlayerFeedback {
    pub fn observe_timing(&self, bytes: &[u8]) -> bool {
        if bytes.len()!=12 || bytes[..8]!=[0x12,1,8,0,0,0,0,0] || bytes[8]>1 || bytes[9..]!=[0,0,0] {
            return false;
        }
        *self.server_timing.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=Some(bytes.try_into().unwrap());
        true
    }
    pub fn server_timing(&self) -> Option<[u8;12]> {
        *self.server_timing.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn active(&self) -> bool { self.active.load(Ordering::Acquire) }
    pub fn generation(&self) -> u64 { self.generation.load(Ordering::Acquire) }
    pub fn set_active(&self, active: bool) {
        let mut queue = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Each claim establishes a fresh player, even during a client takeover.
        if self.active.swap(active, Ordering::AcqRel) != active || active {
            queue.clear();
            self.generation.fetch_add(1, Ordering::Release);
        }
    }
    pub fn enqueue(&self, bytes: &[u8], now: Instant) -> Result<(), &'static str> {
        self.enqueue_deadline(bytes, now, PARTIAL_LIFETIME)
    }
    pub fn enqueue_deadline(&self, bytes: &[u8], now: Instant, lifetime: Duration) -> Result<(), &'static str> {
        if lifetime.is_zero() || lifetime > Duration::from_millis(84) { return Err("invalid feedback lifetime"); }
        let route = native_route(bytes)?;
        let mut queue = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.active() { return Err("player feedback is not active"); }
        // Periodic video QoS supersedes its older sample. Reliable audio preserves order.
        if bytes[..2] == [7, 2] {
            queue.retain(|q| q.bytes[..2] != [7, 2]);
        }
        if queue.len() >= CAPACITY { return Err("player feedback queue full"); }
        queue.push_back(QueuedControl {bytes: bytes.to_vec(), route, queued: now, lifetime});
        Ok(())
    }
    pub fn flush(&self, now: Instant, mut send: impl FnMut(NativeRoute, &[u8]) -> bool) -> usize {
        let mut queue = self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sent = 0;
        let mut i = 0;
        let mut tried = 0;
        while self.active() && i < queue.len() && tried < 8 {
            let item = &queue[i];
            if item.route == NativeRoute::Partial && now.saturating_duration_since(item.queued) >= item.lifetime {
                queue.remove(i); continue;
            }
            tried += 1;
            if send(item.route, &item.bytes) { queue.remove(i); sent += 1; }
            else { i += 1; }
        }
        sent
    }
}

/// Split aggregated native commands without modifying headers or versioned tails.
pub fn incoming_commands(bytes: &[u8], mut deliver: impl FnMut(&[u8])) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        if bytes.len() - at < 4 { return false; }
        let len = usize::from(u16::from_le_bytes([bytes[at+2], bytes[at+3]]));
        let end = at + 4 + len;
        if end > bytes.len() { return false; }
        let code = u16::from_le_bytes([bytes[at], bytes[at+1]]);
        if matches!(code, 0x0101 | 0x0102 | 0x0109 | 0x0112) { deliver(&bytes[at..end]); }
        at = end;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(code: u16, size: usize) -> Vec<u8> {
        let mut b = vec![0; size + 4];
        b[..2].copy_from_slice(&code.to_le_bytes());
        b[2..4].copy_from_slice(&(size as u16).to_le_bytes());
        if code == 0x202 { b[4] = 4; }
        if code == 0x207 { b[4] = 7; }
        if code == 0x204 { b[4] = 1; b[6] = 9; }
        if code == 0x30f { b[4] = 1; }
        b
    }
    #[test]
    fn nack_state_and_slow_decoder_request_go_reliable() {
        assert_eq!(native_route(&command(0x20b, 12)), Ok(NativeRoute::Reliable));
        let mut on = command(0x20b, 12); on[12] = 1;
        assert_eq!(native_route(&on), Ok(NativeRoute::Reliable));
        on[12] = 2; assert!(native_route(&on).is_err());
        on[12] = 1; on[4] = 1; assert!(native_route(&on).is_err());
        assert!(native_route(&command(0x20b, 11)).is_err());
        assert_eq!(native_route(&command(0x30f, 8)), Ok(NativeRoute::Reliable));
        let mut other = command(0x30f, 8); other[4] = 2;
        assert!(native_route(&other).is_err());
    }
    #[test]
    fn routes_are_roles_not_numeric_sctp_streams() {
        assert_eq!(native_route(&command(0x202, 124)), Ok(NativeRoute::Reliable));
        assert_eq!(native_route(&command(0x207, 52)), Ok(NativeRoute::Partial));
        assert!(native_route(&command(0xffff, 52)).is_err());
        assert!(native_route(&command(0x207, 51)).is_err());
        let mut ack=command(0x204,102);assert_eq!(native_route(&ack),Ok(NativeRoute::Partial));
        ack[6]=8;assert!(native_route(&ack).is_err());
        let mut truncated = command(0x207, 52); truncated.pop();
        assert!(native_route(&truncated).is_err());
    }
    #[test]
    fn ownership_and_queue_bound_are_explicit() {
        let f = PlayerFeedback::default(); let now = Instant::now();
        assert!(f.enqueue(&command(0x202, 124), now).is_err());
        f.set_active(true); assert_eq!(f.generation(), 1);
        for _ in 0..CAPACITY { assert!(f.enqueue(&command(0x202, 124), now).is_ok()); }
        assert!(f.enqueue(&command(0x202, 124), now).is_err());
        f.set_active(false); f.set_active(true);
        assert_eq!(f.flush(now, |_, _| true), 0);
    }
    #[test]
    fn player_takeover_discards_previous_feedback() {
        let f = PlayerFeedback::default(); let now = Instant::now();
        f.set_active(true); f.enqueue(&command(0x202,124),now).unwrap();
        f.set_active(true); assert_eq!(f.generation(),2);
        assert_eq!(f.flush(now,|_,_|true),0);
    }
    #[test]
    fn qos_supersedes_and_expired_partial_does_not_stall_reliable_audio() {
        let f = PlayerFeedback::default(); f.set_active(true); let now = Instant::now();
        for _ in 0..100 { f.enqueue(&command(0x207, 52), now).unwrap(); }
        assert_eq!(f.flush(now, |_, _| true), 1);
        f.enqueue(&command(0x202, 124), now).unwrap();
        f.enqueue(&command(0x207, 52), now).unwrap();
        assert_eq!(f.flush(now, |r, _| r == NativeRoute::Partial), 1);
        f.enqueue(&command(0x20c, 16), now).unwrap();
        assert_eq!(f.flush(now + PARTIAL_LIFETIME, |_, b| b[0] == 2), 1);
    }
    #[test]
    fn incoming_preserves_entire_commands_and_rejects_truncation() {
        let one = command(0x101, 20); let two = command(0x102, 4);
        let bytes = [one.clone(), two.clone()].concat(); let mut out = vec![];
        assert!(incoming_commands(&bytes, |b| out.push(b.to_vec())));
        assert_eq!(out, vec![one, two]);
        assert!(!incoming_commands(&bytes[..bytes.len()-1], |_| {}));
    }
    #[test]
    fn processing_v3_batch_is_bounded_and_uses_partial_reliability() {
        for count in 1..=16 {
            let mut b=command(0x203,16+32*count); b[4]=3; b[12]=count as u8;
            assert_eq!(native_route(&b),Ok(NativeRoute::Partial));
            b[16]=1;assert!(native_route(&b).is_err());b[16]=0;
            b[12]=0;assert!(native_route(&b).is_err());
        }
        let mut b=command(0x203,48);b[4]=3;b[12]=1;
        let f=PlayerFeedback::default(); f.set_active(true); let now=Instant::now();
        f.enqueue(&b,now).unwrap();
        assert_eq!(f.flush(now+PARTIAL_LIFETIME,|_,_|panic!("expired processing sample sent")),0);
    }
    #[test]
    fn timing_enable_and_disable_are_forwarded_without_reinterpreting_server_payloads() {
        let mut b=command(0x112,8);b[8]=1;
        let mut out=vec![];assert!(incoming_commands(&b,|v|out.push(v.to_vec())));assert_eq!(out,vec![b.clone()]);
        b[8]=0;out.clear();assert!(incoming_commands(&b,|v|out.push(v.to_vec())));assert_eq!(out,vec![b]);
    }
    #[test]
    fn server_timing_overrides_startup_and_survives_player_takeover() {
        let f=PlayerFeedback::default();assert!(f.server_timing().is_none());
        let enabled=[0x12,1,8,0,0,0,0,0,1,0,0,0];
        assert!(f.observe_timing(&enabled));assert_eq!(f.server_timing(),Some(enabled));
        let mut disabled=enabled;disabled[8]=0;assert!(f.observe_timing(&disabled));
        f.set_active(true);f.set_active(false);f.set_active(true);assert_eq!(f.server_timing(),Some(disabled));
        disabled[8]=2;assert!(!f.observe_timing(&disabled));assert!(!f.observe_timing(&disabled[..11]));
        disabled[8]=0;disabled[4]=1;assert!(!f.observe_timing(&disabled));
    }

}
