//! Player receive/report parameters. Defaults and SDP overrides are distinguished.
use serde_json::Value;
pub const SIZE:usize=40;
pub fn player_profile(handoff:&Value,timings:bool)->Result<[u8;SIZE], &'static str>{
 let values=handoff.get("nativePlayerProfile");
 if values.is_some_and(|v|!v.is_object()){return Err("nativePlayerProfile")}
 let number=|name:&str,default:u32,min:u32,max:u32|->Result<u32,&'static str>{
  let n=match values.and_then(|v|v.get(name)){None=>default,Some(v)=>v.as_u64().and_then(|n|u32::try_from(n).ok()).ok_or("nativePlayerProfile")?};
  if n<min||n>max{Err("nativePlayerProfile")}else{Ok(n)}
 };
 let mut b=[0;SIZE];b[0]=1;b[1]=203|if timings{16}else{0};if values.is_some(){b[1]|=32}
 for (name,mask,default) in [("fec",1,1),("bll",2,1),("adaptiveBll",4,0),("nack",8,1),("videoQos",64,1),("audioQos",128,1)]{
  if number(name,default,0,1)?==0{b[1]&=!mask}else{b[1]|=mask}
 }
 b[2]=number("bllThreshold",98,0,100)? as u8;b[3]=number("rtdHistory",5,5,5)? as u8;
 for (name,offset,default,min,max,multiplier) in [("videoMs",4,50,1,1000,1000),("audioMs",8,70,1,1000,1000),
  ("cpmMilli",24,0,0,64000,1),("decodeFactorMilli",28,2000,1000,16000,1),("bweMilli",32,0,0,64000,1),("focusMs",36,5000,0,60000,1)]{
  let n=number(name,default,min,max)?;if matches!(name,"cpmMilli"|"bweMilli")&&n!=0&&n<1000{return Err("nativePlayerProfile")}
  b[offset..offset+4].copy_from_slice(&(n*multiplier).to_be_bytes());
 }
 let wait=number("waitMs",52,4,84)?;let hysteresis=number("hysteresisMs",4,0,83)?;if hysteresis>=wait{return Err("nativePlayerProfile")}
 for (name,offset,default,min,max) in [("waitMs",12,52,4,84),("offWaitMs",14,8,1,84),("dynamicWaitMs",16,68,4,84),
  ("hysteresisMs",18,4,0,83),("pollMs",20,4,1,52),("graceMs",22,1,0,wait)]{
  b[offset..offset+2].copy_from_slice(&(number(name,default,min,max)? as u16).to_be_bytes());
 }Ok(b)
}
#[cfg(test)]mod tests{use super::*;use serde_json::json;
 #[test]fn defaults_and_explicit_overrides(){let b=player_profile(&json!({}),false).unwrap();assert_eq!(&b[..4],&[1,203,98,5]);assert_eq!(&b[12..14],&[0,52]);assert_eq!(u32::from_be_bytes(b[4..8].try_into().unwrap()),50000);
 let b=player_profile(&json!({"nativePlayerProfile":{"waitMs":68,"videoMs":100,"adaptiveBll":1}}),true).unwrap();assert_eq!(b[1],255);assert_eq!(&b[12..14],&[0,68]);assert_eq!(u32::from_be_bytes(b[4..8].try_into().unwrap()),100000);}
 #[test]fn invalid_profile_cannot_silently_change_reports(){for v in [json!({"rtdHistory":6}),json!({"videoMs":0}),json!({"pollMs":53}),json!({"cpmMilli":1}),json!({"waitMs":4,"hysteresisMs":4})]{assert!(player_profile(&json!({"nativePlayerProfile":v}),false).is_err());}}
}
