use std::net::UdpSocket;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use opennow_streamer_hid::HidRuntime;
use opennow_streamer_protocol::{
    Capabilities, Command, PROTOCOL_VERSION, SessionContext, error, event, response,
};
use opennow_streamer_transport::{
    FrameStageTimings, NvstControllerRumble, NvstDropReason, NvstReceiveEvent, NvstReceiverState,
    NvstRecovery, NvstUdpReceiverControl, NvstUdpReceiverSession, ReservedNvstBundle,
    SharedNvstFeedback, parse_nvst_video_handoff, reserve_nvst_mjolnir_udp_socket,
    spawn_nvst_mjolnir_receiver, spawn_nvst_udp_receiver_with_socket,
};
use serde_json::{Value, json};

mod input;
mod nvst_rtsp;
mod queue_drops;
mod stream_config;

use nvst_rtsp::{ActiveNvstRtspSession, prepare_owned_nvst};
use queue_drops::QueueDropReports;
use stream_config::{MediaColorQuality, MediaStreamConfig, MediaVideoCodec};

pub use input::{CapturedInput, CapturedInputQueue, CapturedInputSample};
pub use opennow_streamer_transport::{EncodedMediaFrame, MediaConsumer, RawPacketTap};

#[derive(Clone)]
pub struct EventSender {
    inner: EventSenderInner,
}

#[derive(Clone)]
enum EventSenderInner {
    Unbounded(Sender<Value>),
    Bounded(SyncSender<Value>),
}

impl EventSender {
    fn unbounded(sender: Sender<Value>) -> Self {
        Self {
            inner: EventSenderInner::Unbounded(sender),
        }
    }

    pub fn bounded(sender: SyncSender<Value>) -> Self {
        Self {
            inner: EventSenderInner::Bounded(sender),
        }
    }

    fn send(&self, value: Value) -> Result<(), ()> {
        match &self.inner {
            EventSenderInner::Unbounded(sender) => sender.send(value).map_err(|_| ()),
            EventSenderInner::Bounded(sender) => match sender.try_send(value) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => Err(()),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Connected,
}

const NVST_RECOVERY_ATTEMPT_LIMIT: usize = 1;
const NATIVE_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const NATIVE_INPUT_DRAIN_BATCH: usize = 32;
const MAX_STREAM_FPS: u32 = 360;

trait NvstSessionResources {
    fn take_rumble(&self) -> ([Option<NvstControllerRumble>; 4], usize) {
        ([None; 4], 0)
    }
    fn ping_ms(&self) -> Option<f64> {
        None
    }
    fn network_metrics(&self) -> Option<(f64, f64)> {
        None
    }
    fn socket_receive_bytes(&self) -> Option<u64> {
        None
    }
    fn frame_stage_timings(&self) -> Option<FrameStageTimings> {
        None
    }
    fn delivered_frames(&self) -> u64 {
        0
    }
    fn delivered_bytes(&self) -> u64 {
        0
    }
    fn delivered_keyframes(&self) -> u64 {
        0
    }
    fn request_keyframe(&self);
    fn send_captured_input(&self, bytes: Vec<u8>) -> Result<(), String>;
    fn send_captured_text(
        &self,
        text: opennow_streamer_protocol::text_input::UnicodeText,
        timestamp_us: u64,
    ) -> Result<(), String>;
    fn recover(&self) -> Result<(), String>;
    fn stop(&self);
}

struct ActiveNvstResources {
    bundle: NvstUdpReceiverControl,
    mjolnir: Option<NvstUdpReceiverControl>,
    feedback: SharedNvstFeedback,
}

impl NvstSessionResources for ActiveNvstResources {
    fn take_rumble(&self) -> ([Option<NvstControllerRumble>; 4], usize) {
        self.feedback.haptics.take()
    }
    fn ping_ms(&self) -> Option<f64> {
        self.feedback.ping_ms(Instant::now())
    }
    fn network_metrics(&self) -> Option<(f64, f64)> {
        self.feedback.recent_network_metrics(Instant::now())
    }
    fn socket_receive_bytes(&self) -> Option<u64> {
        Some(self.feedback.socket_receive_bytes())
    }
    fn frame_stage_timings(&self) -> Option<FrameStageTimings> {
        let timings = self.feedback.frame_stage_timings();
        (!timings.is_empty()).then_some(timings)
    }
    fn request_keyframe(&self) {
        self.feedback.request_keyframe();
    }
    fn delivered_frames(&self) -> u64 {
        self.feedback.delivered_frames()
    }
    fn delivered_bytes(&self) -> u64 {
        self.feedback.delivered_bytes()
    }
    fn delivered_keyframes(&self) -> u64 {
        self.feedback.delivered_keyframes()
    }

    fn send_captured_input(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.bundle
            .queue_input(bytes, false)
            .map_err(|error| error.to_string())
    }

    fn send_captured_text(
        &self,
        text: opennow_streamer_protocol::text_input::UnicodeText,
        timestamp_us: u64,
    ) -> Result<(), String> {
        self.bundle
            .queue_text(text, timestamp_us)
            .map_err(|error| error.to_string())
    }

    fn recover(&self) -> Result<(), String> {
        self.bundle
            .recover()
            .map_err(|error| format!("bundle recovery failed: {error}"))?;
        if let Some(mjolnir) = self.mjolnir.as_ref() {
            mjolnir
                .recover()
                .map_err(|error| format!("Mjolnir recovery failed: {error}"))?;
        }
        Ok(())
    }

    fn stop(&self) {
        let _ = self.bundle.stop();
        if let Some(mjolnir) = self.mjolnir.as_ref() {
            let _ = mjolnir.stop();
        }
    }
}

pub struct Engine {
    lifecycle: Arc<Mutex<Lifecycle>>,
    nvst_transport: Option<NvstUdpReceiverSession>,
    nvst_mjolnir_transport: Option<NvstUdpReceiverSession>,
    reserved_nvst_bundle: Option<ReservedNvstBundle>,
    nvst_hole_punch_socket: Option<UdpSocket>,
    nvst_rtsp: Option<ActiveNvstRtspSession>,
    events: EventSender,
    media_consumer: MediaConsumer,
    event_worker: Option<JoinHandle<QueueDropReports>>,
    captured_input: Arc<CapturedInputQueue>,
    hid_runtime: Arc<HidRuntime>,
    raw_video_tap: Option<RawPacketTap>,
}

#[derive(Debug)]
struct Lifecycle {
    state: State,
    context: Option<SessionContext>,
    generation: u64,
}

impl Engine {
    pub fn with_media_consumer(events: Sender<Value>, media_consumer: MediaConsumer) -> Self {
        Self::with_media_consumer_and_event_sender(EventSender::unbounded(events), media_consumer)
    }

    pub fn with_media_consumer_and_event_sender(
        events: EventSender,
        media_consumer: MediaConsumer,
    ) -> Self {
        Self {
            lifecycle: Arc::new(Mutex::new(Lifecycle {
                state: State::Idle,
                context: None,
                generation: 0,
            })),
            nvst_transport: None,
            nvst_mjolnir_transport: None,
            reserved_nvst_bundle: None,
            nvst_hole_punch_socket: None,
            nvst_rtsp: None,
            events,
            media_consumer,
            event_worker: None,
            captured_input: Arc::new(CapturedInputQueue::default()),
            hid_runtime: Arc::new(HidRuntime::new()),
            raw_video_tap: None,
        }
    }

    pub fn with_hid_runtime(mut self, hid_runtime: Arc<HidRuntime>) -> Self {
        self.hid_runtime = hid_runtime;
        self
    }

    pub fn with_raw_video_tap(mut self, tap: RawPacketTap) -> Self {
        self.raw_video_tap = Some(tap);
        self
    }

    pub fn captured_input(&self) -> Arc<CapturedInputQueue> {
        Arc::clone(&self.captured_input)
    }

    pub fn set_microphone_enabled(&self, enabled: bool) -> Result<(), String> {
        let transport = self
            .nvst_transport
            .as_ref()
            .ok_or_else(|| "no active session".to_owned())?;
        transport
            .control()
            .set_microphone_enabled(enabled)
            .map_err(|error| error.to_string())
    }

    pub fn send_microphone_opus(&self, payload: Vec<u8>, rtp_timestamp: u32) -> Result<(), String> {
        let transport = self
            .nvst_transport
            .as_ref()
            .ok_or_else(|| "no active session".to_owned())?;
        transport
            .control()
            .send_microphone_opus(payload, rtp_timestamp)
            .map_err(|error| error.to_string())
    }

    pub fn handle(&mut self, command: Command) -> (Vec<Value>, bool) {
        let summary = opennow_streamer_protocol::log::message_summary(&json!({
            "id": &command.id, "type": &command.kind
        }));
        opennow_streamer_protocol::log::log_line(
            "INFO",
            "engine-command",
            &format!("begin {summary}"),
        );
        let mut stage = opennow_streamer_protocol::log::Stage::begin("engine.command");
        let id = command.id.clone();
        let result = match command.kind.as_str() {
            "hello" => self.hello(&command),
            "nvst-bind" => self.nvst_bind(command),
            "nvst-unbind" => self.nvst_unbind(command),
            "nvst-send" => self.nvst_send(command),
            "start" => self.start(command),
            "anti-afk-pulse" => self.anti_afk_pulse(command),
            "stop" => {
                self.stop(command.reason.as_deref().unwrap_or("stopped"));
                Ok(vec![response(id, "ok")])
            }
            "shutdown" => {
                self.stop(command.reason.as_deref().unwrap_or("shutdown"));
                stage.complete();
                return (vec![response(id, "ok")], false);
            }
            other => Err(error(
                Some(&id),
                "unknown-command",
                format!("Unknown command: {other}"),
            )),
        };

        if result.is_ok() {
            stage.complete();
        }
        opennow_streamer_protocol::log::log_line(
            "INFO",
            "engine-command",
            &format!("end {summary} success={}", result.is_ok()),
        );
        match result {
            Ok(values) => (values, true),
            Err(value) => (vec![value], true),
        }
    }

    fn hello(&self, command: &Command) -> Result<Vec<Value>, Value> {
        if command.protocol_version != Some(PROTOCOL_VERSION) {
            return Err(error(
                Some(&command.id),
                "protocol-version-mismatch",
                format!("Native streamer requires protocol {PROTOCOL_VERSION}"),
            ));
        }
        opennow_streamer_protocol::log::log_line(
            "INFO",
            "handshake",
            &format!("protocol={PROTOCOL_VERSION}"),
        );
        let capabilities = Capabilities {
            protocol_version: PROTOCOL_VERSION,
            backend: "native",
            supports_input: true,
            supports_microphone: true,
            supports_owned_nvst_negotiation: true,
        };
        let ready = json!({
            "id": command.id,
            "type": "ready",
            "processId": std::process::id(),
            "capabilities": capabilities,
        });
        Ok(vec![ready])
    }

    fn nvst_bind(&mut self, command: Command) -> Result<Vec<Value>, Value> {
        if self.reserved_nvst_bundle.is_none() {
            let bundle = ReservedNvstBundle::reserve().map_err(|bind_error| {
                error(
                    Some(&command.id),
                    "nvst-bind-failed",
                    format!("failed to reserve NVST UDP socket: {bind_error}"),
                )
            })?;
            eprintln!(
                "NVST reserved video UDP socket on {} (Mjolnir on {})",
                bundle
                    .local_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| "unknown".to_owned()),
                bundle
                    .mjolnir_local_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| "unknown".to_owned()),
            );
            self.reserved_nvst_bundle = Some(bundle);
        }
        let bundle = self.reserved_nvst_bundle.as_mut().ok_or_else(|| {
            error(
                Some(&command.id),
                "nvst-bind-failed",
                "reserved NVST UDP socket has no local port",
            )
        })?;
        let local_addr = bundle.local_addr().map_err(|_| {
            error(
                Some(&command.id),
                "nvst-bind-failed",
                "reserved NVST UDP socket has no local port",
            )
        })?;
        let mjolnir_addr = bundle.mjolnir_local_addr().map_err(|_| {
            error(
                Some(&command.id),
                "nvst-bind-failed",
                "reserved NVST Mjolnir UDP socket has no local port",
            )
        })?;
        let port = local_addr.port();
        let local_address = bundle.advertised_local_address();
        let identity = bundle.identity();
        Ok(vec![json!({
            "id": command.id,
            "type": "nvst-bound",
            "port": port,
            "mjolnirPort": mjolnir_addr.port(),
            "localAddress": local_address,
            "iceUsernameFragment": identity.ice_username_fragment,
            "icePassword": identity.ice_password,
            "dtlsFingerprint": identity.dtls_fingerprint,
        })])
    }

    fn nvst_send(&mut self, command: Command) -> Result<Vec<Value>, Value> {
        let host = command.host.ok_or_else(|| {
            error(
                Some(&command.id),
                "nvst-send-failed",
                "nvst-send requires host",
            )
        })?;
        let port = command.port.ok_or_else(|| {
            error(
                Some(&command.id),
                "nvst-send-failed",
                "nvst-send requires port",
            )
        })?;
        let payload = BASE64
            .decode(command.payload_base64.unwrap_or_default())
            .map_err(|decode_error| {
                error(
                    Some(&command.id),
                    "nvst-send-failed",
                    format!("nvst-send payload is not valid base64: {decode_error}"),
                )
            })?;
        let send_result = if let Some(bundle) = self.reserved_nvst_bundle.as_ref() {
            bundle.send_to(&payload, host.as_str(), port)
        } else if let Some(socket) = self.nvst_hole_punch_socket.as_ref() {
            socket.send_to(&payload, (host.as_str(), port))
        } else {
            return Err(error(
                Some(&command.id),
                "nvst-send-failed",
                "NVST UDP socket has not been reserved",
            ));
        };
        send_result.map_err(|send_error| {
            error(
                Some(&command.id),
                "nvst-send-failed",
                format!("failed to send NVST UDP datagram: {send_error}"),
            )
        })?;
        Ok(vec![response(command.id, "ok")])
    }

