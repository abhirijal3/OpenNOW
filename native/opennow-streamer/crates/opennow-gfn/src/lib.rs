#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use opennow_streamer_core::{CapturedInput, CapturedInputQueue, EncodedMediaFrame, Engine};
use opennow_streamer_protocol::Command;
use serde_json::{Value, json};

const MEDIA_QUEUE_FRAMES: usize = 256;

pub const OPENNOW_GFN_MEDIA_VIDEO: u32 = 0;
pub const OPENNOW_GFN_MEDIA_AUDIO: u32 = 1;

#[repr(C)]
pub struct OpennowGfnMedia {
    pub kind: u32,
    pub data: *const u8,
    pub len: usize,
    pub codec: *const u8,
    pub codec_len: usize,
    pub frame_index: u32,
    pub has_frame_index: u8,
    pub keyframe: u8,
    pub rtp_timestamp: u64,
    pub clock_rate_hz: u32,
    pub received_at_us: u64,
}

#[repr(C)]
pub struct OpennowGfnGamepad {
    pub controller_id: u8,
    pub bitmap: u16,
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
}

pub type OpennowGfnBytesFn = Option<unsafe extern "C" fn(*mut c_void, *const u8, usize)>;
pub type OpennowGfnMediaFn = Option<unsafe extern "C" fn(*mut c_void, *const OpennowGfnMedia)>;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpennowGfnCallbacks {
    pub user: *mut c_void,
    pub on_video_packet: OpennowGfnBytesFn,
    pub on_media: OpennowGfnMediaFn,
    pub on_event: OpennowGfnBytesFn,
}

#[derive(Clone, Copy)]
struct Callbacks(OpennowGfnCallbacks);

unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

impl Callbacks {
    fn video_packet(&self, bytes: &[u8]) {
        if let Some(callback) = self.0.on_video_packet {
            unsafe { callback(self.0.user, bytes.as_ptr(), bytes.len()) };
        }
    }

    fn media(&self, frame: &EncodedMediaFrame) {
        let Some(callback) = self.0.on_media else {
            return;
        };
        let media = OpennowGfnMedia {
            kind: if frame.codec == "opus" {
                OPENNOW_GFN_MEDIA_AUDIO
            } else {
                OPENNOW_GFN_MEDIA_VIDEO
            },
            data: frame.payload.as_ptr(),
            len: frame.payload.len(),
            codec: frame.codec.as_ptr(),
            codec_len: frame.codec.len(),
            frame_index: frame.frame_index.unwrap_or(0),
            has_frame_index: u8::from(frame.frame_index.is_some()),
            keyframe: u8::from(frame.keyframe),
            rtp_timestamp: frame.rtp_timestamp,
            clock_rate_hz: frame.clock_rate_hz,
            received_at_us: frame.received_at_us,
        };
        unsafe { callback(self.0.user, &media) };
    }

    fn event(&self, value: &Value) {
        if let Some(callback) = self.0.on_event {
            let text = value.to_string();
            unsafe { callback(self.0.user, text.as_ptr(), text.len()) };
        }
    }
}

pub struct OpennowGfn {
    engine: Mutex<Option<Engine>>,
    input: Arc<CapturedInputQueue>,
    callbacks: Callbacks,
    workers: Vec<JoinHandle<()>>,
}

fn spawn_media_worker(frames: Receiver<EncodedMediaFrame>, callbacks: Callbacks) -> JoinHandle<()> {
    thread::Builder::new()
        .name("opennow-gfn-media".to_owned())
        .spawn(move || {
            for frame in frames {
                callbacks.media(&frame);
            }
        })
        .expect("spawn opennow-gfn-media")
}

fn spawn_event_worker(events: Receiver<Value>, callbacks: Callbacks) -> JoinHandle<()> {
    thread::Builder::new()
        .name("opennow-gfn-events".to_owned())
        .spawn(move || {
            for event in events {
                callbacks.event(&event);
            }
        })
        .expect("spawn opennow-gfn-events")
}

fn push_input(gfn: *mut OpennowGfn, input: CapturedInput) -> i32 {
    match unsafe { gfn.as_ref() } {
        Some(gfn) => {
            gfn.input.push(input);
            0
        }
        None => -1,
    }
}

