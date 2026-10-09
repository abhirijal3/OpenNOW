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