    fn nvst_unbind(&mut self, command: Command) -> Result<Vec<Value>, Value> {
        let lifecycle = lock_lifecycle(&self.lifecycle);
        if lifecycle.state != State::Idle
            || self.nvst_transport.is_some()
            || self.nvst_mjolnir_transport.is_some()
        {
            return Err(error(
                Some(&command.id),
                "nvst-unbind-in-use",
                "Cannot release an NVST UDP reservation after session start",
            ));
        }
        drop(lifecycle);
        self.reserved_nvst_bundle = None;
        self.nvst_hole_punch_socket = None;
        Ok(vec![response(command.id, "ok")])
    }

    fn start(&mut self, command: Command) -> Result<Vec<Value>, Value> {
        let mut context = parse_context(command.context, &command.id)?;
        validate_context(&context, &command.id)?;
        opennow_streamer_protocol::log::log_line("INFO", "session", "context validated");
        {
            let lifecycle = lock_lifecycle(&self.lifecycle);
            if lifecycle.state != State::Idle {
                return Err(invalid_state(&command.id, "start", lifecycle.state, "Idle"));
            }
        }
        let wants_owned_nvst = context
            .settings
            .get("transportMode")
            .and_then(Value::as_str)
            .is_some_and(|mode| mode.eq_ignore_ascii_case("nvst"))
            && context.nvst_video.is_none();
        let mut prepared_nvst = if wants_owned_nvst {
            if self.reserved_nvst_bundle.is_none() {
                self.reserved_nvst_bundle =
                    Some(ReservedNvstBundle::reserve().map_err(|error_value| {
                        error(
                            Some(&command.id),
                            "nvst-bind-failed",
                            format!(
                                "Native streamer could not reserve its NVST sockets: {error_value}"
                            ),
                        )
                    })?);
            }
            let prepared_result = {
                let mut stage = opennow_streamer_protocol::log::Stage::begin("nvst.negotiate");
                let bundle = self
                    .reserved_nvst_bundle
                    .as_mut()
                    .expect("NVST reservation created above");
                let result = prepare_owned_nvst(&context, bundle);
                if result.is_ok() {
                    stage.complete();
                }
                result
            };
            let prepared = match prepared_result {
                Ok(prepared) => prepared,
                Err(negotiation_error) => {
                    self.reserved_nvst_bundle = None;
                    return Err(error(
                        Some(&command.id),
                        negotiation_error.code,
                        negotiation_error.message,
                    ));
                }
            };
            context.nvst_video = Some(prepared.handoff.clone());
            Some(prepared)
        } else {
            None
        };
        let transport_context = serde_json::to_value(&context).map_err(|context_error| {
            error(
                Some(&command.id),
                "invalid-context",
                format!("Session context is not serializable: {context_error}"),
            )
        })?;
        let nvst_config = match parse_nvst_video_handoff(&transport_context) {
            Ok(Some(config)) => Some(match &self.raw_video_tap {
                Some(tap) => config
                    .with_raw_video_tap(Arc::clone(tap))
                    .with_header_only(true),
                None => config,
            }),
            Ok(None) => {
                return Err(error(
                    Some(&command.id),
                    "nvst-handoff-required",
                    "Native streaming requires an NVST handoff",
                ));
            }
            Err(reason) => {
                return Err(error(
                    Some(&command.id),
                    "invalid-nvst-handoff",
                    format!("NVST transport is invalid: {reason}"),
                ));
            }
        };
        let nvst_bundle_available = nvst_config
            .as_ref()
            .is_some_and(|config| config.remote_dtls_fingerprint().is_some());
        let nvst_audio_negotiated = nvst_config
            .as_ref()
            .is_some_and(|config| config.audio_track().is_some());
        let microphone_requested =
            context.settings["microphoneMode"].as_str() == Some("voice-activity");
        let microphone_available = microphone_requested
            && nvst_config
                .as_ref()
                .is_some_and(|config| config.microphone_available());

        if let Some(transport) = self.nvst_transport.take() {
            transport.stop();
        }
        if let Some(transport) = self.nvst_mjolnir_transport.take() {
            transport.stop();
        }
        if let Some(mut rtsp) = self.nvst_rtsp.take() {
            rtsp.shutdown();
        }
        self.stop_media_resources();
        let stream_config = prepared_nvst
            .as_ref()
            .map(|prepared| prepared.media_config)
            .unwrap_or_else(|| media_stream_config(&context));
        opennow_streamer_protocol::log::log_line(
            "INFO",
            "media-config",
            &format!(
                "codec={:?} color={:?} width={} height={} fps={} bitrate_bps={} audio_negotiated={} dtls_bundle={}",
                stream_config.codec,
                stream_config.color_quality,
                stream_config.width,
                stream_config.height,
                stream_config.fps,
                stream_config.bitrate_bps,
                nvst_audio_negotiated,
                nvst_bundle_available
            ),
        );

        if let Some(prepared) = prepared_nvst.as_mut()
            && let Err(negotiation_error) = prepared.announce()
        {
            self.stop_media_resources();
            self.reserved_nvst_bundle = None;
            return Err(error(
                Some(&command.id),
                negotiation_error.code,
                negotiation_error.message,
            ));
        }

        let mut nvst_events = None;
        let mut nvst_resources = None;
        let mut nvst_upstream_ready = None;
        if let Some(config) = nvst_config {
            let media_consumer = self.media_consumer.clone();
            let (event_sender, event_receiver) = std::sync::mpsc::channel();
            let input_waker = event_sender.clone();
            self.captured_input.set_waker(Some(Box::new(move || {
                let _ = input_waker.send(NvstReceiveEvent::Wake);
            })));
            let (reserved_socket, reserved_rtc, reserved_mjolnir) =
                match self.reserved_nvst_bundle.take() {
                    Some(bundle) => {
                        self.nvst_hole_punch_socket = bundle.try_clone_socket().ok();
                        let (socket, rtc, mjolnir_socket) = bundle.into_parts();
                        (Some(socket), Some(rtc), Some(mjolnir_socket))
                    }
                    None => (None, None, None),
                };
            let mjolnir_udp_port = config.mjolnir_udp_port();
            let feedback = config.feedback();
            let (upstream_ready, upstream_waiter) = if prepared_nvst.is_some() {
                let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                (Some(sender), Some(receiver))
            } else {
                (None, None)
            };
            let transport = match spawn_nvst_udp_receiver_with_socket(
                config.clone(),
                media_consumer.clone(),
                event_sender.clone(),
                reserved_socket,
                reserved_rtc,
                Arc::clone(&self.hid_runtime),
                upstream_waiter,
            ) {
                Ok(transport) => transport,
                Err(transport_error) => {
                    drop(media_consumer);
                    self.stop_media_resources();
                    return Err(error(
                        Some(&command.id),
                        "nvst-start-failed",
                        transport_error.to_string(),
                    ));
                }
            };
            let bundle_control = transport.control();
            self.nvst_transport = Some(transport);
            let mut mjolnir_control = None;
            if let Some(expected_port) = mjolnir_udp_port {
                // Official two-socket model: video RTP/SRTP arrives on the
                // dedicated NATT-only Mjolnir socket, not on the ICE/DTLS bundle.
                let mjolnir_socket = match reserved_mjolnir {
                    Some(socket) => {
                        let actual_port = socket.local_addr().map(|addr| addr.port()).unwrap_or(0);
                        if actual_port != expected_port {
                            eprintln!(
                                "NVST Mjolnir socket port mismatch: reserved {actual_port}, handoff expects {expected_port}; NATT keepalive determines routing"
                            );
                        }
                        socket
                    }
                    None => {
                        eprintln!(
                            "NVST Mjolnir reservation missing at start; binding a fresh video UDP socket"
                        );
                        reserve_nvst_mjolnir_udp_socket().map_err(|bind_error| {
                            if let Some(transport) = self.nvst_transport.take() {
                                transport.stop();
                            }
                            self.stop_media_resources();
                            error(
                                Some(&command.id),
                                "nvst-start-failed",
                                format!("failed to reserve NVST Mjolnir UDP socket: {bind_error}"),
                            )
                        })?
                    }
                };
                let mjolnir = spawn_nvst_mjolnir_receiver(
                    mjolnir_socket,
                    config,
                    media_consumer,
                    event_sender,
                )
                .map_err(|mjolnir_error| {
                    if let Some(transport) = self.nvst_transport.take() {
                        transport.stop();
                    }
                    self.stop_media_resources();
                    error(
                        Some(&command.id),
                        "nvst-start-failed",
                        mjolnir_error.to_string(),
                    )
                })?;
                mjolnir_control = Some(mjolnir.control());
                self.nvst_mjolnir_transport = Some(mjolnir);
            }
            nvst_resources = Some(ActiveNvstResources {
                bundle: bundle_control,
                mjolnir: mjolnir_control,
                feedback,
            });
            nvst_events = Some(event_receiver);
            nvst_upstream_ready = upstream_ready;
        } else {
            self.reserved_nvst_bundle = None;
            self.nvst_hole_punch_socket = None;
        }

        let generation = {
            let mut lifecycle = lock_lifecycle(&self.lifecycle);
            lifecycle.generation = lifecycle.generation.wrapping_add(1);
            lifecycle.context = Some(context);
            lifecycle.state = State::Connected;
            lifecycle.generation
        };
        if self.hid_runtime.bind_session(generation).is_none() {
            self.stop("NVST association exited before the session start was accepted");
            return Err(error(
                Some(&command.id),
                "nvst-start-failed",
                "NVST association exited before the session start was accepted",
            ));
        }
        if let Some(nvst_events) = nvst_events {
            let output = self.events.clone();
            let lifecycle = self.lifecycle.clone();
            let captured_input = Arc::clone(&self.captured_input);
            let start_id = command.id.clone();
            let nvst_resources = nvst_resources.expect("NVST events require active resources");
            self.event_worker = thread::Builder::new()
                .name("opennow-nvst-events".to_owned())
                .spawn(move || {
                    forward_nvst_session_events(
                        &output,
                        &lifecycle,
                        generation,
                        NvstSessionEventResources {
                            start_id,
                            nvst_events,
                            captured_input,
                            transport: nvst_resources,
                        },
                    )
                })
                .ok();
            if self.event_worker.is_none() {
                if let Some(transport) = self.nvst_transport.take() {
                    transport.stop();
                }
                if let Some(transport) = self.nvst_mjolnir_transport.take() {
                    transport.stop();
                }
                self.stop_media_resources();
                let mut lifecycle = lock_lifecycle(&self.lifecycle);
                if lifecycle.generation == generation {
                    lifecycle.context = None;
                    lifecycle.state = State::Idle;
                }
                drop(lifecycle);
                self.hid_runtime.unbind_session(generation);
                return Err(error(
                    Some(&command.id),
                    "media-worker-failed",
                    "Failed to start NVST lifecycle worker",
                ));
            }
        }
        if let Some(prepared) = prepared_nvst {
            match prepared.finish() {
                Ok(active) => {
                    self.nvst_rtsp = Some(active);
                    if nvst_upstream_ready
                        .take()
                        .is_some_and(|ready| ready.try_send(()).is_err())
                    {
                        self.stop("NVST bundle exited before PLAY completed");
                        return Err(error(
                            Some(&command.id),
                            "nvst-start-failed",
                            "NVST bundle exited before PLAY completed",
                        ));
                    }
                }
                Err(negotiation_error) => {
                    self.stop("Native-owned NVST negotiation failed");
                    return Err(error(
                        Some(&command.id),
                        negotiation_error.code,
                        negotiation_error.message,
                    ));
                }
            }
        }
        let _ = self.events.send(event(
            "status",
            json!({
                "status": "ready",
                "message": "NVST authenticated media path initialized"
            }),
        ));
        let mut start_response = response(command.id, "ok");
        start_response["transport"] = Value::String("nvst".to_owned());
        start_response["capabilities"] = json!({
            "supportsInput": nvst_bundle_available,
            "supportsAudio": nvst_audio_negotiated,
            "supportsMicrophone": microphone_available,
        });
        Ok(vec![start_response])
    }

    fn stop(&mut self, reason: &str) {
        self.captured_input.set_waker(None);
        if let Some(generation) = self.hid_runtime.session_generation() {
            self.hid_runtime.unbind_session(generation);
        }
        let was_active = {
            let mut lifecycle = lock_lifecycle(&self.lifecycle);
            let was_active = lifecycle.state != State::Idle;
            lifecycle.generation = lifecycle.generation.wrapping_add(1);
            lifecycle.context = None;
            lifecycle.state = State::Idle;
            was_active
        };
        if let Some(transport) = self.nvst_transport.take() {
            transport.stop();
        }
        if let Some(transport) = self.nvst_mjolnir_transport.take() {
            transport.stop();
        }
        if let Some(mut rtsp) = self.nvst_rtsp.take() {
            rtsp.shutdown();
        }
        self.reserved_nvst_bundle = None;
        self.nvst_hole_punch_socket = None;
        self.stop_media_resources();
        if was_active {
            let _ = self.events.send(event(
                "status",
                json!({ "status": "stopped", "message": reason }),
            ));
        }
    }

    fn stop_media_resources(&mut self) {
        self.captured_input.clear();
        if let Some(worker) = self.event_worker.take()
            && let Ok(mut reports) = worker.join()
        {
            reports.flush(&self.events, Instant::now(), true);
        }
    }

    fn anti_afk_pulse(&self, command: Command) -> Result<Vec<Value>, Value> {
        let state = lock_lifecycle(&self.lifecycle).state;
        if state != State::Connected {
            return Err(invalid_state(
                &command.id,
                "anti-afk-pulse",
                state,
                "Connected with an initialized input channel",
            ));
        }
        let send = |input| {
            let bytes = captured_input_packet(input, 0);
            if let Some(transport) = self.nvst_transport.as_ref() {
                transport.send_input(bytes, false)
            } else {
                Err(opennow_streamer_transport::TransportError::Closed)
            }
        };
        send(CapturedInput::Key {
            virtual_key: 0x7c,
            modifiers: 0,
            pressed: true,
        })
        .and_then(|_| {
            send(CapturedInput::Key {
                virtual_key: 0x7c,
                modifiers: 0,
                pressed: false,
            })
        })
        .map_err(|transport_error| {
            error(
                Some(&command.id),
                transport_error.code(),
                transport_error.to_string(),
            )
        })?;
        Ok(vec![response(command.id, "ok")])
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop("process closed");
    }
}