/// @rule PLAYER-13: Clipboard text uses the existing bounded native Unicode queue; no host paste key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_text(
    gfn: *mut OpennowGfn, bytes: *const u8, len: usize,
) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    if bytes.is_null() || len == 0 || len > opennow_streamer_protocol::text_input::MAX_TEXT_BYTES {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    if gfn.input.submit_text(bytes).is_ok() { 0 } else { -1 }
}

/// @rule PLAYER-13: Reports input-channel readiness, not acknowledgement by a game's text box.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_text_available(gfn: *mut OpennowGfn) -> i32 {
    match unsafe { gfn.as_ref() } {
        Some(gfn) => i32::from(gfn.input.text_available()),
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_create(
    callbacks: *const OpennowGfnCallbacks,
) -> *mut OpennowGfn {
    let Some(callbacks) = (unsafe { callbacks.as_ref() }) else {
        return ptr::null_mut();
    };
    let callbacks = Callbacks(*callbacks);
    let (media_sender, media_receiver) = mpsc::sync_channel(MEDIA_QUEUE_FRAMES);
    let (event_sender, event_receiver) = mpsc::channel();
    let mut engine = Engine::with_media_consumer(event_sender, media_sender);
    if callbacks.0.on_video_packet.is_some() {
        let tap = callbacks;
        engine = engine.with_raw_video_tap(Arc::new(move |bytes: &[u8]| tap.video_packet(bytes)));
    }
    let input = engine.captured_input();
    let workers = vec![
        spawn_media_worker(media_receiver, callbacks),
        spawn_event_worker(event_receiver, callbacks),
    ];
    Box::into_raw(Box::new(OpennowGfn {
        engine: Mutex::new(Some(engine)),
        input,
        callbacks,
        workers,
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_command(
    gfn: *mut OpennowGfn,
    json: *const u8,
    len: usize,
) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else {
        return -1;
    };
    if json.is_null() {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(json, len) };
    let Ok(command) = serde_json::from_slice::<Command>(bytes) else {
        return -1;
    };
    let responses = {
        let mut engine = gfn
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match engine.as_mut() {
            Some(engine) => engine.handle(command).0,
            None => return -1,
        }
    };
    for response in &responses {
        gfn.callbacks.event(response);
    }
    0
}

pub type OpennowGfnAudioFn = Option<unsafe extern "C" fn(*mut c_void, *const u8, usize, *const u8, *const u8, u8)>;
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_set_audio_callback(gfn: *mut OpennowGfn,
    callback: OpennowGfnAudioFn, user: *mut c_void) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    let mut guard = gfn.engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(engine) = guard.as_mut() else { return -1; };
    let cookie = user as usize;
    engine.set_raw_audio_tap(callback.map(|cb| -> opennow_streamer_core::RawAudioTap {
        Arc::new(move |bytes, keys| unsafe { cb(cookie as *mut c_void, bytes.as_ptr(), bytes.len(), keys.key.as_ptr(), keys.salt.as_ptr(), keys.profile) })
    }));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_set_control_callback(
    gfn: *mut OpennowGfn, callback: OpennowGfnBytesFn, user: *mut c_void,
) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    let mut guard = gfn.engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(engine) = guard.as_mut() else { return -1; };
    let cookie = user as usize;
    engine.set_raw_control_tap(callback.map(|cb| -> opennow_streamer_core::RawPacketTap {
        Arc::new(move |bytes| unsafe { cb(cookie as *mut c_void, bytes.as_ptr(), bytes.len()) })
    }));
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_set_player_feedback(gfn: *mut OpennowGfn, enabled: u8) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    let guard = gfn.engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.as_ref().and_then(|e| e.set_player_feedback(enabled != 0).ok()) {
        Some(()) => 0, None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_send_control(gfn: *mut OpennowGfn, bytes: *const u8, len: usize) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    if bytes.is_null() || !(4..=768).contains(&len) { return -1; }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let Ok(guard) = gfn.engine.try_lock() else { return -1; };
    match guard.as_ref().and_then(|e| e.send_native_control(bytes).ok()) {
        Some(()) => 0, None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_player_profile(gfn:*mut OpennowGfn, out:*mut u8, cap:usize)->i32 {
    let Some(gfn) = (unsafe {gfn.as_ref()}) else {return -1};
    if out.is_null() || cap<40 {return -1}
    let Ok(guard) = gfn.engine.try_lock() else {return -1};
    let Some(profile) = guard.as_ref().and_then(|e|e.native_player_profile()) else {return -1};
    unsafe {std::ptr::copy_nonoverlapping(profile.as_ptr(),out,40)};40
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_player_timing(gfn:*mut OpennowGfn, out:*mut u8, cap:usize)->i32 {
    let Some(gfn) = (unsafe {gfn.as_ref()}) else {return -1};
    if out.is_null() || cap<12 {return -1}
    let Ok(guard) = gfn.engine.try_lock() else {return -1};
    let Some(bytes) = guard.as_ref().and_then(|e|e.native_player_timing()) else {return -1};
    unsafe {std::ptr::copy_nonoverlapping(bytes.as_ptr(),out,12)};12
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_upstream_rtt_us(gfn: *mut OpennowGfn) -> u32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return 0; };
    let Ok(guard) = gfn.engine.try_lock() else { return 0; };
    guard.as_ref().and_then(|e| e.upstream_rtt_us()).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_send_control_deadline(gfn: *mut OpennowGfn,
    bytes: *const u8, len: usize, lifetime_us: u32) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    if bytes.is_null() || !(4..=768).contains(&len) { return -1; }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let Ok(guard) = gfn.engine.try_lock() else { return -1; };
    match guard.as_ref().and_then(|e| e.send_native_control_deadline(bytes, lifetime_us).ok()) {
        Some(()) => 0, None => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_microphone_available(gfn: *mut OpennowGfn) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    let Ok(guard) = gfn.engine.try_lock() else { return -1; };
    i32::from(guard.as_ref().is_some_and(|engine| engine.microphone_available()))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_microphone_enable(gfn: *mut OpennowGfn, enabled: u8) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    let Ok(guard) = gfn.engine.try_lock() else { return -1; };
    match guard.as_ref().and_then(|engine| engine.set_microphone_enabled(enabled != 0).ok()) {
        Some(()) => 0, None => -1,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_microphone_opus(gfn: *mut OpennowGfn, opus: *const u8,
    len: usize, rtp_timestamp: u32) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    if opus.is_null() || !(1..=1275).contains(&len) { return -1; }
    let Ok(guard) = gfn.engine.try_lock() else { return -1; };
    let payload = unsafe { std::slice::from_raw_parts(opus, len) }.to_vec();
    match guard.as_ref().and_then(|engine| engine.send_microphone_opus(payload, rtp_timestamp).ok()) {
        Some(()) => 0, None => -1,
    }
}

fn push_input_at(gfn: *mut OpennowGfn, input: CapturedInput, timestamp_us: u64) -> i32 {
    let Some(gfn) = (unsafe { gfn.as_ref() }) else { return -1; };
    gfn.input.push_sample(opennow_streamer_core::CapturedInputSample {
        input, captured_at: std::time::Instant::now(), source_timestamp_us: Some(timestamp_us),
    });
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_key_at(gfn: *mut OpennowGfn, virtual_key: u16, modifiers: u16,
    pressed: u8, timestamp_us: u64) -> i32 {
    push_input_at(gfn, CapturedInput::Key { virtual_key, modifiers, pressed: pressed != 0 }, timestamp_us)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_move_at(gfn: *mut OpennowGfn, delta_x: i16, delta_y: i16,
    timestamp_us: u64) -> i32 {
    push_input_at(gfn, CapturedInput::MouseMove { delta_x, delta_y }, timestamp_us)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_abs_at(gfn: *mut OpennowGfn, x: u16, y: u16,
    width: u16, height: u16, timestamp_us: u64) -> i32 {
    push_input_at(gfn, CapturedInput::MouseAbsolute { x, y, width, height }, timestamp_us)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_button_at(gfn: *mut OpennowGfn, button: u8,
    pressed: u8, timestamp_us: u64) -> i32 {
    push_input_at(gfn, CapturedInput::MouseButton { button, pressed: pressed != 0 }, timestamp_us)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_wheel_at(gfn: *mut OpennowGfn, delta_x: i16,
    delta_y: i16, timestamp_us: u64) -> i32 {
    push_input_at(gfn, CapturedInput::MouseWheel { delta_x, delta_y }, timestamp_us)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_gamepad_at(gfn: *mut OpennowGfn, pad: *const OpennowGfnGamepad,
    timestamp_us: u64) -> i32 {
    let Some(pad) = (unsafe { pad.as_ref() }) else { return -1; };
    push_input_at(gfn, CapturedInput::Gamepad { controller_id: pad.controller_id, bitmap: pad.bitmap,
        buttons: pad.buttons, left_trigger: pad.left_trigger, right_trigger: pad.right_trigger,
        left_stick_x: pad.left_stick_x, left_stick_y: pad.left_stick_y,
        right_stick_x: pad.right_stick_x, right_stick_y: pad.right_stick_y }, timestamp_us)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_key(
    gfn: *mut OpennowGfn,
    virtual_key: u16,
    modifiers: u16,
    pressed: u8,
) -> i32 {
    push_input(
        gfn,
        CapturedInput::Key {
            virtual_key,
            modifiers,
            pressed: pressed != 0,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_move(
    gfn: *mut OpennowGfn,
    delta_x: i16,
    delta_y: i16,
) -> i32 {
    push_input(gfn, CapturedInput::MouseMove { delta_x, delta_y })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_abs(
    gfn: *mut OpennowGfn,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> i32 {
    push_input(
        gfn,
        CapturedInput::MouseAbsolute {
            x,
            y,
            width,
            height,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_button(
    gfn: *mut OpennowGfn,
    button: u8,
    pressed: u8,
) -> i32 {
    push_input(
        gfn,
        CapturedInput::MouseButton {
            button,
            pressed: pressed != 0,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_mouse_wheel(
    gfn: *mut OpennowGfn,
    delta_x: i16,
    delta_y: i16,
) -> i32 {
    push_input(gfn, CapturedInput::MouseWheel { delta_x, delta_y })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_gamepad(
    gfn: *mut OpennowGfn,
    pad: *const OpennowGfnGamepad,
) -> i32 {
    let Some(pad) = (unsafe { pad.as_ref() }) else {
        return -1;
    };
    push_input(
        gfn,
        CapturedInput::Gamepad {
            controller_id: pad.controller_id,
            bitmap: pad.bitmap,
            buttons: pad.buttons,
            left_trigger: pad.left_trigger,
            right_trigger: pad.right_trigger,
            left_stick_x: pad.left_stick_x,
            left_stick_y: pad.left_stick_y,
            right_stick_x: pad.right_stick_x,
            right_stick_y: pad.right_stick_y,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn opennow_gfn_destroy(gfn: *mut OpennowGfn) {
    if gfn.is_null() {
        return;
    }
    let gfn = unsafe { Box::from_raw(gfn) };
    let engine = gfn
        .engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(mut engine) = engine {
        if let Ok(command) =
            serde_json::from_value::<Command>(json!({"id":"destroy","type":"shutdown"}))
        {
            engine.handle(command);
        }
        drop(engine);
    }
    for worker in gfn.workers {
        let _ = worker.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct Seen {
        events: StdMutex<Vec<Value>>,
    }

    unsafe extern "C" fn record_event(user: *mut c_void, bytes: *const u8, len: usize) {
        let seen = unsafe { &*(user as *const Seen) };
        let text = unsafe { std::slice::from_raw_parts(bytes, len) };
        seen.events
            .lock()
            .unwrap()
            .push(serde_json::from_slice(text).unwrap());
    }

    fn callbacks(seen: &Seen) -> OpennowGfnCallbacks {
        OpennowGfnCallbacks {
            user: seen as *const Seen as *mut c_void,
            on_video_packet: None,
            on_media: None,
            on_event: Some(record_event),
        }
    }

    #[test]
    // @rule PLAYER-13
    fn clipboard_text_abi_checks_bounds_readiness_and_busy_without_partial_enqueue() {
        let seen = Seen::default();
        let gfn = unsafe { opennow_gfn_create(&callbacks(&seen)) };
        let input = unsafe { &*gfn }.input.clone();
        unsafe {
            assert_eq!(opennow_gfn_text_available(ptr::null_mut()), -1);
            assert_eq!(opennow_gfn_text_available(gfn), 0);
            assert_eq!(opennow_gfn_text(gfn, b"paste".as_ptr(), 5), -1);
            input.set_text_ready(1, true);
            assert_eq!(opennow_gfn_text_available(gfn), 1);
            assert_eq!(opennow_gfn_text(ptr::null_mut(), b"x".as_ptr(), 1), -1);
            assert_eq!(opennow_gfn_text(gfn, ptr::null(), 1), -1);
            for len in [0, 65537, usize::MAX] {
                assert_eq!(opennow_gfn_text(gfn, b"x".as_ptr(), len), -1);
            }
            for bytes in [&b"a\0b"[..], &[0xff][..]] {
                assert_eq!(opennow_gfn_text(gfn, bytes.as_ptr(), bytes.len()), -1);
            }
            assert!(input.take_sample().is_none());
            let bytes = "café 世界 🦫\n".as_bytes();
            assert_eq!(opennow_gfn_text(gfn, bytes.as_ptr(), bytes.len()), 0);
            assert_eq!(opennow_gfn_text(gfn, b"other".as_ptr(), 5), -1);
            for vk in [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c] {
                assert_eq!(input.take_sample().unwrap().input,
                    CapturedInput::Key { virtual_key: vk, modifiers: 0, pressed: false });
            }
            let CapturedInput::Text(text) = input.take_sample().unwrap().input else { panic!("text missing"); };
            assert_eq!(text.as_str().as_bytes(), bytes);
            assert!(input.take_sample().is_none());
            input.set_text_ready(2, false);
            assert!(text.is_cancelled());
            assert_eq!(opennow_gfn_text(gfn, bytes.as_ptr(), bytes.len()), -1);
            drop(text);
            input.set_text_ready(3, true);
            let max = vec![b'x'; 65536];
            assert_eq!(opennow_gfn_text(gfn, max.as_ptr(), max.len()), 0);
            input.set_text_ready(4, false);
            assert!(input.take_sample().is_some()); // harmless releases may remain
            input.clear();
            input.set_text_ready(5, true);
            for vk in 0..248 {
                input.push(CapturedInput::Key { virtual_key: vk, modifiers: 0, pressed: true });
            }
            assert_eq!(opennow_gfn_text(gfn, b"full".as_ptr(), 4), -1);
            for vk in 0..248 {
                assert_eq!(input.take_sample().unwrap().input,
                    CapturedInput::Key { virtual_key: vk, modifiers: 0, pressed: true });
            }
            assert!(input.take_sample().is_none());
            assert_eq!(opennow_gfn_text(gfn, b"retry".as_ptr(), 5), 0);
            opennow_gfn_destroy(gfn);
        }
    }

    #[test]
    fn player_timing_abi_rejects_missing_session_and_small_output_without_writes() {
        let seen=Seen::default();let gfn=unsafe {opennow_gfn_create(&callbacks(&seen))};assert!(!gfn.is_null());
        let mut out=[0xa5u8;12];
        unsafe {
            assert_eq!(opennow_gfn_player_timing(ptr::null_mut(),out.as_mut_ptr(),12),-1);
            assert_eq!(opennow_gfn_player_timing(gfn,ptr::null_mut(),12),-1);
            assert_eq!(opennow_gfn_player_timing(gfn,out.as_mut_ptr(),11),-1);
            assert_eq!(opennow_gfn_player_timing(gfn,out.as_mut_ptr(),12),-1);
            opennow_gfn_destroy(gfn);
        }
        assert_eq!(out,[0xa5u8;12]);
    }

    #[test]
    fn timed_input_abi_keeps_each_producer_timestamp_and_null_is_rejected() {
        let seen = Seen::default();
        let gfn = unsafe { opennow_gfn_create(&callbacks(&seen)) };
        assert!(!gfn.is_null());
        let pad = OpennowGfnGamepad { controller_id: 0, bitmap: 1, buttons: 0,
            left_trigger: 10, right_trigger: 20, left_stick_x: 1, left_stick_y: 2,
            right_stick_x: 3, right_stick_y: 4 };
        unsafe {
            assert_eq!(opennow_gfn_key_at(gfn, 0x41, 0, 1, 1001), 0);
            assert_eq!(opennow_gfn_mouse_move_at(gfn, 2, -3, 1002), 0);
            assert_eq!(opennow_gfn_mouse_abs_at(gfn, 10, 20, 100, 100, 1003), 0);
            assert_eq!(opennow_gfn_mouse_button_at(gfn, 1, 1, 1004), 0);
            assert_eq!(opennow_gfn_mouse_wheel_at(gfn, 0, 120, 1005), 0);
            assert_eq!(opennow_gfn_gamepad_at(gfn, &pad, 1006), 0);
            assert_eq!(opennow_gfn_key_at(ptr::null_mut(), 0x41, 0, 1, 1), -1);
            assert_eq!(opennow_gfn_mouse_move_at(ptr::null_mut(), 1, 1, 1), -1);
            assert_eq!(opennow_gfn_mouse_abs_at(ptr::null_mut(), 1, 1, 10, 10, 1), -1);
            assert_eq!(opennow_gfn_mouse_button_at(ptr::null_mut(), 1, 1, 1), -1);
            assert_eq!(opennow_gfn_mouse_wheel_at(ptr::null_mut(), 0, 1, 1), -1);
            assert_eq!(opennow_gfn_gamepad_at(gfn, ptr::null(), 1), -1);
        }
        let input = unsafe { &*gfn }.input.clone();
        for ts in 1001..=1006 { assert_eq!(input.take_sample().unwrap().source_timestamp_us, Some(ts)); }
        unsafe { opennow_gfn_destroy(gfn) };
    }

    #[test]
    fn hello_answers_through_the_event_callback() {
        let seen = Seen::default();
        let gfn = unsafe { opennow_gfn_create(&callbacks(&seen)) };
        assert!(!gfn.is_null());
        let hello = br#"{"id":"1","type":"hello","protocolVersion":7}"#;
        assert_eq!(
            unsafe { opennow_gfn_command(gfn, hello.as_ptr(), hello.len()) },
            0
        );
        unsafe { opennow_gfn_destroy(gfn) };
        let events = seen.events.lock().unwrap();
        let reply = events
            .iter()
            .find(|event| event["id"] == "1")
            .expect("hello reply");
        assert_eq!(reply["type"], "ready");
    }

    #[test]
    fn malformed_commands_and_null_handles_are_rejected() {
        let seen = Seen::default();
        let gfn = unsafe { opennow_gfn_create(&callbacks(&seen)) };
        let broken = b"{not json";
        assert_eq!(
            unsafe { opennow_gfn_command(gfn, broken.as_ptr(), broken.len()) },
            -1
        );
        assert_eq!(unsafe { opennow_gfn_command(gfn, ptr::null(), 0) }, -1);
        assert_eq!(unsafe { opennow_gfn_key(ptr::null_mut(), 0x41, 0, 1) }, -1);
        assert_eq!(unsafe { opennow_gfn_gamepad(gfn, ptr::null()) }, -1);
        assert!(unsafe { opennow_gfn_create(ptr::null()) }.is_null());
        unsafe { opennow_gfn_destroy(gfn) };
        unsafe { opennow_gfn_destroy(ptr::null_mut()) };
    }

    #[test]
    fn microphone_abi_rejects_invalid_and_unnegotiated_source() {
        let seen=Seen::default();
        let gfn=unsafe {opennow_gfn_create(&callbacks(&seen))};
        let opus=[0xf8,0xff,0xfe];
        assert_eq!(unsafe {opennow_gfn_microphone_available(std::ptr::null_mut())},-1);
        assert_eq!(unsafe {opennow_gfn_microphone_enable(std::ptr::null_mut(),1)},-1);
        assert_eq!(unsafe {opennow_gfn_microphone_opus(std::ptr::null_mut(),opus.as_ptr(),3,0)},-1);
        assert_eq!(unsafe {opennow_gfn_microphone_available(gfn)},0);
        assert_eq!(unsafe {opennow_gfn_microphone_enable(gfn,1)},-1);
        for len in [0,1276,usize::MAX] { assert_eq!(unsafe {opennow_gfn_microphone_opus(gfn,opus.as_ptr(),len,0)},-1); }
        assert_eq!(unsafe {opennow_gfn_microphone_opus(gfn,std::ptr::null(),3,0)},-1);
        assert_eq!(unsafe {opennow_gfn_microphone_opus(gfn,opus.as_ptr(),3,960)},-1);
        unsafe {opennow_gfn_destroy(gfn)};
    }

    #[test]
    fn input_calls_reach_the_engine_queue() {
        let seen = Seen::default();
        let gfn = unsafe { opennow_gfn_create(&callbacks(&seen)) };
        let input = Arc::clone(&unsafe { &*gfn }.input);
        assert_eq!(unsafe { opennow_gfn_key(gfn, 0x41, 0, 1) }, 0);
        assert_eq!(unsafe { opennow_gfn_mouse_move(gfn, 24, -24) }, 0);
        assert_eq!(
            input.take(),
            Some(CapturedInput::Key {
                virtual_key: 0x41,
                modifiers: 0,
                pressed: true
            })
        );
        assert_eq!(
            input.take(),
            Some(CapturedInput::MouseMove {
                delta_x: 24,
                delta_y: -24
            })
        );
        unsafe { opennow_gfn_destroy(gfn) };
    }
}
