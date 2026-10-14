//! Bounded original-packet handoff, published only after str0m authenticates it.
const CAPACITY: usize = 32;
const MAX_PACKET: usize = 2048;
#[derive(Clone)]
struct Packet {used:bool,ssrc:u32,sequence:u16,len:usize,bytes:[u8;MAX_PACKET]}
pub struct RawAudioPackets {packets:[Packet;CAPACITY],next:usize}
impl Default for RawAudioPackets {
 fn default()->Self {Self{packets:std::array::from_fn(|_|Packet{used:false,ssrc:0,sequence:0,len:0,bytes:[0;MAX_PACKET]}),next:0}}
}
impl RawAudioPackets {
 pub fn stage(&mut self,bytes:&[u8])->bool {
  if !(12..=MAX_PACKET).contains(&bytes.len()) || bytes[0]>>6!=2{return false}
  let ssrc=u32::from_be_bytes(bytes[8..12].try_into().unwrap());let sequence=u16::from_be_bytes(bytes[2..4].try_into().unwrap());
  // An unauthenticated earlier copy must not replace the packet just admitted by RTC.
  let at=if let Some(at)=self.packets.iter().position(|p|p.used&&p.ssrc==ssrc&&p.sequence==sequence){at}
      else{let at=self.next;self.next=(self.next+1)%CAPACITY;at};
  let p=&mut self.packets[at];p.ssrc=ssrc;p.sequence=sequence;
  p.bytes[..bytes.len()].copy_from_slice(bytes);p.len=bytes.len();p.used=true;true
 }
 pub fn authenticated(&mut self,ssrc:u32,sequence:u16,mut deliver:impl FnMut(&[u8]))->bool {
  if let Some(p)=self.packets.iter_mut().find(|p|p.used&&p.ssrc==ssrc&&p.sequence==sequence) {p.used=false;deliver(&p.bytes[..p.len]);true}else{false}
 }
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn original_bytes_require_authenticated_identity(){let mut q=RawAudioPackets::default();let mut packet=[0;40];packet[0]=0x90;packet[3]=7;packet[11]=9;packet[39]=123;
 assert!(q.stage(&packet));assert!(!q.authenticated(9,8,|_|panic!()));assert!(q.authenticated(9,7,|p|assert_eq!(p,&packet)));assert!(!q.authenticated(9,7,|_|panic!()));assert!(!q.stage(&packet[..11]));}
 #[test]fn rejected_copy_cannot_shadow_later_authenticated_ciphertext(){
  let mut q=RawAudioPackets::default();let mut bad=[0;40];bad[0]=0x90;bad[3]=7;bad[11]=9;
  let mut good=bad;good[39]=123;
  assert!(q.stage(&bad));assert!(q.stage(&good));
  assert!(q.authenticated(9,7,|p|assert_eq!(p,&good)));
  assert!(!q.authenticated(9,7,|_|panic!("old candidate survived")));
 }
 #[test]fn packet_queue_is_bounded(){let mut q=RawAudioPackets::default();let mut packet=[0;40];packet[0]=0x90;for i in 0..33{packet[3]=i;q.stage(&packet);}assert!(!q.authenticated(0,0,|_|panic!()));assert!(q.authenticated(0,32,|p|assert_eq!(p[3],32)));}
}
