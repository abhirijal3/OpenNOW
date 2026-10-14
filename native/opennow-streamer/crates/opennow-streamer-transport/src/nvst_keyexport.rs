//! Session-scoped DTLS-SRTP receive-key export through str0m's public provider API.
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock, Weak}, time::Instant};
use str0m::crypto::{CryptoError, CryptoProvider, dtls::*};
use aes::cipher::{KeyIvInit, StreamCipher};
type Registry = HashMap<Vec<u8>, Weak<ReceiveKeys>>;
fn registry() -> &'static Mutex<Registry> { static R: OnceLock<Mutex<Registry>> = OnceLock::new(); R.get_or_init(Mutex::default) }
#[derive(Clone)]
pub struct AudioKeys { pub key: [u8;32], pub salt: [u8;12], pub profile: u8 }
impl std::fmt::Debug for AudioKeys {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str("AudioKeys(redacted)")}}
impl Drop for AudioKeys {fn drop(&mut self){for b in self.key.iter_mut().chain(self.salt.iter_mut()) {unsafe { std::ptr::write_volatile(b,0); }}}}
#[derive(Debug,Default)]
pub struct ReceiveKeys(Mutex<Option<AudioKeys>>);
impl ReceiveKeys {pub fn snapshot(&self)->Option<AudioKeys>{self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()}}
pub fn for_fingerprint(bytes:&[u8])->Option<Arc<ReceiveKeys>>{registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(bytes)?.upgrade()}
/// RFC 5764 server-write material when local DTLS role is active; RFC 3711 KDR=0.
pub fn receive_keys(material:&[u8],profile:SrtpProfile,active:bool)->Option<AudioKeys>{
 let size=match profile{SrtpProfile::AEAD_AES_256_GCM=>32,SrtpProfile::AEAD_AES_128_GCM=>16,_=>return None};
 if material.len()!=size*2+24{return None}
 let side=usize::from(active);let key=&material[side*size..(side+1)*size];
 let salt=&material[size*2+side*12..size*2+(side+1)*12];
 let mut out=AudioKeys{key:[0;32],salt:[0;12],profile:u8::from(size==16)};
 for label in [0,2]{let mut iv=[0;16];iv[..12].copy_from_slice(salt);iv[7]^=label;
  let target:&mut[u8]=if label==0{&mut out.key[..size]}else{&mut out.salt};
  if size==32{ctr::Ctr128BE::<aes::Aes256>::new_from_slices(key,&iv).ok()?.apply_keystream(target)}
  else{ctr::Ctr128BE::<aes::Aes128>::new_from_slices(key,&iv).ok()?.apply_keystream(target)}
 }Some(out)
}
struct ExportProvider;
impl std::fmt::Debug for ExportProvider {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("ExportProvider")}}
fn base()->&'static CryptoProvider{static P:OnceLock<CryptoProvider>=OnceLock::new();P.get_or_init(str0m::crypto::from_feature_flags)}
static EXPORT:ExportProvider=ExportProvider;
pub fn provider()->Arc<CryptoProvider>{let mut p=base().clone();p.dtls_provider=&EXPORT;Arc::new(p)}
impl DtlsProvider for ExportProvider{
 fn generate_certificate(&self)->Option<DtlsCert>{base().dtls_provider.generate_certificate()}
 fn new_dtls(&self,cert:&DtlsCert,now:Instant,version:DtlsVersion,mtu:Option<usize>)->Result<Box<dyn DtlsInstance>,CryptoError>{
  let inner=base().dtls_provider.new_dtls(cert,now,version,mtu)?;let keys=Arc::new(ReceiveKeys::default());
  let fingerprint=base().sha256_provider.sha256(&cert.certificate).to_vec();
  let mut r=registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner);r.retain(|_,v|v.strong_count()>0);r.insert(fingerprint,Arc::downgrade(&keys));
  Ok(Box::new(ExportInstance{inner,keys}))
 }
}
struct ExportInstance{inner:Box<dyn DtlsInstance>,keys:Arc<ReceiveKeys>}
impl std::fmt::Debug for ExportInstance{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("ExportInstance")}}
impl DtlsInstance for ExportInstance{
 fn set_active(&mut self,a:bool){self.inner.set_active(a)}
 fn handle_packet(&mut self,p:&[u8])->Result<(),DtlsImplError>{self.inner.handle_packet(p)}
 fn poll_output<'a>(&mut self,b:&'a mut[u8])->DtlsOutput<'a>{let out=self.inner.poll_output(b);
  if let DtlsOutput::KeyingMaterial(ref material,profile)=out{*self.keys.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=receive_keys(material,profile,self.inner.is_active());}out
 }
 fn handle_timeout(&mut self,n:Instant)->Result<(),DtlsImplError>{self.inner.handle_timeout(n)}
 fn send_application_data(&mut self,d:&[u8])->Result<(),DtlsImplError>{self.inner.send_application_data(d)}
 fn is_active(&self)->bool{self.inner.is_active()}
 fn protocol_version(&self)->Option<ProtocolVersion>{self.inner.protocol_version()}
 fn is_closing(&self)->bool{self.inner.is_closing()}
 fn is_closed(&self)->bool{self.inner.is_closed()}
 fn close(&mut self)->Result<(),DtlsImplError>{self.inner.close()}
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn receive_direction_and_bounds(){let m:Vec<u8>=(0..88).collect();let server=receive_keys(&m,SrtpProfile::AEAD_AES_256_GCM,true).unwrap();let client=receive_keys(&m,SrtpProfile::AEAD_AES_256_GCM,false).unwrap();assert_ne!(server.key,client.key);assert_eq!(server.profile,0);assert!(receive_keys(&m[..87],SrtpProfile::AEAD_AES_256_GCM,true).is_none());assert!(receive_keys(&m,SrtpProfile::AES128_CM_SHA1_80,true).is_none());let narrow=receive_keys(&m[..56],SrtpProfile::AEAD_AES_128_GCM,true).unwrap();assert_eq!(narrow.profile,1);assert_eq!(narrow.key[16..],[0;16]);}
}