fn parse_context(context: Option<Value>, id: &str) -> Result<SessionContext, Value> {
    let context = context.ok_or_else(|| {
        error(
            Some(id),
            "missing-context",
            "Command requires session context",
        )
    })?;
    serde_json::from_value(context).map_err(|context_error| {
        error(
            Some(id),
            "invalid-context",
            format!("Invalid session context: {context_error}"),
        )
    })
}

fn validate_context(context: &SessionContext, id: &str) -> Result<(), Value> {
    if context.session.session_id.trim().is_empty() {
        return Err(error(
            Some(id),
            "invalid-context",
            "Session context requires a non-empty sessionId",
        ));
    }
    if context.session.server_ip.trim().is_empty() {
        return Err(error(
            Some(id),
            "invalid-context",
            "Session context requires a non-empty serverIp endpoint",
        ));
    }
    if !context.settings.is_object() || !context.shortcuts.is_object() {
        return Err(error(
            Some(id),
            "invalid-context",
            "Session context settings and shortcuts must be objects",
        ));
    }
    if let Some(profile) = context.session.extra.get("negotiatedStreamProfile") {
        let color_reported = ["bitDepthSource", "chromaFormatSource"].iter().any(|key| {
            matches!(
                profile[*key].as_str(),
                Some("request" | "finalized" | "server")
            )
        });
        if (color_reported && profile["colorQuality"].as_str().is_none())
            || (profile["enableHdrSource"] == "server" && profile["enableHdr"].as_bool().is_none())
        {
            return Err(error(
                Some(id),
                "invalid-context",
                "The accepted color or HDR profile is incomplete or unsupported",
            ));
        }
        let codec = profile["codec"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase();
        let color = profile["colorQuality"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if (codec == "H264" && matches!(color.as_str(), "8bit_444" | "10bit_420" | "10bit_444"))
            || (codec == "AV1" && matches!(color.as_str(), "8bit_444" | "10bit_444"))
        {
            return Err(error(
                Some(id),
                "invalid-context",
                "The accepted codec and color profile cannot be preserved by the streamer",
            ));
        }
    }
    // HDR acceptance comes from the negotiated profile, but the codec is
    // client-selected (the official client never sends it to CloudMatch), so
    // validate the effective stream config rather than the raw profile: a
    // session without a server-reported codec must not fail when the client
    // selected HEVC/AV1 with 10-bit color.
    let stream = media_stream_config(context);
    if stream.hdr
        && !matches!(
            (stream.codec, stream.color_quality),
            (
                MediaVideoCodec::H265,
                MediaColorQuality::TenBit420 | MediaColorQuality::TenBit444
            ) | (MediaVideoCodec::Av1, MediaColorQuality::TenBit420)
        )
    {
        return Err(error(
            Some(id),
            "invalid-context",
            "HDR requires an accepted HEVC/AV1 10-bit profile with supported chroma",
        ));
    }
    if let Some(endpoint) = &context.session.media_connection_info {
        if endpoint.ip.trim().is_empty() || endpoint.port == 0 || endpoint.port > u16::MAX.into() {
            return Err(error(
                Some(id),
                "invalid-context",
                "mediaConnectionInfo requires a hostname and a port in 1..=65535",
            ));
        }
    }
    serde_json::to_value(context).map_err(|context_error| {
        error(
            Some(id),
            "invalid-context",
            format!("Session context is not serializable: {context_error}"),
        )
    })?;
    Ok(())
}

fn invalid_state(id: &str, command: &str, state: State, required: &str) -> Value {
    error(
        Some(id),
        "invalid-state",
        format!("Cannot apply {command} while lifecycle is {state:?}; required state: {required}"),
    )
}

fn lock_lifecycle(lifecycle: &Mutex<Lifecycle>) -> MutexGuard<'_, Lifecycle> {
    lifecycle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct NvstSessionEventResources<R> {
    start_id: String,
    nvst_events: Receiver<NvstReceiveEvent>,
    captured_input: Arc<CapturedInputQueue>,
    transport: R,
}

fn forward_nvst_session_events<R: NvstSessionResources>(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    event_resources: NvstSessionEventResources<R>,
) -> QueueDropReports {
    let NvstSessionEventResources {
        start_id,
        nvst_events,
        captured_input,
        transport: resources,
    } = event_resources;
    let mut feedback_state = NvstMediaFeedbackState::new(false);
    feedback_state.previous_socket_receive_bytes = resources.socket_receive_bytes().unwrap_or(0);
    feedback_state.start_id = start_id.clone();
    captured_input.set_text_ready(generation, false);
    let mut pending_rumble = [None; 4];
    let mut pending_cursor_capture = NvstCursorCaptureOutput {
        start_id: start_id.clone(),
        pending: None,
    };
    'session: loop {
        let mut drained = 0_usize;
        flush_cursor_capture(output, lifecycle, generation, &mut pending_cursor_capture);
        observe_delivered_frames(&resources, &mut feedback_state);
        feedback_state
            .drop_reports
            .flush(output, Instant::now(), false);
        if lock_lifecycle(lifecycle).generation != generation {
            break;
        }
        let (rumble, coalesced) = resources.take_rumble();
        feedback_state
            .drop_reports
            .record("controller-rumble-coalesced", coalesced);
        for command in rumble.into_iter().flatten() {
            if pending_rumble[usize::from(command.controller_id)]
                .replace(command)
                .is_some()
            {
                feedback_state
                    .drop_reports
                    .record("controller-rumble-coalesced", 1);
            }
        }
        for pending in &mut pending_rumble {
            if let Some(command) = *pending
                && forward_controller_rumble(output, lifecycle, generation, &start_id, command)
            {
                *pending = None;
            }
        }
        if !feedback_state.input_available {
            captured_input.clear();
        } else if captured_input.take_overflowed() {
            let _ = emit_nvst_terminal(
                output,
                lifecycle,
                generation,
                &resources,
                "native-input-capture-overflow",
                "Native input capture queue overflowed; stopping to prevent stuck input".to_owned(),
            );
            break;
        } else {
            captured_input.begin_drain();
            for _ in 0..NATIVE_INPUT_DRAIN_BATCH {
                let Some(input) = captured_input.take_sample() else {
                    break;
                };
                drained += 1;
                if let Err(error) = forward_nvst_captured_sample(&resources, input, &feedback_state)
                {
                    let _ = emit_nvst_terminal(
                        output,
                        lifecycle,
                        generation,
                        &resources,
                        "native-input-capture-failed",
                        format!("Native window input capture failed: {error}"),
                    );
                    break 'session;
                }
            }
        }
        let wait = if drained == NATIVE_INPUT_DRAIN_BATCH {
            Duration::ZERO
        } else {
            NATIVE_INPUT_POLL_INTERVAL
        };
        match nvst_events.recv_timeout(wait) {
            Ok(NvstReceiveEvent::Wake) => {}
            Ok(nvst_event) => {
                match &nvst_event {
                    NvstReceiveEvent::InputReady(_) => {
                        feedback_state.input_available = true;
                        captured_input.set_text_ready(generation, true);
                    }
                    NvstReceiveEvent::InputUnavailable(_) => {
                        feedback_state.input_available = false;
                        captured_input.set_text_ready(generation, false);
                    }
                    _ => {}
                }
                match nvst_event {
                    NvstReceiveEvent::FrameProgressStall { .. }
                    | NvstReceiveEvent::RecoveryNeeded(NvstRecovery::FrameProgress { .. }) => {
                        feedback_state.transport_frame_progress_stalled = true;
                    }
                    NvstReceiveEvent::FrameProgressResumed => {
                        feedback_state.transport_frame_progress_stalled = false;
                    }
                    _ => {}
                }
                let terminal = forward_nvst_event(
                    output,
                    lifecycle,
                    generation,
                    &resources,
                    &mut feedback_state.recovery_attempts,
                    &mut pending_cursor_capture,
                    nvst_event,
                );
                if terminal {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = emit_nvst_terminal(
                    output,
                    lifecycle,
                    generation,
                    &resources,
                    "nvst-event-channel-closed",
                    "NVST receiver event channel closed unexpectedly".to_owned(),
                );
                break;
            }
        }
        if feedback_state.telemetry_started
            || resources
                .socket_receive_bytes()
                .is_some_and(|bytes| bytes > feedback_state.previous_socket_receive_bytes)
        {
            feedback_state.telemetry_started = true;
            flush_nvst_telemetry(output, &resources, &mut feedback_state);
        }
    }
    captured_input.set_text_ready(generation, false);
    feedback_state
        .drop_reports
        .flush(output, Instant::now(), true);
    feedback_state.drop_reports
}

fn observe_delivered_frames<R: NvstSessionResources>(
    resources: &R,
    state: &mut NvstMediaFeedbackState,
) {
    let delivered_keyframes = resources.delivered_keyframes();
    if delivered_keyframes != state.delivered_keyframes {
        state.delivered_keyframes = delivered_keyframes;
        state.recovery_attempts = 0;
    }
    let delivered_frames = resources.delivered_frames();
    if delivered_frames != state.delivered_frames {
        state.delivered_frames = delivered_frames;
        state.transport_frame_progress_stalled = false;
        state.telemetry_started = true;
    }
}

fn forward_controller_rumble(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    start_id: &str,
    command: NvstControllerRumble,
) -> bool {
    let current = lock_lifecycle(lifecycle);
    if current.generation != generation || current.state != State::Connected {
        return true;
    }
    let mut payload = json!({
        "startId": start_id,
        "controllerId": command.controller_id,
        "lowFrequency": command.low_frequency,
        "highFrequency": command.high_frequency,
        "durationMs": command.duration_ms,
    });
    if let Some(incarnation) = command.source_incarnation
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("sourceIncarnation".to_owned(), json!(incarnation));
    }
    output.send(event("controller-rumble", payload)).is_ok()
}

#[derive(Default)]
struct NvstCursorCaptureOutput {
    start_id: String,
    pending: Option<bool>,
}

fn flush_cursor_capture(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    state: &mut NvstCursorCaptureOutput,
) {
    let Some(composited) = state.pending else {
        return;
    };
    let current = lock_lifecycle(lifecycle);
    if current.generation != generation
        || current.context.is_none()
        || output
            .send(event(
                "cursor-capture",
                json!({ "startId": state.start_id, "composited": composited }),
            ))
            .is_ok()
    {
        state.pending = None;
    }
}

fn forward_nvst_event<R: NvstSessionResources>(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    resources: &R,
    recovery_attempts: &mut usize,
    pending_cursor_capture: &mut NvstCursorCaptureOutput,
    nvst_event: NvstReceiveEvent,
) -> bool {
    if lock_lifecycle(lifecycle).generation != generation {
        return true;
    }

    match nvst_event {
        NvstReceiveEvent::MicrophoneError(message) => {
            opennow_streamer_protocol::log::log_line("WARN", "microphone", &message);
            false
        }
        NvstReceiveEvent::FrameProgressStall {
            idle_for,
            recovery_required: false,
            ..
        } => {
            let _ = output.send(event(
                "log",
                json!({
                    "level": "warn",
                    "message": format!(
                        "Produced-frame stall for {idle_for:?}; requested a fresh keyframe"
                    )
                }),
            ));
            false
        }
        NvstReceiveEvent::FrameProgressStall { .. } | NvstReceiveEvent::FrameProgressResumed => {
            false
        }
        NvstReceiveEvent::RecoveryNeeded(NvstRecovery::PacketGap {
            first_missing_index,
            last_missing_index,
        }) => {
            // Packet loss is expected on the UDP media leg. The reorder buffer
            // has already skipped the unrecoverable range and reset the frame
            // assembler, so request a clean decoder reference without spending
            // the terminal transport-recovery budget. Several gaps can arrive
            // before the requested keyframe reaches us at high bitrates.
            resources.request_keyframe();
            let _ = output.send(event(
                "log",
                json!({
                    "level": "warn",
                    "message": format!(
                        "Recovering NVST packet gap with a fresh keyframe: {first_missing_index}..={last_missing_index}"
                    )
                }),
            ));
            false
        }
        NvstReceiveEvent::RecoveryNeeded(recovery) => attempt_nvst_recovery(
            output,
            lifecycle,
            generation,
            resources,
            recovery_attempts,
            format!("{recovery:?}"),
        ),
        NvstReceiveEvent::Lifecycle(NvstReceiverState::RecoveryRequired) => attempt_nvst_recovery(
            output,
            lifecycle,
            generation,
            resources,
            recovery_attempts,
            "authenticated media timeout".to_owned(),
        ),
        NvstReceiveEvent::Lifecycle(NvstReceiverState::Stopped) => emit_nvst_terminal(
            output,
            lifecycle,
            generation,
            resources,
            "nvst-transport-stopped",
            "NVST receiver stopped unexpectedly".to_owned(),
        ),
        NvstReceiveEvent::Dropped(NvstDropReason::MediaConsumerBackpressured) => {
            resources.request_keyframe();
            let _ = output.send(event(
                "log",
                json!({
                    "level": "warn",
                    "message": "Dropped a backpressured NVST video frame and requested a fresh keyframe"
                }),
            ));
            false
        }
        NvstReceiveEvent::Dropped(NvstDropReason::MediaConsumerClosed) => emit_nvst_terminal(
            output,
            lifecycle,
            generation,
            resources,
            "media-consumer-closed",
            "NVST receiver stopped because the media consumer closed".to_owned(),
        ),
        NvstReceiveEvent::Lifecycle(NvstReceiverState::Running) => {
            lock_lifecycle(lifecycle).state = State::Connected;
            let _ = output.send(event(
                "status",
                json!({ "status": "streaming", "message": "NVST SRTP video receiver is running" }),
            ));
            false
        }
        NvstReceiveEvent::Lifecycle(NvstReceiverState::Paused) => {
            let _ = output.send(event(
                "status",
                json!({ "status": "paused", "message": "NVST SRTP video receiver is paused" }),
            ));
            false
        }
        NvstReceiveEvent::TransportReady(phase) => {
            let _ = output.send(event("nvst-transport-ready", json!({ "phase": phase })));
            false
        }
        NvstReceiveEvent::InputReady(protocol_version) => {
            let _ = output.send(event(
                "input-ready",
                json!({ "protocolVersion": protocol_version }),
            ));
            false
        }
        NvstReceiveEvent::InputUnavailable(reason) => {
            let _ = output.send(event("input-unavailable", json!({ "reason": reason })));
            false
        }
        NvstReceiveEvent::Cursor(bytes) => {
            let _ = output.send(event(
                "cursor-update",
                json!({ "startId": pending_cursor_capture.start_id, "payloadBase64": BASE64.encode(bytes) }),
            ));
            false
        }
        NvstReceiveEvent::CursorCapture(composited) => {
            pending_cursor_capture.pending = Some(composited);
            flush_cursor_capture(output, lifecycle, generation, pending_cursor_capture);
            false
        }
        NvstReceiveEvent::Dropped(
            NvstDropReason::AwaitingStartOfFrame
            | NvstDropReason::StaleRtpPacket { .. }
            | NvstDropReason::DuplicateRtpPacket { .. },
        ) => {
            // These are expected while a packet-gap recovery waits for the
            // requested keyframe. Logging every following datagram can flood
            // stdout and steal time from the receive/decode threads.
            false
        }
        NvstReceiveEvent::Dropped(reason) => {
            let _ = output.send(event(
                "log",
                json!({ "level": "debug", "message": format!("Dropped NVST datagram: {reason:?}") }),
            ));
            false
        }
        NvstReceiveEvent::Frame(_) | NvstReceiveEvent::Wake => false,
    }
}

fn attempt_nvst_recovery<R: NvstSessionResources>(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    resources: &R,
    recovery_attempts: &mut usize,
    reason: String,
) -> bool {
    if *recovery_attempts >= NVST_RECOVERY_ATTEMPT_LIMIT {
        return emit_nvst_terminal(
            output,
            lifecycle,
            generation,
            resources,
            "nvst-recovery-exhausted",
            format!("NVST recovery failed after one attempt: {reason}"),
        );
    }

    *recovery_attempts += 1;
    resources.request_keyframe();
    if let Err(recovery_error) = resources.recover() {
        return emit_nvst_terminal(
            output,
            lifecycle,
            generation,
            resources,
            "nvst-recovery-failed",
            format!("NVST recovery could not be started: {recovery_error}"),
        );
    }
    let _ = output.send(event(
        "log",
        json!({
            "level": "warn",
            "message": format!("Attempting bounded NVST recovery with a fresh keyframe: {reason}")
        }),
    ));
    false
}

fn emit_nvst_terminal<R: NvstSessionResources>(
    output: &EventSender,
    lifecycle: &Mutex<Lifecycle>,
    generation: u64,
    resources: &R,
    code: &str,
    message: String,
) -> bool {
    {
        let mut lifecycle = lock_lifecycle(lifecycle);
        if lifecycle.generation != generation {
            return true;
        }
        lifecycle.context = None;
        lifecycle.state = State::Idle;
    }
    resources.stop();
    opennow_streamer_protocol::log::log_line("WARN", "transport", &format!("{code}: {message}"));
    let termination = json!({"source":"nvst-transport","code":code,"resumable":null});
    let _ = output.send(event(
        "error",
        json!({ "code": code, "message": &message, "termination": &termination }),
    ));
    let _ = output.send(event(
        "status",
        json!({ "status": "stopped", "message": message, "termination": termination }),
    ));
    true
}

fn frame_stage_timings_event(timings: Option<FrameStageTimings>) -> Value {
    let Some(timings) = timings else {
        return Value::Null;
    };
    let stage = |summary: Option<opennow_streamer_transport::StageSummary>| match summary {
        Some(summary) => json!({
            "p50": summary.p50_ms,
            "p95": summary.p95_ms,
            "max": summary.max_ms,
        }),
        None => Value::Null,
    };
    json!({
        "deliveryToAdmissionMs": stage(timings.delivery_to_admission),
        "admissionToControlQueueMs": stage(timings.admission_to_control_queue),
        "assembledToControlQueueMs": stage(timings.assembled_to_control_queue),
        "deliveryWindowSamples": timings.delivery_window_samples,
        "ackWindowSamples": timings.ack_window_samples,
        "assembledFramesTotal": timings.assembled_frames_total,
        "admittedFramesTotal": timings.admitted_frames_total,
        "queuedAckFramesTotal": timings.queued_ack_frames_total,
        "undeliveredFramesTotal": timings.undelivered_frames_total,
        "pendingDeliveries": timings.pending_deliveries,
        "unmatchedDeliveries": timings.unmatched_deliveries,
        "unmatchedAdmissions": timings.unmatched_admissions,
    })
}

struct NvstMediaFeedbackState {
    drop_reports: QueueDropReports,
    recovery_attempts: usize,
    input_origin: Instant,
    input_available: bool,
    telemetry_window_started: Instant,
    window_start_frames: u64,
    window_start_bytes: u64,
    telemetry_started: bool,
    previous_socket_receive_bytes: u64,
    peak_bitrate_mbps: f64,
    delivered_frames: u64,
    delivered_keyframes: u64,
    transport_frame_progress_stalled: bool,
    start_id: String,
}

impl NvstMediaFeedbackState {
    fn new(input_available: bool) -> Self {
        Self {
            drop_reports: QueueDropReports::new(),
            recovery_attempts: 0,
            input_origin: Instant::now(),
            input_available,
            telemetry_window_started: Instant::now(),
            window_start_frames: 0,
            window_start_bytes: 0,
            telemetry_started: false,
            previous_socket_receive_bytes: 0,
            peak_bitrate_mbps: 0.0,
            delivered_frames: 0,
            delivered_keyframes: 0,
            transport_frame_progress_stalled: false,
            start_id: String::new(),
        }
    }
}

fn flush_nvst_telemetry<R: NvstSessionResources>(
    output: &EventSender,
    resources: &R,
    state: &mut NvstMediaFeedbackState,
) {
    let elapsed = state.telemetry_window_started.elapsed();
    if elapsed < Duration::from_secs(1) {
        return;
    }
    let elapsed_seconds = elapsed.as_secs_f64();
    let delivered_frames = resources.delivered_frames();
    let delivered_bytes = resources.delivered_bytes();
    let frames_per_second =
        delivered_frames.saturating_sub(state.window_start_frames) as f64 / elapsed_seconds;
    let bitrate_mbps = delivered_bytes.saturating_sub(state.window_start_bytes) as f64 * 8.0
        / elapsed_seconds
        / 1_000_000.0;
    let socket_bytes = resources.socket_receive_bytes();
    let receive_bitrate_mbps = socket_bytes.and_then(|bytes| {
        bytes
            .checked_sub(state.previous_socket_receive_bytes)
            .map(|delta| delta as f64 * 8.0 / elapsed_seconds / 1_000_000.0)
    });
    if let Some(bytes) = socket_bytes {
        state.previous_socket_receive_bytes = bytes;
    }
    state.peak_bitrate_mbps = state.peak_bitrate_mbps.max(bitrate_mbps);
    let network = resources.network_metrics();
    let _ = output.send(event(
        "telemetry",
        json!({
            "framesPerSecond": frames_per_second,
            "bitrateMbps": bitrate_mbps,
            "receiveBitrateMbps": receive_bitrate_mbps,
            "peakBitrateMbps": state.peak_bitrate_mbps,
            "pingMs": resources.ping_ms(),
            "jitterMs": network.map(|metrics| metrics.0),
            "packetLossPercent": network.map(|metrics| metrics.1),
            "frameStageTimings": frame_stage_timings_event(resources.frame_stage_timings()),
            "transportFrameProgressStalled": state.transport_frame_progress_stalled,
            "startId": state.start_id,
        }),
    ));
    state.telemetry_window_started = Instant::now();
    state.window_start_frames = delivered_frames;
    state.window_start_bytes = delivered_bytes;
}

#[cfg(test)]
fn forward_nvst_captured_input<R: NvstSessionResources>(
    resources: &R,
    input: CapturedInput,
    state: &NvstMediaFeedbackState,
) -> Result<(), String> {
    let timestamp_us = u64::try_from(state.input_origin.elapsed().as_micros()).unwrap_or(u64::MAX);
    if let CapturedInput::Text(text) = input {
        return resources.send_captured_text(text, timestamp_us);
    }
    resources.send_captured_input(captured_input_packet(input, timestamp_us))
}

fn forward_nvst_captured_sample<R: NvstSessionResources>(
    resources: &R,
    sample: CapturedInputSample,
    state: &NvstMediaFeedbackState,
) -> Result<(), String> {
    // Bifrost timestamps native input at OS capture, before aggregation and
    // SCTP sending. Keeping that time prevents a delayed queue drain from
    // making a group of older reports look newly generated.
    let captured = sample
        .captured_at
        .checked_duration_since(state.input_origin)
        .unwrap_or_default();
    let timestamp_us = u64::try_from(captured.as_micros()).unwrap_or(u64::MAX);
    if let CapturedInput::Text(text) = sample.input {
        return resources.send_captured_text(text, timestamp_us);
    }
    resources.send_captured_input(captured_input_packet(sample.input, timestamp_us))
}

fn captured_input_packet(input: CapturedInput, timestamp_us: u64) -> Vec<u8> {
    match input {
        CapturedInput::Text(_) => unreachable!("text uses typed transport submission"),
        CapturedInput::Key {
            virtual_key,
            modifiers,
            pressed,
        } => {
            let mut packet = Vec::with_capacity(18);
            packet.extend_from_slice(&(if pressed { 3_u32 } else { 4_u32 }).to_le_bytes());
            packet.extend_from_slice(&virtual_key.to_be_bytes());
            packet.extend_from_slice(&modifiers.to_be_bytes());
            packet.extend_from_slice(&0_u16.to_be_bytes());
            packet.extend_from_slice(&timestamp_us.to_be_bytes());
            packet
        }
        CapturedInput::MouseMove { delta_x, delta_y } => {
            let (delta_x, delta_y) = tune_relative_mouse(delta_x, delta_y, input_tuning());
            let mut packet = Vec::with_capacity(22);
            packet.extend_from_slice(&7_u32.to_le_bytes());
            packet.extend_from_slice(&delta_x.to_be_bytes());
            packet.extend_from_slice(&delta_y.to_be_bytes());
            packet.extend_from_slice(&[0; 6]);
            packet.extend_from_slice(&timestamp_us.to_be_bytes());
            packet
        }
        CapturedInput::MouseAbsolute {
            x,
            y,
            width,
            height,
        } => {
            let mut packet = Vec::with_capacity(26);
            packet.extend_from_slice(&5_u32.to_le_bytes());
            packet.extend_from_slice(&x.to_be_bytes());
            packet.extend_from_slice(&y.to_be_bytes());
            packet.extend_from_slice(&0_u16.to_be_bytes());
            packet.extend_from_slice(&width.to_be_bytes());
            packet.extend_from_slice(&height.to_be_bytes());
            packet.extend_from_slice(&0_u32.to_be_bytes());
            packet.extend_from_slice(&timestamp_us.to_be_bytes());
            packet
        }
        CapturedInput::MouseButton { button, pressed } => {
            let mut packet = Vec::with_capacity(18);
            packet.extend_from_slice(&(if pressed { 8_u32 } else { 9_u32 }).to_le_bytes());
            packet.extend_from_slice(&[button, 0]);
            packet.extend_from_slice(&[0; 4]);
            packet.extend_from_slice(&timestamp_us.to_be_bytes());
            packet
        }
        CapturedInput::MouseWheel { delta_x, delta_y } => {
            let mut packet = Vec::with_capacity(22);
            packet.extend_from_slice(&10_u32.to_le_bytes());
            packet.extend_from_slice(&delta_x.to_be_bytes());
            packet.extend_from_slice(&delta_y.to_be_bytes());
            packet.extend_from_slice(&[0; 6]);
            packet.extend_from_slice(&timestamp_us.to_be_bytes());
            packet
        }
        CapturedInput::Gamepad {
            controller_id,
            bitmap,
            buttons,
            left_trigger,
            right_trigger,
            left_stick_x,
            left_stick_y,
            right_stick_x,
            right_stick_y,
        } => {
            let mut packet = Vec::with_capacity(38);
            packet.extend_from_slice(&12_u32.to_le_bytes());
            packet.extend_from_slice(&26_u16.to_le_bytes());
            packet.extend_from_slice(&u16::from(controller_id & 0x03).to_le_bytes());
            packet.extend_from_slice(&bitmap.to_le_bytes());
            packet.extend_from_slice(&20_u16.to_le_bytes());
            packet.extend_from_slice(&buttons.to_le_bytes());
            packet.extend_from_slice(
                &(u16::from(left_trigger) | (u16::from(right_trigger) << 8)).to_le_bytes(),
            );
            packet.extend_from_slice(&left_stick_x.to_le_bytes());
            packet.extend_from_slice(&left_stick_y.to_le_bytes());
            packet.extend_from_slice(&right_stick_x.to_le_bytes());
            packet.extend_from_slice(&right_stick_y.to_le_bytes());
            packet.extend_from_slice(&0_u16.to_le_bytes());
            packet.extend_from_slice(&85_u16.to_le_bytes());
            packet.extend_from_slice(&0_u16.to_le_bytes());
            packet.extend_from_slice(&timestamp_us.to_le_bytes());
            packet
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct InputTuning {
    sensitivity: f64,
    acceleration_percent: f64,
}

fn input_tuning() -> InputTuning {
    static TUNING: OnceLock<InputTuning> = OnceLock::new();
    *TUNING.get_or_init(|| InputTuning {
        sensitivity: std::env::var("OPENNOW_MOUSE_SENSITIVITY")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(0.1, 3.0),
        acceleration_percent: std::env::var("OPENNOW_MOUSE_ACCELERATION")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(1.0, 150.0),
    })
}

fn tune_relative_mouse(delta_x: i16, delta_y: i16, tuning: InputTuning) -> (i16, i16) {
    let mut x = f64::from(delta_x) * tuning.sensitivity;
    let mut y = f64::from(delta_y) * tuning.sensitivity;
    if tuning.acceleration_percent > 1.0 {
        let speed = x.hypot(y);
        let strength = (tuning.acceleration_percent - 1.0) / 149.0;
        // Match the legacy client curve: preserve low-speed precision and cap
        // the maximum turn boost at 60% for the 150% setting.
        let factor = 1.0 + (0.6 * strength).min(speed / 50.0 * strength);
        x *= factor;
        y *= factor;
    }
    let clamp = |value: f64| {
        value
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
    };
    (clamp(x), clamp(y))
}

fn media_stream_config(context: &SessionContext) -> MediaStreamConfig {
    let codec_name = context
        .session
        .extra
        .get("negotiatedStreamProfile")
        .and_then(|profile| profile.get("codec"))
        .and_then(Value::as_str)
        .or_else(|| context.settings.get("codec").and_then(Value::as_str))
        .unwrap_or("H264");
    let codec = match codec_name.trim().to_ascii_uppercase().as_str() {
        "H265" | "HEVC" => MediaVideoCodec::H265,
        "AV1" => MediaVideoCodec::Av1,
        _ => MediaVideoCodec::H264,
    };
    let color_quality_name = context
        .session
        .extra
        .get("negotiatedStreamProfile")
        .and_then(|profile| profile.get("colorQuality"))
        .and_then(Value::as_str)
        .or_else(|| context.settings.get("colorQuality").and_then(Value::as_str))
        .unwrap_or("8bit_420");
    let color_quality = match color_quality_name.trim().to_ascii_lowercase().as_str() {
        "8bit_444" if codec == MediaVideoCodec::H265 => MediaColorQuality::EightBit444,
        "10bit_420" if codec != MediaVideoCodec::H264 => MediaColorQuality::TenBit420,
        "10bit_444" if codec == MediaVideoCodec::H265 => MediaColorQuality::TenBit444,
        "10bit_444" if codec == MediaVideoCodec::Av1 => MediaColorQuality::TenBit420,
        _ => MediaColorQuality::EightBit420,
    };
    let resolution = context
        .session
        .extra
        .get("negotiatedStreamProfile")
        .and_then(|profile| profile.get("resolution"))
        .and_then(Value::as_str)
        .or_else(|| context.settings.get("resolution").and_then(Value::as_str))
        .and_then(|value| {
            let lowercase = value.to_ascii_lowercase();
            let (width, height) = lowercase.split_once('x')?;
            Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
        })
        .filter(|(width, height)| (48..=4096).contains(width) && (48..=2304).contains(height))
        .unwrap_or((1920, 1080));
    let fps = context
        .session
        .extra
        .get("negotiatedStreamProfile")
        .and_then(|profile| profile.get("fps"))
        .or_else(|| context.settings.get("fps"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(60)
        .clamp(1, MAX_STREAM_FPS);
    let bitrate_mbps = context
        .settings
        .get("maxBitrateMbps")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(75.0)
        .clamp(0.22, 200.0);
    let bitrate_bps = (bitrate_mbps * 1_000_000.0)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    let hdr = context
        .session
        .extra
        .get("negotiatedStreamProfile")
        .and_then(|profile| profile.get("enableHdr"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    MediaStreamConfig {
        codec,
        color_quality,
        hdr,
        width: resolution.0,
        height: resolution.1,
        fps,
        bitrate_bps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Instant;

    fn command(value: Value) -> Command {
        serde_json::from_value(value).expect("command")
    }

    fn test_engine(events: Sender<Value>) -> Engine {
        let (media_sender, _media_receiver) = std::sync::mpsc::sync_channel(4);
        Engine::with_media_consumer(events, media_sender)
    }

    fn synthetic_context(session_id: &str, ice_servers: Value) -> Value {
        json!({
            "session": {
                "sessionId": session_id,
                "serverIp": "127-0-0-1.synthetic.invalid",
                "iceServers": ice_servers,
                "mediaConnectionInfo": {
                    "ip": "127-0-0-1.media.synthetic.invalid",
                    "port": 18_784,
                    "usage": 17
                },
                "syntheticExtension": "preserved"
            },
            "settings": { "codec": "H264", "fps": 60 },
            "shortcuts": { "stopStream": "Ctrl+Shift+Q" },
            "syntheticContextExtension": true
        })
    }

    fn lifecycle_state(engine: &Engine) -> State {
        lock_lifecycle(&engine.lifecycle).state
    }

    #[test]
    fn irrelevant_cloudmatch_connections_do_not_reject_a_valid_media_endpoint() {
        let mut context = synthetic_context("seat", json!([]));
        context["session"]["connectionInfo"] = json!([
            {"usage":15,"ip":"unused.example","port":0},
            {"usage":14,"ip":"signaling.example","port":322}
        ]);
        context["session"]["mediaConnectionInfo"] =
            json!({"ip":"203.0.113.20","port":5004,"usage":17});
        let context: SessionContext = serde_json::from_value(context).unwrap();
        assert!(validate_context(&context, "start").is_ok());

        let mut invalid_media = context;
        invalid_media
            .session
            .media_connection_info
            .as_mut()
            .unwrap()
            .port = 0;
        assert_eq!(
            validate_context(&invalid_media, "start").unwrap_err()["code"],
            "invalid-context"
        );
    }

    fn unused_udp_port() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("ephemeral UDP port");
        let port = socket.local_addr().expect("socket address").port();
        drop(socket);
        port
    }

    #[derive(Default)]
    struct TestNvstResources {
        rumble: Arc<Mutex<[Option<NvstControllerRumble>; 4]>>,
        rumble_coalesced: Arc<AtomicUsize>,
        ping_ms: Option<f64>,
        socket_bytes: Option<Arc<AtomicU64>>,
        frame_stage_timings: Option<FrameStageTimings>,
        keyframe_requests: Arc<AtomicUsize>,
        recoveries: Arc<AtomicUsize>,
        stops: AtomicUsize,
        captured_inputs: Arc<Mutex<Vec<Vec<u8>>>>,
        delivered_frames: Arc<AtomicU64>,
        delivered_bytes: Arc<AtomicU64>,
        delivered_keyframes: Arc<AtomicU64>,
    }

    impl NvstSessionResources for TestNvstResources {
        fn take_rumble(&self) -> ([Option<NvstControllerRumble>; 4], usize) {
            (
                std::mem::take(&mut *self.rumble.lock().unwrap()),
                self.rumble_coalesced.swap(0, Ordering::Relaxed),
            )
        }
        fn ping_ms(&self) -> Option<f64> {
            self.ping_ms
        }

        fn socket_receive_bytes(&self) -> Option<u64> {
            self.socket_bytes
                .as_ref()
                .map(|bytes| bytes.load(Ordering::Relaxed))
        }

        fn frame_stage_timings(&self) -> Option<FrameStageTimings> {
            self.frame_stage_timings
        }

        fn delivered_frames(&self) -> u64 {
            self.delivered_frames.load(Ordering::Relaxed)
        }

        fn delivered_bytes(&self) -> u64 {
            self.delivered_bytes.load(Ordering::Relaxed)
        }

        fn delivered_keyframes(&self) -> u64 {
            self.delivered_keyframes.load(Ordering::Relaxed)
        }

        fn request_keyframe(&self) {
            self.keyframe_requests.fetch_add(1, Ordering::Relaxed);
        }

        fn send_captured_input(&self, bytes: Vec<u8>) -> Result<(), String> {
            self.captured_inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(bytes);
            Ok(())
        }

        fn send_captured_text(
            &self,
            text: opennow_streamer_protocol::text_input::UnicodeText,
            _timestamp_us: u64,
        ) -> Result<(), String> {
            self.captured_inputs
                .lock()
                .unwrap()
                .push(text.as_str().as_bytes().to_vec());
            Ok(())
        }

        fn recover(&self) -> Result<(), String> {
            self.recoveries.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn connected_lifecycle() -> Mutex<Lifecycle> {
        Mutex::new(Lifecycle {
            state: State::Connected,
            context: Some(
                serde_json::from_value(synthetic_context("nvst-recovery", json!([])))
                    .expect("session context"),
            ),
            generation: 7,
        })
    }

    #[test]
    fn delivered_keyframe_resets_the_recovery_budget_and_clears_the_frame_stall() {
        let resources = TestNvstResources::default();
        let mut state = NvstMediaFeedbackState::new(true);
        state.recovery_attempts = 1;
        state.transport_frame_progress_stalled = true;

        resources.delivered_frames.store(1, Ordering::Relaxed);
        observe_delivered_frames(&resources, &mut state);
        assert_eq!(state.recovery_attempts, 1);
        assert!(!state.transport_frame_progress_stalled);
        assert!(state.telemetry_started);

        resources.delivered_frames.store(2, Ordering::Relaxed);
        resources.delivered_keyframes.store(1, Ordering::Relaxed);
        observe_delivered_frames(&resources, &mut state);
        assert_eq!(state.recovery_attempts, 0);

        state.recovery_attempts = 1;
        observe_delivered_frames(&resources, &mut state);
        assert_eq!(
            state.recovery_attempts, 1,
            "a keyframe that was already counted does not reset the budget again"
        );
    }

    #[test]
    fn delivered_frames_emit_bounded_telemetry() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let resources = TestNvstResources::default();
        let mut state = NvstMediaFeedbackState::new(true);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        resources.delivered_frames.store(1, Ordering::Relaxed);
        resources.delivered_bytes.store(125_000, Ordering::Relaxed);

        flush_nvst_telemetry(&sender, &resources, &mut state);

        let telemetry = receiver.recv().expect("stream telemetry");
        assert_eq!(telemetry["type"], "telemetry");
        assert!(
            telemetry["framesPerSecond"]
                .as_f64()
                .is_some_and(|value| value > 0.0)
        );
        assert!(
            telemetry["bitrateMbps"]
                .as_f64()
                .is_some_and(|value| (0.9..=1.0).contains(&value))
        );
        assert_eq!(telemetry["peakBitrateMbps"], telemetry["bitrateMbps"]);
    }

    #[test]
    fn telemetry_rates_are_per_window_and_not_repeated_from_a_stale_window() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let resources = TestNvstResources::default();
        let mut state = NvstMediaFeedbackState::new(true);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        resources.delivered_frames.store(1, Ordering::Relaxed);
        resources
            .delivered_bytes
            .store(1_000_000, Ordering::Relaxed);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        let measured = receiver.try_recv().expect("frame window telemetry");
        assert!(
            measured["framesPerSecond"].as_f64().expect("rate") > 0.0,
            "a window with a delivered frame reports a measured rate"
        );
        assert!(measured["bitrateMbps"].as_f64().expect("bitrate") > 0.0);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        let stalled = receiver.try_recv().expect("idle window telemetry");
        assert_eq!(
            stalled["framesPerSecond"],
            json!(0.0),
            "the refreshed window reports its own frames, not the previous window's rate"
        );
        assert_eq!(stalled["bitrateMbps"], json!(0.0));
    }

    #[test]
    fn delivered_telemetry_preserves_measured_and_unavailable_ping() {
        for ping_ms in [None, Some(0.0), Some(25.5)] {
            let (sender, receiver) = std::sync::mpsc::channel();
            let sender = EventSender::unbounded(sender);
            let resources = TestNvstResources {
                ping_ms,
                ..Default::default()
            };
            let mut state = NvstMediaFeedbackState::new(true);
            state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
            resources.delivered_frames.store(1, Ordering::Relaxed);
            resources.delivered_bytes.store(125_000, Ordering::Relaxed);
            flush_nvst_telemetry(&sender, &resources, &mut state);
            let telemetry = receiver.recv().unwrap();
            assert_eq!(telemetry["type"], "telemetry");
            assert!(telemetry.get("pingMs").is_some());
            assert_eq!(telemetry["pingMs"], json!(ping_ms));
            assert_eq!(telemetry["frameStageTimings"], json!(null));
        }
    }

    #[test]
    fn socket_receive_rate_tracks_cumulative_bytes_through_idle_and_reset() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let bytes = Arc::new(AtomicU64::new(50_000));
        let resources = TestNvstResources {
            socket_bytes: Some(Arc::clone(&bytes)),
            ..Default::default()
        };
        let mut state = NvstMediaFeedbackState::new(true);
        state.previous_socket_receive_bytes = resources.socket_receive_bytes().unwrap();
        bytes.store(300_000, Ordering::Relaxed);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(2);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        let first = receiver.recv().unwrap();
        assert!((first["receiveBitrateMbps"].as_f64().unwrap() - 1.0).abs() < 0.01);
        assert_eq!(first["bitrateMbps"], json!(0.0));

        bytes.store(550_000, Ordering::Relaxed);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        let second = receiver.recv().unwrap();
        assert!((second["receiveBitrateMbps"].as_f64().unwrap() - 2.0).abs() < 0.02);

        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        assert_eq!(receiver.recv().unwrap()["receiveBitrateMbps"], json!(0.0));

        bytes.store(100, Ordering::Relaxed);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        assert_eq!(receiver.recv().unwrap()["receiveBitrateMbps"], Value::Null);

        bytes.store(125_100, Ordering::Relaxed);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        assert!(
            (receiver.recv().unwrap()["receiveBitrateMbps"]
                .as_f64()
                .unwrap()
                - 1.0)
                .abs()
                < 0.01
        );

        let mut next_session = NvstMediaFeedbackState::new(true);
        let new_resources = TestNvstResources {
            socket_bytes: Some(Arc::new(AtomicU64::new(0))),
            ..Default::default()
        };
        next_session.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        flush_nvst_telemetry(&sender, &new_resources, &mut next_session);
        assert_eq!(receiver.recv().unwrap()["receiveBitrateMbps"], json!(0.0));
    }

    #[test]
    fn telemetry_reports_only_measured_frame_stage_timings() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let resources = TestNvstResources {
            frame_stage_timings: Some(FrameStageTimings {
                delivery_to_admission: Some(opennow_streamer_transport::StageSummary {
                    p50_ms: 3.5,
                    p95_ms: 8.25,
                    max_ms: 11.0,
                }),
                admission_to_control_queue: Some(opennow_streamer_transport::StageSummary {
                    p50_ms: 1.5,
                    p95_ms: 4.0,
                    max_ms: 6.0,
                }),
                assembled_to_control_queue: Some(opennow_streamer_transport::StageSummary {
                    p50_ms: 5.0,
                    p95_ms: 12.0,
                    max_ms: 17.0,
                }),
                delivery_window_samples: 42,
                ack_window_samples: 40,
                assembled_frames_total: 900,
                admitted_frames_total: 897,
                queued_ack_frames_total: 896,
                pending_deliveries: 2,
                unmatched_deliveries: 1,
                unmatched_admissions: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut state = NvstMediaFeedbackState::new(true);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        resources.delivered_frames.store(1, Ordering::Relaxed);
        resources.delivered_bytes.store(125_000, Ordering::Relaxed);
        flush_nvst_telemetry(&sender, &resources, &mut state);
        let telemetry = receiver
            .try_recv()
            .expect("frame stage telemetry for the completed window");
        let timings = &telemetry["frameStageTimings"];
        assert_eq!(timings["deliveryToAdmissionMs"]["p50"], json!(3.5));
        assert_eq!(timings["deliveryToAdmissionMs"]["p95"], json!(8.25));
        assert_eq!(timings["admissionToControlQueueMs"]["max"], json!(6.0));
        assert_eq!(timings["assembledToControlQueueMs"]["p95"], json!(12.0));
        assert_eq!(timings["deliveryWindowSamples"], json!(42));
        assert_eq!(timings["ackWindowSamples"], json!(40));
        assert_eq!(timings["assembledFramesTotal"], json!(900));
        assert_eq!(timings["admittedFramesTotal"], json!(897));
        assert_eq!(timings["queuedAckFramesTotal"], json!(896));
        assert_eq!(timings["pendingDeliveries"], json!(2));
        assert_eq!(timings["unmatchedDeliveries"], json!(1));
        assert_eq!(timings["unmatchedAdmissions"], json!(3));
    }

    #[test]
    fn frame_stage_timings_event_keeps_unmeasured_stages_null() {
        assert_eq!(frame_stage_timings_event(None), json!(null));
        let timings = frame_stage_timings_event(Some(FrameStageTimings {
            admission_to_control_queue: Some(opennow_streamer_transport::StageSummary {
                p50_ms: 2.0,
                p95_ms: 2.5,
                max_ms: 3.0,
            }),
            ack_window_samples: 1,
            ..Default::default()
        }));
        assert_eq!(timings["deliveryToAdmissionMs"], json!(null));
        assert_eq!(timings["assembledToControlQueueMs"], json!(null));
        assert_eq!(timings["admissionToControlQueueMs"]["p50"], json!(2.0));
        assert_eq!(timings["deliveryWindowSamples"], json!(0));
        assert_eq!(timings["assembledFramesTotal"], json!(0));
        assert_eq!(timings["pendingDeliveries"], json!(0));
    }

    #[test]
    fn active_video_telemetry_requires_a_network_ping_sample() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let config = parse_nvst_video_handoff(&json!({
            "nvstVideo": {
                "clientUdpPort": socket.local_addr().unwrap().port(),
                "videoPeerIp": "127.0.0.1",
                "videoPeerPort": peer.local_addr().unwrap().port(),
                "srtpAesKeyHex": "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F",
                "srtpSaltHex": "00000000000000009ECA935E",
                "codec": "H264"
            }
        }))
        .unwrap()
        .unwrap();
        let feedback = config.feedback();
        let (media_sender, _media_receiver) = std::sync::mpsc::sync_channel(1);
        let (event_sender, _event_receiver) = std::sync::mpsc::channel();
        let transport = spawn_nvst_udp_receiver_with_socket(
            config,
            media_sender,
            event_sender,
            Some(socket),
            None,
            Arc::new(HidRuntime::new()),
            None,
        )
        .unwrap();
        let resources = ActiveNvstResources {
            bundle: transport.control(),
            mjolnir: None,
            feedback: Arc::clone(&feedback),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut state = NvstMediaFeedbackState::new(true);
        state.telemetry_window_started = Instant::now() - Duration::from_secs(1);
        feedback.record_delivered_frame(72, 125_000, false, Instant::now());
        flush_nvst_telemetry(&EventSender::unbounded(sender), &resources, &mut state);
        let telemetry = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        transport.stop();
        assert_eq!(telemetry["type"], "telemetry");
        assert!(telemetry["framesPerSecond"].as_f64().unwrap() > 0.0);
        assert_eq!(telemetry.get("pingMs"), Some(&Value::Null));
    }

    #[test]
    fn captured_sdl_input_routes_through_the_nvst_input_codec_packet_shape() {
        let resources = TestNvstResources::default();
        let state = NvstMediaFeedbackState::new(true);

        assert!(
            forward_nvst_captured_input(
                &resources,
                CapturedInput::Key {
                    virtual_key: 0x57,
                    modifiers: 0x01,
                    pressed: true,
                },
                &state,
            )
            .is_ok()
        );
        assert!(
            forward_nvst_captured_input(
                &resources,
                CapturedInput::MouseMove {
                    delta_x: -12,
                    delta_y: 34,
                },
                &state,
            )
            .is_ok()
        );
        assert!(
            forward_nvst_captured_input(
                &resources,
                CapturedInput::MouseAbsolute {
                    x: 321,
                    y: 180,
                    width: 1280,
                    height: 720,
                },
                &state,
            )
            .is_ok()
        );

        let inputs = resources
            .captured_inputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(inputs.len(), 3);
        assert_eq!(u32::from_le_bytes(inputs[0][0..4].try_into().unwrap()), 3);
        assert_eq!(
            u16::from_be_bytes(inputs[0][4..6].try_into().unwrap()),
            0x57
        );
        assert_eq!(
            u16::from_be_bytes(inputs[0][6..8].try_into().unwrap()),
            0x01
        );
        assert_eq!(inputs[0].len(), 18);
        assert_eq!(u32::from_le_bytes(inputs[1][0..4].try_into().unwrap()), 7);
        assert_eq!(i16::from_be_bytes(inputs[1][4..6].try_into().unwrap()), -12);
        assert_eq!(i16::from_be_bytes(inputs[1][6..8].try_into().unwrap()), 34);
        assert_eq!(inputs[1].len(), 22);
        assert_eq!(u32::from_le_bytes(inputs[2][0..4].try_into().unwrap()), 5);
        assert_eq!(u16::from_be_bytes(inputs[2][4..6].try_into().unwrap()), 321);
        assert_eq!(u16::from_be_bytes(inputs[2][6..8].try_into().unwrap()), 180);
        assert_eq!(
            u16::from_be_bytes(inputs[2][10..12].try_into().unwrap()),
            1280
        );
        assert_eq!(
            u16::from_be_bytes(inputs[2][12..14].try_into().unwrap()),
            720
        );
        assert_eq!(inputs[2].len(), 26);
    }

    #[test]
    fn captured_input_preserves_the_os_capture_timestamp() {
        let resources = TestNvstResources::default();
        let input_origin = Instant::now();
        let mut state = NvstMediaFeedbackState::new(true);
        state.input_origin = input_origin;
        forward_nvst_captured_sample(
            &resources,
            CapturedInputSample {
                input: CapturedInput::MouseMove {
                    delta_x: 1,
                    delta_y: -1,
                },
                captured_at: input_origin + Duration::from_micros(4_242),
            },
            &state,
        )
        .expect("captured input");

        let inputs = resources
            .captured_inputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            u64::from_be_bytes(inputs[0][14..22].try_into().unwrap()),
            4_242
        );
    }

    #[test]
    fn captured_unicode_text_uses_typed_transport_without_key_expansion() {
        use opennow_streamer_protocol::text_input::TextInputSlot;
        let resources = TestNvstResources::default();
        let state = NvstMediaFeedbackState::new(true);
        let text = TextInputSlot::default()
            .submit("é世界🦫".as_bytes())
            .unwrap();
        forward_nvst_captured_sample(
            &resources,
            CapturedInputSample {
                input: CapturedInput::Text(text),
                captured_at: state.input_origin,
            },
            &state,
        )
        .unwrap();
        assert_eq!(
            *resources.captured_inputs.lock().unwrap(),
            vec!["é世界🦫".as_bytes().to_vec()]
        );
    }

    #[test]
    fn captured_gamepad_matches_the_official_38_byte_packet() {
        let packet = captured_input_packet(
            CapturedInput::Gamepad {
                controller_id: 2,
                bitmap: 0x0404,
                buttons: 0x5101,
                left_trigger: 17,
                right_trigger: 231,
                left_stick_x: -12_345,
                left_stick_y: 23_456,
                right_stick_x: -30_000,
                right_stick_y: 30_001,
            },
            0x0102_0304_0506_0708,
        );

        assert_eq!(packet.len(), 38);
        assert_eq!(u32::from_le_bytes(packet[0..4].try_into().unwrap()), 12);
        assert_eq!(u16::from_le_bytes(packet[4..6].try_into().unwrap()), 26);
        assert_eq!(u16::from_le_bytes(packet[6..8].try_into().unwrap()), 2);
        assert_eq!(
            u16::from_le_bytes(packet[8..10].try_into().unwrap()),
            0x0404
        );
        assert_eq!(u16::from_le_bytes(packet[10..12].try_into().unwrap()), 20);
        assert_eq!(
            u16::from_le_bytes(packet[12..14].try_into().unwrap()),
            0x5101
        );
        assert_eq!(
            u16::from_le_bytes(packet[14..16].try_into().unwrap()),
            0xe711
        );
        assert_eq!(
            i16::from_le_bytes(packet[16..18].try_into().unwrap()),
            -12_345
        );
        assert_eq!(
            i16::from_le_bytes(packet[18..20].try_into().unwrap()),
            23_456
        );
        assert_eq!(
            i16::from_le_bytes(packet[20..22].try_into().unwrap()),
            -30_000
        );
        assert_eq!(
            i16::from_le_bytes(packet[22..24].try_into().unwrap()),
            30_001
        );
        assert_eq!(u16::from_le_bytes(packet[26..28].try_into().unwrap()), 85);
        assert_eq!(
            u64::from_le_bytes(packet[30..38].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
    }

    #[test]
    fn relative_mouse_tuning_matches_sensitivity_and_bounded_acceleration() {
        assert_eq!(
            tune_relative_mouse(
                20,
                -10,
                InputTuning {
                    sensitivity: 0.5,
                    acceleration_percent: 1.0,
                },
            ),
            (10, -5)
        );
        let accelerated = tune_relative_mouse(
            100,
            0,
            InputTuning {
                sensitivity: 1.0,
                acceleration_percent: 150.0,
            },
        );
        assert_eq!(accelerated, (160, 0));
        assert_eq!(
            tune_relative_mouse(
                i16::MAX,
                i16::MIN,
                InputTuning {
                    sensitivity: 3.0,
                    acceleration_percent: 150.0,
                },
            ),
            (i16::MAX, i16::MIN)
        );
    }

    #[test]
    fn rumble_stop_is_retried_after_event_queue_backpressure() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        sender.send(json!({"type":"busy"})).unwrap();
        let output = EventSender::bounded(sender);
        let lifecycle = Arc::new(connected_lifecycle());
        let resources = TestNvstResources::default();
        resources.rumble.lock().unwrap()[2] = Some(NvstControllerRumble {
            controller_id: 2,
            low_frequency: 0,
            high_frequency: 0,
            duration_ms: 1000,
            source_incarnation: None,
        });
        let pending = resources.rumble.clone();
        let worker_lifecycle = lifecycle.clone();
        let (_sender, nvst_events) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            forward_nvst_session_events(
                &output,
                &worker_lifecycle,
                7,
                NvstSessionEventResources {
                    start_id: "start-7".to_owned(),
                    nvst_events,
                    captured_input: Arc::new(CapturedInputQueue::default()),
                    transport: resources,
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while pending.lock().unwrap()[2].is_some() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(3)).unwrap()["type"],
            "busy"
        );
        let stop = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(stop["type"], "controller-rumble");
        assert_eq!(stop["controllerId"], 2);
        assert_eq!(stop["lowFrequency"], 0);
        assert_eq!(stop["highFrequency"], 0);
        lock_lifecycle(&lifecycle).generation += 1;
        worker.join().unwrap();
    }

    #[test]
    fn rumble_events_are_typed_session_scoped_and_nonblocking() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let output = EventSender::bounded(sender);
        let lifecycle = connected_lifecycle();
        let command = opennow_streamer_transport::NvstControllerRumble {
            controller_id: 3,
            low_frequency: 65535,
            high_frequency: 12345,
            duration_ms: 65535,
            source_incarnation: None,
        };
        assert!(forward_controller_rumble(
            &output, &lifecycle, 7, "start-7", command
        ));
        assert!(!forward_controller_rumble(
            &output, &lifecycle, 7, "start-7", command
        ));
        assert_eq!(
            receiver.try_recv().unwrap(),
            json!({"type":"controller-rumble",
            "startId":"start-7", "controllerId":3, "lowFrequency":65535,
            "highFrequency":12345, "durationMs":65535})
        );
        assert!(forward_controller_rumble(
            &output,
            &lifecycle,
            6,
            "old-start",
            command
        ));
        lock_lifecycle(&lifecycle).state = State::Idle;
        assert!(forward_controller_rumble(
            &output, &lifecycle, 7, "start-7", command
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn sony_rumble_carries_the_source_incarnation_and_stays_session_scoped() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let output = EventSender::bounded(sender);
        let lifecycle = connected_lifecycle();
        assert!(forward_controller_rumble(
            &output,
            &lifecycle,
            7,
            "start-7",
            opennow_streamer_transport::NvstControllerRumble {
                controller_id: 1,
                low_frequency: 0x4000,
                high_frequency: 0x8000,
                duration_ms: 0,
                source_incarnation: Some(91),
            }
        ));
        assert_eq!(
            receiver.try_recv().expect("sony rumble event"),
            json!({"type":"controller-rumble",
            "startId":"start-7", "controllerId":1, "lowFrequency":0x4000,
            "highFrequency":0x8000, "durationMs":0, "sourceIncarnation":91})
        );
    }

    #[test]
    fn nvst_recovery_is_attempted_once_with_pli() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;

        let terminal = forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut NvstCursorCaptureOutput::default(),
            NvstReceiveEvent::RecoveryNeeded(opennow_streamer_transport::NvstRecovery::Timeout {
                idle_for: Duration::from_secs(2),
            }),
        );

        assert!(!terminal);
        assert_eq!(recovery_attempts, 1);
        assert_eq!(resources.recoveries.load(Ordering::Relaxed), 1);
        assert_eq!(resources.keyframe_requests.load(Ordering::Relaxed), 1);
        assert_eq!(resources.stops.load(Ordering::Relaxed), 0);
        assert_eq!(lock_lifecycle(&lifecycle).state, State::Connected);
        assert!(
            receiver
                .try_iter()
                .all(|message| message["type"] != "error")
        );
    }

    #[test]
    fn cursor_capture_output_survives_setup_before_running() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = EventSender::bounded(sender);
        let lifecycle = connected_lifecycle();
        lock_lifecycle(&lifecycle).state = State::Idle;
        let mut state = NvstCursorCaptureOutput {
            start_id: "cursor-setup".to_owned(),
            pending: Some(false),
        };
        sender.send(json!({ "type": "log" })).unwrap();
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert_eq!(state.pending, Some(false));
        receiver.try_recv().unwrap();
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert_eq!(state.pending, None);
        assert_eq!(receiver.try_recv().unwrap()["composited"], false);
        lock_lifecycle(&lifecycle).state = State::Connected;
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert!(receiver.try_recv().is_err());
        state.pending = Some(true);
        lock_lifecycle(&lifecycle).context = None;
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert_eq!(state.pending, None);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn cursor_capture_output_retries_latest_state_after_backpressure() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = EventSender::bounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;
        let mut state = NvstCursorCaptureOutput {
            start_id: "cursor-session".to_owned(),
            pending: None,
        };
        sender.send(json!({ "type": "log" })).unwrap();
        for composited in [true, false] {
            assert!(!forward_nvst_event(
                &sender,
                &lifecycle,
                7,
                &resources,
                &mut recovery_attempts,
                &mut state,
                NvstReceiveEvent::CursorCapture(composited),
            ));
            assert_eq!(state.pending, Some(composited));
        }
        assert_eq!(receiver.try_recv().unwrap()["type"], "log");
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert_eq!(state.pending, None);
        let message = receiver.try_recv().unwrap();
        assert_eq!(message["type"], "cursor-capture");
        assert_eq!(message["startId"], "cursor-session");
        assert_eq!(message["composited"], false);
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert!(receiver.try_recv().is_err());
        state.pending = Some(true);
        lock_lifecycle(&lifecycle).generation += 1;
        flush_cursor_capture(&sender, &lifecycle, 7, &mut state);
        assert_eq!(state.pending, None);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn cursor_capture_events_preserve_composition_across_reactivation() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;

        for composited in [true, false, true, false] {
            assert!(!forward_nvst_event(
                &sender,
                &lifecycle,
                7,
                &resources,
                &mut recovery_attempts,
                &mut NvstCursorCaptureOutput::default(),
                NvstReceiveEvent::CursorCapture(composited),
            ));
            let message = receiver.try_recv().expect("cursor composition event");
            assert_eq!(message["type"], "cursor-capture");
            assert_eq!(message["composited"], composited);
        }
        assert_eq!(resources.stops.load(Ordering::Relaxed), 0);
        assert_eq!(resources.keyframe_requests.load(Ordering::Relaxed), 0);
        assert_eq!(resources.recoveries.load(Ordering::Relaxed), 0);
        assert_eq!(lock_lifecycle(&lifecycle).state, State::Connected);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn repeated_packet_gaps_request_keyframes_without_stopping_the_session() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;

        for first_missing_index in [100, 200] {
            assert!(!forward_nvst_event(
                &sender,
                &lifecycle,
                7,
                &resources,
                &mut recovery_attempts,
                &mut NvstCursorCaptureOutput::default(),
                NvstReceiveEvent::RecoveryNeeded(NvstRecovery::PacketGap {
                    first_missing_index,
                    last_missing_index: first_missing_index + 31,
                }),
            ));
        }

        assert_eq!(recovery_attempts, 0);
        assert_eq!(resources.recoveries.load(Ordering::Relaxed), 0);
        assert_eq!(resources.keyframe_requests.load(Ordering::Relaxed), 2);
        assert_eq!(resources.stops.load(Ordering::Relaxed), 0);
        assert_eq!(lock_lifecycle(&lifecycle).state, State::Connected);
        assert!(
            receiver
                .try_iter()
                .all(|message| message["type"] != "error")
        );
    }

    #[test]
    fn transient_media_backpressure_requests_keyframe_without_stopping_session() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;

        assert!(!forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut NvstCursorCaptureOutput::default(),
            NvstReceiveEvent::Dropped(NvstDropReason::MediaConsumerBackpressured),
        ));

        assert_eq!(recovery_attempts, 0);
        assert_eq!(resources.keyframe_requests.load(Ordering::Relaxed), 1);
        assert_eq!(resources.stops.load(Ordering::Relaxed), 0);
        assert_eq!(lock_lifecycle(&lifecycle).state, State::Connected);
        assert!(
            receiver
                .try_iter()
                .all(|message| message["type"] != "error" && message["type"] != "status")
        );
    }

    #[test]
    fn exhausted_nvst_recovery_stops_every_leg_and_emits_terminal_status() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;
        let recovery = || {
            NvstReceiveEvent::RecoveryNeeded(opennow_streamer_transport::NvstRecovery::Timeout {
                idle_for: Duration::from_secs(2),
            })
        };

        assert!(!forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut NvstCursorCaptureOutput::default(),
            recovery(),
        ));
        assert!(forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut NvstCursorCaptureOutput::default(),
            recovery(),
        ));

        assert_eq!(resources.recoveries.load(Ordering::Relaxed), 1);
        assert_eq!(resources.keyframe_requests.load(Ordering::Relaxed), 1);
        assert_eq!(resources.stops.load(Ordering::Relaxed), 1);
        let lifecycle = lock_lifecycle(&lifecycle);
        assert_eq!(lifecycle.state, State::Idle);
        assert!(lifecycle.context.is_none());
        drop(lifecycle);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(events.iter().any(|message| {
            message["type"] == "error" && message["code"] == "nvst-recovery-exhausted"
        }));
        for message in &events {
            if matches!(message["type"].as_str(), Some("error" | "status")) {
                assert_eq!(message["termination"]["source"], "nvst-transport");
                assert_eq!(message["termination"]["code"], "nvst-recovery-exhausted");
                assert!(message["termination"]["resumable"].is_null());
            }
        }
        assert!(
            events
                .iter()
                .any(|message| { message["type"] == "status" && message["status"] == "stopped" })
        );
    }

    #[test]
    fn assembled_keyframe_does_not_reset_recovery_episode_budget() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 1;

        assert!(!forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut NvstCursorCaptureOutput::default(),
            NvstReceiveEvent::Frame(opennow_streamer_transport::EncodedVideoAccessUnit {
                codec: opennow_streamer_transport::NvstVideoCodec::H264,
                timestamp: 1,
                frame_index: 1,
                first_stream_packet_index: 1,
                keyframe: true,
                contiguous: true,
                bytes: vec![0, 0, 0, 1, 0x65],
            }),
        ));
        assert_eq!(recovery_attempts, 1);
    }

    #[test]
    fn accepted_color_profiles_are_not_silently_downgraded() {
        let mut value = synthetic_context("accepted-color-profile", json!([]));
        for (codec, color) in [
            ("H264", "8bit_444"),
            ("H264", "10bit_420"),
            ("H264", "10bit_444"),
            ("AV1", "8bit_444"),
            ("AV1", "10bit_444"),
            (" av1 ", " 10bit_444 "),
        ] {
            value["session"]["negotiatedStreamProfile"] = json!({
                "codec": codec, "colorQuality": color, "enableHdr": false
            });
            let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
            assert!(
                validate_context(&context, "invalid-color").is_err(),
                "{codec} {color}"
            );
        }
        for (codec, color) in [
            ("H264", "8bit_420"),
            ("H265", "8bit_444"),
            ("H265", "10bit_444"),
            ("AV1", "10bit_420"),
        ] {
            value["session"]["negotiatedStreamProfile"] = json!({
                "codec": codec, "colorQuality": color, "enableHdr": false
            });
            let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
            assert!(
                validate_context(&context, "valid-color").is_ok(),
                "{codec} {color}"
            );
        }
    }

    #[test]
    fn unknown_accepted_color_is_not_replaced_by_local_settings() {
        let mut value = synthetic_context("invalid-accepted-color", json!([]));
        value["settings"]["colorQuality"] = json!("10bit_444");
        value["session"]["negotiatedStreamProfile"] = json!({
            "codec":"H265", "colorQuality":null, "bitDepthSource":"finalized"
        });
        let context: SessionContext = serde_json::from_value(value).unwrap();
        assert!(validate_context(&context, "color").is_err());
    }

    #[test]
    fn accepted_color_maps_to_native_decode_depth_and_chroma() {
        for (color, expected) in [
            ("8bit_420", MediaColorQuality::EightBit420),
            ("10bit_420", MediaColorQuality::TenBit420),
            ("10bit_444", MediaColorQuality::TenBit444),
        ] {
            let mut value = synthetic_context("accepted-color", json!([]));
            value["settings"]["colorQuality"] = json!("8bit_420");
            value["session"]["negotiatedStreamProfile"] =
                json!({"codec":"H265","colorQuality":color});
            let context: SessionContext = serde_json::from_value(value).unwrap();
            assert!(validate_context(&context, "color").is_ok());
            assert_eq!(media_stream_config(&context).color_quality, expected);
        }
    }

    #[test]
    fn media_hdr_uses_only_accepted_profile_including_sdr_fallback() {
        let mut value = synthetic_context("hdr-media-config", json!([]));
        value["settings"] = json!({"enableHdr":true,"codec":"H264","colorQuality":"8bit_420"});
        value["session"]["negotiatedStreamProfile"] = json!({
            "codec":"H265","colorQuality":"10bit_420","enableHdr":true
        });
        let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
        assert!(media_stream_config(&context).hdr);
        assert_eq!(
            media_stream_config(&context).color_quality,
            MediaColorQuality::TenBit420
        );
        assert!(validate_context(&context, "hdr").is_ok());
        value["settings"]["enableHdr"] = json!(false);
        let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
        assert!(media_stream_config(&context).hdr);
        value["settings"]["enableHdr"] = json!(true);
        for accepted in [json!(false), Value::Null] {
            value["session"]["negotiatedStreamProfile"]["enableHdr"] = accepted;
            let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
            assert!(!media_stream_config(&context).hdr);
        }
        value["session"]["negotiatedStreamProfile"]["enableHdr"] = json!(true);
        for (codec, color) in [
            ("H264", "10bit_420"),
            ("H265", "8bit_420"),
            ("AV1", "10bit_444"),
        ] {
            value["session"]["negotiatedStreamProfile"]["codec"] = json!(codec);
            value["session"]["negotiatedStreamProfile"]["colorQuality"] = json!(color);
            let context: SessionContext = serde_json::from_value(value.clone()).unwrap();
            assert!(validate_context(&context, "invalid-hdr").is_err());
        }
    }

    #[test]
    fn accepted_hevc_hdr_444_preserves_bit_depth_chroma_and_hdr() {
        for codec in ["H265", "HEVC"] {
            let mut value = synthetic_context("hdr-444-media-config", json!([]));
            value["settings"] = json!({"enableHdr": false, "colorQuality": "8bit_420"});
            value["session"]["negotiatedStreamProfile"] = json!({
                "codec": codec, "colorQuality": "10bit_444", "enableHdr": true
            });
            let context: SessionContext = serde_json::from_value(value).unwrap();
            assert!(validate_context(&context, "hdr-444").is_ok());
            let stream = media_stream_config(&context);
            assert_eq!(stream.codec, MediaVideoCodec::H265);
            assert_eq!(stream.color_quality, MediaColorQuality::TenBit444);
            assert!(stream.hdr);
        }
    }

    #[test]
    fn queue_drop_feedback_flushes_periodically_without_another_drop() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = Arc::new(connected_lifecycle());
        let (transport_sender, nvst_events) = std::sync::mpsc::channel();
        let resources = TestNvstResources::default();
        resources.rumble_coalesced.store(5, Ordering::Relaxed);
        let worker_lifecycle = lifecycle.clone();
        let worker = thread::spawn(move || {
            forward_nvst_session_events(
                &sender,
                &worker_lifecycle,
                7,
                NvstSessionEventResources {
                    start_id: "test-session".to_owned(),
                    nvst_events,
                    captured_input: Arc::new(CapturedInputQueue::default()),
                    transport: resources,
                },
            )
        });
        let report = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("idle periodic flush");
        assert_eq!(report["event"], "queue-dropped");
        assert_eq!(report["media"], "controller-rumble-coalesced");
        assert_eq!(report["count"], 5);
        lock_lifecycle(&lifecycle).generation += 1;
        worker.join().unwrap();
        assert!(receiver.try_recv().is_err());
        drop(transport_sender);
    }

    #[test]
    fn queue_drop_shutdown_flushes_pending_reports_once() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        let output = engine.events.clone();
        let lifecycle = engine.lifecycle.clone();
        let (_transport_sender, nvst_events) = std::sync::mpsc::channel();
        let resources = TestNvstResources::default();
        let coalesced = Arc::clone(&resources.rumble_coalesced);
        coalesced.store(3, Ordering::Relaxed);
        engine.event_worker = Some(thread::spawn(move || {
            forward_nvst_session_events(
                &output,
                &lifecycle,
                0,
                NvstSessionEventResources {
                    start_id: "test-session".to_owned(),
                    nvst_events,
                    captured_input: Arc::new(CapturedInputQueue::default()),
                    transport: resources,
                },
            )
        }));
        let deadline = Instant::now() + Duration::from_secs(3);
        while coalesced.load(Ordering::Relaxed) != 0 {
            assert!(Instant::now() < deadline, "event worker never polled");
            thread::sleep(Duration::from_millis(5));
        }
        engine.stop("test shutdown");
        let reports: Vec<_> = receiver
            .try_iter()
            .filter(|value| value["event"] == "queue-dropped")
            .collect();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["count"], 3);
        engine.stop("repeated shutdown");
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn captured_input_queue_is_drained_into_the_nvst_input_channel() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = Arc::new(connected_lifecycle());
        let (transport_sender, nvst_events) = std::sync::mpsc::channel();
        let queue = Arc::new(CapturedInputQueue::default());
        let resources = TestNvstResources::default();
        let sent = Arc::clone(&resources.captured_inputs);
        let worker_lifecycle = lifecycle.clone();
        let worker_queue = Arc::clone(&queue);
        let worker = thread::spawn(move || {
            forward_nvst_session_events(
                &sender,
                &worker_lifecycle,
                7,
                NvstSessionEventResources {
                    start_id: "input-session".to_owned(),
                    nvst_events,
                    captured_input: worker_queue,
                    transport: resources,
                },
            )
        });
        transport_sender
            .send(NvstReceiveEvent::InputReady(3))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        queue.push(CapturedInput::Key {
            virtual_key: 0x41,
            modifiers: 0,
            pressed: true,
        });
        while sent.lock().unwrap().is_empty() {
            if Instant::now() >= deadline {
                break;
            }
            queue.push(CapturedInput::MouseButton {
                button: 1,
                pressed: true,
            });
            thread::sleep(Duration::from_millis(5));
        }
        lock_lifecycle(&lifecycle).generation += 1;
        worker.join().unwrap();
        let packets = sent.lock().unwrap();
        assert!(!packets.is_empty(), "input never reached the transport");
        assert!(packets.iter().all(|packet| packet.len() >= 18));
    }

    #[test]
    fn cursor_messages_are_delivered_to_the_embedder_as_events() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sender = EventSender::unbounded(sender);
        let lifecycle = connected_lifecycle();
        let resources = TestNvstResources::default();
        let mut recovery_attempts = 0;
        let mut cursor = NvstCursorCaptureOutput {
            start_id: "cursor-session".to_owned(),
            pending: None,
        };
        assert!(!forward_nvst_event(
            &sender,
            &lifecycle,
            7,
            &resources,
            &mut recovery_attempts,
            &mut cursor,
            NvstReceiveEvent::Cursor(vec![1, 2, 3, 4]),
        ));
        let message = receiver.try_recv().expect("cursor event");
        assert_eq!(message["type"], "cursor-update");
        assert_eq!(message["startId"], "cursor-session");
        assert_eq!(message["payloadBase64"], BASE64.encode([1, 2, 3, 4]));
    }

    #[test]
    fn hello_reports_honest_transport_only_capabilities() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        let command = command(json!({
            "id": "hello",
            "type": "hello",
            "protocolVersion": PROTOCOL_VERSION,
        }));
        let (responses, _) = engine.handle(command);
        assert_eq!(responses[0]["type"], "ready");
        let capabilities = &responses[0]["capabilities"];
        assert!(capabilities.get("supportsOfferAnswer").is_none());
        assert!(capabilities.get("supportsRemoteIce").is_none());
        assert!(capabilities.get("supportsVideoPresent").is_none());
        assert!(capabilities.get("supportsVideoDecode").is_none());
        assert_eq!(capabilities["supportsInput"], true);
    }

    #[test]
    fn removed_local_playback_commands_are_unknown() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        for kind in [
            "audioDevices",
            "setAudioMuted",
            "input-paused",
            "surface",
            "recording-start",
            "clip-save",
            "microphone-set",
            "stats-toggle",
        ] {
            let (responses, keep_running) =
                engine.handle(command(json!({"id": "removed", "type": kind})));
            assert!(keep_running);
            assert_eq!(responses[0]["code"], "unknown-command", "{kind}");
        }
        assert_eq!(lifecycle_state(&engine), State::Idle);
    }

    #[test]
    fn microphone_sending_requires_an_active_session() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let engine = test_engine(sender);
        assert!(engine.set_microphone_enabled(true).is_err());
        assert!(engine.send_microphone_opus(vec![1, 2, 3], 960).is_err());
    }

    #[test]
    fn derives_bounded_windows_media_configuration_from_stream_settings() {
        let mut value = synthetic_context("media-config", json!([]));
        value["settings"] = json!({
            "codec": "H264",
            "resolution": "2560x1440",
            "fps": 120,
            "maxBitrateMbps": 75,
            "autoFullScreen": true
        });
        let context: SessionContext = serde_json::from_value(value).expect("context");

        assert_eq!(
            media_stream_config(&context),
            MediaStreamConfig {
                codec: MediaVideoCodec::H264,
                color_quality: MediaColorQuality::EightBit420,
                hdr: false,
                width: 2560,
                height: 1440,
                fps: 120,
                bitrate_bps: 75_000_000,
            }
        );
        let mut low_rate = context.clone();
        low_rate.settings["maxBitrateMbps"] = json!(0.22);
        assert_eq!(media_stream_config(&low_rate).bitrate_bps, 220_000);

        let fallback: SessionContext =
            serde_json::from_value(synthetic_context("fallback-config", json!([])))
                .expect("context");
        assert_eq!(media_stream_config(&fallback), MediaStreamConfig::default());

        let mut high_fps = synthetic_context("high-fps-config", json!([]));
        high_fps["settings"] = json!({
            "codec": "H264",
            "resolution": "1920x1080",
            "fps": 360,
            "maxBitrateMbps": 100
        });
        high_fps["session"]["negotiatedStreamProfile"] = json!({
            "codec": "AV1",
            "fps": 400,
            "colorQuality": "10bit_444"
        });
        let high_fps: SessionContext = serde_json::from_value(high_fps).expect("context");
        assert_eq!(media_stream_config(&high_fps).codec, MediaVideoCodec::Av1);
        assert_eq!(media_stream_config(&high_fps).fps, 360);
        assert_eq!(
            media_stream_config(&high_fps).color_quality,
            MediaColorQuality::TenBit420
        );

        let mut top_tier = synthetic_context("top-tier-config", json!([]));
        top_tier["settings"] = json!({
            "codec": "H265",
            "resolution": "1920x1080",
            "fps": 360,
            "maxBitrateMbps": 100
        });
        top_tier["session"]["negotiatedStreamProfile"] = json!({"fps": 360});
        let top_tier: SessionContext = serde_json::from_value(top_tier).expect("context");
        assert_eq!(media_stream_config(&top_tier).fps, 360);

        let mut legacy_overlay = synthetic_context("legacy-overlay-config", json!([]));
        legacy_overlay["settings"] = json!({
            "showNativeStreamerStats": true,
            "showStatsOnLaunch": true,
            "statsOverlayPosition": "bottom-left",
            "autoFullScreen": true
        });
        let legacy_overlay: SessionContext =
            serde_json::from_value(legacy_overlay).expect("context");
        assert_eq!(
            media_stream_config(&legacy_overlay),
            MediaStreamConfig::default()
        );
    }

    #[test]
    fn valid_nvst_handoff_starts_udp_video_and_rejects_removed_offer_command() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (media_sender, _media_receiver) = std::sync::mpsc::sync_channel(4);
        let mut engine = Engine::with_media_consumer(sender, media_sender);
        let mut context = synthetic_context("nvst-session", json!([]));
        context["settings"]["codec"] = json!("AV1");
        context["nvstVideo"] = json!({
            "clientUdpPort": unused_udp_port(),
            "videoPeerIp": "127.0.0.1",
            "videoPeerPort": 5004,
            "srtpAesKeyHex": "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F",
            "srtpSaltHex": "00000000000000009ECA935E",
            "codec": "H264"
        });
        let (responses, _) = engine.handle(command(json!({
            "id": "start",
            "type": "start",
            "context": context.clone(),
        })));

        assert_eq!(responses[0]["type"], "ok");
        assert_eq!(responses[0]["transport"], "nvst");
        assert!(
            responses[0]["capabilities"]
                .get("supportsOfferAnswer")
                .is_none()
        );
        assert!(
            responses[0]["capabilities"]
                .get("supportsRemoteIce")
                .is_none()
        );
        assert_eq!(responses[0]["capabilities"]["supportsInput"], false);
        assert_eq!(responses[0]["capabilities"]["supportsAudio"], false);
        assert_eq!(lifecycle_state(&engine), State::Connected);
        assert!(engine.nvst_transport.is_some());
        assert!(receiver.try_iter().any(|message| {
            message["type"] == "status"
                && message["message"]
                    .as_str()
                    .is_some_and(|text| text.contains("NVST"))
        }));

        let (responses, _) = engine.handle(command(json!({
            "id": "offer",
            "type": "offer",
            "context": context,
        })));
        assert_eq!(responses[0]["code"], "unknown-command");

        let (responses, _) = engine.handle(command(json!({
            "id": "stop",
            "type": "stop",
            "reason": "test complete",
        })));
        assert_eq!(responses[0]["type"], "ok");
        assert_eq!(lifecycle_state(&engine), State::Idle);
    }

    #[test]
    fn accepted_start_binds_the_hid_endpoint_and_termination_closes_it() {
        // Hold a live peer for the whole test. On Windows, sending to a closed
        // UDP port makes the next receive fail with WSAECONNRESET. That exits
        // the bundle thread, which unbinds HID before the assertion below can
        // observe the binding start() just installed.
        let peer = UdpSocket::bind("127.0.0.1:0").expect("HID test peer socket");
        let peer_port = peer.local_addr().expect("HID test peer address").port();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let (media_sender, _media_receiver) = std::sync::mpsc::sync_channel(4);
        let mut engine = Engine::with_media_consumer(sender, media_sender);
        let mut context = synthetic_context("hid-endpoint-lifecycle", json!([]));
        context["settings"]["codec"] = json!("AV1");
        context["nvstVideo"] = json!({
            "clientUdpPort": unused_udp_port(),
            "videoPeerIp": "127.0.0.1",
            "videoPeerPort": peer_port,
            "srtpAesKeyHex": "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F",
            "srtpSaltHex": "00000000000000009ECA935E",
            "codec": "H264"
        });
        let (responses, _) = engine.handle(command(json!({
            "id": "start",
            "type": "start",
            "context": context,
        })));
        assert_eq!(responses[0]["type"], "ok");
        let generation = lock_lifecycle(&engine.lifecycle).generation;
        assert_eq!(engine.hid_runtime.session_generation(), Some(generation));
        assert!(
            engine
                .hid_runtime
                .bind_session(generation.wrapping_add(1))
                .is_some()
        );
        engine
            .hid_runtime
            .unbind_session(generation.wrapping_add(1));
        assert_eq!(engine.hid_runtime.session_generation(), None);

        if let Some(transport) = engine.nvst_transport.take() {
            transport.stop();
        }
        assert_eq!(
            engine.hid_runtime.session_generation(),
            None,
            "terminating the owned transport must close the HID endpoint"
        );
        assert!(
            engine.hid_runtime.bind_session(9_999).is_none(),
            "a closed endpoint must refuse late binding"
        );

        let (responses, _) = engine.handle(command(json!({
            "id": "stop",
            "type": "stop",
            "reason": "test complete",
        })));
        assert_eq!(responses[0]["type"], "ok");
        assert_eq!(lifecycle_state(&engine), State::Idle);
        assert!(engine.hid_runtime.bind_session(10_000).is_none());
    }

    #[test]
    fn explicit_invalid_nvst_handoff_fails_closed() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        let mut context = synthetic_context("invalid-nvst-session", json!([]));
        context["nvstVideo"] = json!({
            "clientUdpPort": 0,
            "codec": "H264"
        });

        let (responses, _) = engine.handle(command(json!({
            "id": "start-invalid-nvst",
            "type": "start",
            "context": context,
        })));

        assert_eq!(responses[0]["code"], "invalid-nvst-handoff");
        assert_eq!(lifecycle_state(&engine), State::Idle);
        assert!(engine.nvst_transport.is_none());
        assert!(engine.hid_runtime.bind_session(1).is_none());
        assert_eq!(engine.hid_runtime.session_generation(), None);
    }

    #[test]
    fn explicit_nvst_mode_without_endpoint_fails_closed() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        let mut context = synthetic_context("missing-nvst-session", json!([]));
        context["settings"]["transportMode"] = json!("nvst");

        let (responses, _) = engine.handle(command(json!({
            "id": "start-missing-nvst",
            "type": "start",
            "context": context,
        })));

        assert_eq!(responses[0]["code"], "missing-rtsps-endpoint");
        assert_eq!(lifecycle_state(&engine), State::Idle);
        assert!(engine.nvst_transport.is_none());
    }

    #[test]
    fn unused_nvst_reservation_can_be_released_idempotently() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);

        let (responses, _) = engine.handle(command(json!({
            "id": "bind",
            "type": "nvst-bind",
        })));
        assert_eq!(responses[0]["type"], "nvst-bound");
        assert!(engine.reserved_nvst_bundle.is_some());

        for id in ["unbind", "unbind-again"] {
            let (responses, _) = engine.handle(command(json!({
                "id": id,
                "type": "nvst-unbind",
            })));
            assert_eq!(responses[0]["type"], "ok");
            assert!(engine.reserved_nvst_bundle.is_none());
        }
    }

    #[test]
    fn start_rejects_invalid_contexts_and_missing_nvst_handoffs() {
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut engine = test_engine(sender);
        let invalid = command(json!({
            "id": "invalid",
            "type": "start",
            "context": {
                "session": { "sessionId": "", "serverIp": "host", "iceServers": [] },
                "settings": {},
                "shortcuts": {}
            }
        }));
        let (responses, _) = engine.handle(invalid);
        assert_eq!(responses[0]["code"], "invalid-context");
        assert_eq!(lifecycle_state(&engine), State::Idle);

        let (responses, _) = engine.handle(command(json!({
            "id": "missing-nvst",
            "type": "start",
            "context": synthetic_context("synthetic-session", json!([])),
        })));
        assert_eq!(responses[0]["code"], "nvst-handoff-required");
        assert_eq!(lifecycle_state(&engine), State::Idle);
    }

    #[test]
    fn derives_initial_media_dimensions_from_session_settings() {
        assert_eq!(
            media_stream_config(
                &serde_json::from_value(json!({
                    "session": {
                        "sessionId": "test",
                        "serverIp": "127.0.0.1",
                        "negotiatedStreamProfile": { "resolution": "3840x2160" }
                    },
                    "settings": { "resolution": "1920x1080" },
                    "shortcuts": {}
                }))
                .expect("context")
            ),
            MediaStreamConfig {
                width: 3840,
                height: 2160,
                ..MediaStreamConfig::default()
            }
        );
        assert_eq!(
            media_stream_config(
                &serde_json::from_value(json!({
                    "session": { "sessionId": "test", "serverIp": "127.0.0.1" },
                    "settings": { "resolution": "invalid" },
                    "shortcuts": {}
                }))
                .expect("context")
            ),
            MediaStreamConfig::default()
        );
    }
}
