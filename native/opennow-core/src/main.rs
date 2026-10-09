#![recursion_limit = "512"]

mod cloudmatch;
mod credential_vault;
mod device_identity;
mod diagnostics;
mod frame_rate;
mod gfn;
mod language;
mod network;
mod network_test;
mod proxy;
mod queue_servers;
mod requests;
mod settings;
mod streamer;
mod version;

use fs2::FileExt;
use gfn::GfnService;
use serde_json::{Map, Value, json};
use settings::{SettingsStore, resolve_data_dir};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::Instant;
use streamer::StreamerService;

const PROTOCOL_VERSION: i64 = 6;
const MAXIMUM_LINE_BYTES: usize = 1024 * 1024;
static PROFILE_LOCK: OnceLock<std::fs::File> = OnceLock::new();

struct AppCore {
    session_update_gate: Mutex<()>,
    settings: Mutex<SettingsStore>,
    gfn: Arc<GfnService>,
    streamer: StreamerService,
    diagnostics: diagnostics::DiagnosticsService,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("opennow-core: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let data_dir = resolve_data_dir(argument_value("--data-dir").map(PathBuf::from));
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("Could not initialize the data directory: {error}"))?;
    let profile_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("core.lock"))
        .map_err(|error| format!("Could not open the data directory lock: {error}"))?;
    profile_lock.try_lock_exclusive().map_err(|error| {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            "The OpenNOW data directory is already in use".to_owned()
        } else {
            format!("Could not lock the data directory: {error}")
        }
    })?;
    PROFILE_LOCK.get_or_init(|| profile_lock);
    let (output_tx, output_rx) = mpsc::channel::<Value>();
    thread::Builder::new()
        .name("opennow-core-writer".to_owned())
        .spawn(move || {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            for value in output_rx {
                if let Err(error) = write_json(&mut output, &value) {
                    eprintln!("opennow-core: output failed: {error}");
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    let gfn = Arc::new(GfnService::new(data_dir.clone())?);
    let core = Arc::new(AppCore {
        session_update_gate: Mutex::new(()),
        settings: Mutex::new(
            SettingsStore::load(Some(data_dir.clone())).map_err(|error| error.to_string())?,
        ),
        gfn,
        streamer: StreamerService::new(),
        diagnostics: diagnostics::DiagnosticsService::new(&data_dir)
            .map_err(|error| format!("Could not initialize diagnostics: {error}"))?,
    });
    let requests = Arc::new(requests::Requests::default());
    let stdin = io::stdin();

    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.len() > MAXIMUM_LINE_BYTES {
            return Err("protocol line exceeds the size limit".to_owned());
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => return Err("malformed JSON protocol message".to_owned()),
        };
        if message["type"] == "cancel" {
            if let Some(id) = message["id"].as_str() {
                requests.cancel(id);
            }
            continue;
        }
        if message["type"] == "ack" {
            if let Some(id) = message["id"].as_str() {
                requests.acknowledge(id);
            }
            continue;
        }
        if message["type"] != "request" {
            return Err("unknown protocol message".to_owned());
        }
        let id = message["id"].as_str().unwrap_or_default().to_owned();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if id.is_empty() || method.is_empty() {
            output_tx.send(json!({"type":"response", "id":id, "ok":false, "error":{"code":"invalid_request", "message":"Request requires string id and method"}}))
                .map_err(|error| error.to_string())?;
            continue;
        }
        let Some(permit) = requests.admit(&id, &method) else {
            output_tx.send(json!({"type":"response", "id":id, "ok":false, "error":{"code":"busy", "message":"Core request limit reached"}}))
                .map_err(|error| error.to_string())?;
            continue;
        };

        let worker_core = Arc::clone(&core);
        let worker_output = output_tx.clone();
        thread::Builder::new().name(format!("opennow-rpc-{id}")).spawn(move || {
            let started = Instant::now();
            let result = requests::scope(permit.token.clone(), || {
                requests::check().map_err(|error| (error.code.to_owned(), error.message))?;
                dispatch(&method, &params, &worker_core)
            });
            let outcome = match &result {
                Ok(_) => "ok",
                Err((code, _)) => code.as_str(),
            };
            worker_core.diagnostics.record(
                "rpc",
                &method,
                format!("outcome={outcome} durationMs={}", started.elapsed().as_millis()),
            );
            let was_cancelled = permit.token.cancelled();
            if method == "session.create"
                && let Err((code, message)) = &result
                && code == "session_cleanup_pending"
            {
                let _ = worker_output.send(json!({"type":"event","name":"session.cleanup.pending",
                    "payload":{"code":code,"message":message}}));
            }
            if method == "session.create"
                && let Ok((value, _)) = &result
                && let Some(session_id) = value["session"]["sessionId"].as_str()
            {
                let delivered = !was_cancelled && worker_output.send(json!({"type":"response", "id":id, "ok":true, "result":value})).is_ok();
                let accepted = delivered && permit.token.await_acceptance(std::time::Duration::from_secs(10));
                let cleanup = worker_core.gfn.finish_session_create(session_id, accepted);
                worker_core.diagnostics.record("session", "allocation-handoff", format!(
                    "accepted={accepted} cleanup={}", cleanup.as_ref().map_or_else(|error| error.code, |()| "ok")
                ));
                if let Err(error) = cleanup {
                    let _ = worker_output.send(json!({"type":"event","name":"session.cleanup.pending","payload":{
                        "sessionId":session_id,"code":error.code,"message":"The cancelled cloud session could not be closed. End it before starting another game."
                    }}));
                }
                return;
            }
            if !was_cancelled {
                match result {
                    Ok((value, event)) => {
                        if let Some(("settings.changed", payload)) = &event {
                            let _ = worker_output.send(json!({"type":"event", "name":"settings.changed", "payload":payload}));
                        }
                        let _ = worker_output.send(json!({"type":"response", "id":id, "ok":true, "result":value}));
                        if let Some((name, payload)) = event && name != "settings.changed" {
                            let _ = worker_output.send(json!({"type":"event", "name":name, "payload":payload}));
                        }
                    }
                    Err((code, message)) => {
                        let _ = worker_output.send(json!({"type":"response", "id":id, "ok":false, "error":{"code":code, "message":message}}));
                    }
                }
            }
            drop(permit);
        }).map_err(|error| error.to_string())?;
    }
    Ok(())
}

type DispatchResult = Result<(Value, Option<(&'static str, Value)>), (String, String)>;

fn dispatch(method: &str, params: &Value, core: &AppCore) -> DispatchResult {
    let session_transition = matches!(
        method,
        "session.create" | "session.claim" | "session.poll" | "streamer.prepare"
    );
    let _session_update_guard = if session_transition {
        Some(core.session_update_gate.try_lock().map_err(|_| {
            (
                "session_update_busy".to_owned(),
                "A session transition is in progress".to_owned(),
            )
        })?)
    } else {
        None
    };
    match method {
        "core.hello" => {
            if params["protocolVersion"].as_i64() != Some(PROTOCOL_VERSION) {
                return Err((
                    "incompatible_protocol".to_owned(),
                    "Client and core protocol versions differ".to_owned(),
                ));
            }
            Ok((
                json!({"protocolVersion":PROTOCOL_VERSION, "coreVersion":version::APPLICATION_VERSION, "capabilities":["settings", "gfn.deviceAuth", "gfn.providers", "gfn.regions", "gfn.cloudmatch", "sessionProxy", "nativeStreamer.v7", "nativeStreamer.ownedNvstNegotiation", "redactedDiagnostics"]}),
                None,
            ))
        }
        "app.status" => Ok((
            json!({"status":"ready", "version":version::APPLICATION_VERSION}),
            None,
        )),
        "settings.get" => Ok((
            json!({"settings":core.settings.lock().expect("settings poisoned").all()}),
            None,
        )),
        "settings.set" => {
            let key = params["key"].as_str().ok_or((
                "invalid_params".to_owned(),
                "settings.set requires a key".to_owned(),
            ))?;
            let value = params.get("value").cloned().ok_or((
                "invalid_params".to_owned(),
                "settings.set requires a value".to_owned(),
            ))?;
            if key == "region" {
                let provider = params["providerIdpId"].as_str().unwrap_or("");
                let event = core.gfn.with_region_provider(provider, || {
                    let mut settings = core.settings.lock().expect("settings poisoned");
                    let applied = settings.set_provider_region(provider, value).map_err(|message| gfn::ServiceError { code: "invalid_setting", message })?;
                    Ok(json!({"key":key,"value":applied,"changes":{
                        "regionProviderIdpId":provider,"providerRegions":settings.all()["providerRegions"]
                    }}))
                }).map_err(gfn_error)?;
                return Ok((event.clone(), Some(("settings.changed", event))));
            }
            let mut settings = core.settings.lock().expect("settings poisoned");
            let codec_before = settings.all()["codec"].clone();
            let fallback_before = settings.all()["fallbackCodec"].clone();
            let applied = settings
                .set(key, value)
                .map_err(|message| ("invalid_setting".to_owned(), message))?;
            let mut event = json!({"key":key, "value":applied});
            if key == "colorQuality" {
                let current = settings.all();
                let mut changes = Map::new();
                if current["codec"] != codec_before {
                    changes.insert("codec".to_owned(), current["codec"].clone());
                }
                if current["fallbackCodec"] != fallback_before {
                    changes.insert("fallbackCodec".to_owned(), current["fallbackCodec"].clone());
                }
                if !changes.is_empty() {
                    event["changes"] = Value::Object(changes);
                }
            }
            Ok((event.clone(), Some(("settings.changed", event))))
        }
        "settings.reset" => {
            let values = core
                .settings
                .lock()
                .expect("settings poisoned")
                .reset()
                .map_err(|message| ("settings_write_failed".to_owned(), message))?;
            Ok((
                json!({"settings":values}),
                Some(("settings.reset", json!({}))),
            ))
        }
        "auth.providers.list" => core
            .gfn
            .providers()
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.device.start" => core
            .gfn
            .start_device_login(params)
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.device.poll" => core
            .gfn
            .poll_device_login(params)
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.device.complete" => core
            .gfn
            .complete_device_login(params)
            .map(|value| (value.clone(), Some(("auth.session.changed", value))))
            .map_err(gfn_error),
        "auth.device.cancel" => core
            .gfn
            .cancel_device_login(params)
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.session.get" => core
            .gfn
            .session()
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.logout" => {
            let value = core.gfn.logout().map_err(gfn_error)?;
            Ok((value.clone(), Some(("auth.session.changed", value))))
        }
        "auth.accounts.logoutAll" => {
            let value = core.gfn.logout_all().map_err(gfn_error)?;
            Ok((value.clone(), Some(("auth.session.changed", value))))
        }
        "auth.accounts.list" => core
            .gfn
            .saved_accounts()
            .map(|value| (value, None))
            .map_err(gfn_error),
        "auth.accounts.switch" => core
            .gfn
            .switch_account(params)
            .map(|value| (value.clone(), Some(("auth.session.changed", value))))
            .map_err(gfn_error),
        "auth.accounts.remove" => core
            .gfn
            .remove_account(params)
            .map(|value| (value.clone(), Some(("auth.session.changed", value))))
            .map_err(gfn_error),
        "network.regions.list" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .regions(&settings)
                .map(|value| (value, None))
                .map_err(gfn_error)
        }
        "network.regions.ping" => network::ping_regions(params)
            .map(|value| (value, None))
            .map_err(|message| ("region_ping_failed".to_owned(), message)),
        "session.create" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            let settings = if !params["runtimeCapabilities"].is_null() {
                StreamerService::embedded_session_settings(
                    &settings,
                    &params["runtimeCapabilities"],
                )
                .map_err(streamer_error)?
            } else {
                core.streamer
                    .validate_codec(&settings)
                    .map_err(streamer_error)?;
                settings
            };
            core.gfn
                .create_session(params, &settings)
                .map(|value| (value.clone(), Some(("session.changed", value))))
                .map_err(gfn_error)
        }
        "session.poll" => core
            .gfn
            .poll_session(params)
            .map(|value| {
                (
                    value.clone(),
                    (params["recoveryMode"] != true).then_some(("session.changed", value)),
                )
            })
            .map_err(gfn_error),
        "session.stop" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .stop_session(params, &settings)
                .map(|value| (value.clone(), Some(("session.changed", value))))
                .map_err(gfn_error)
        }
        "session.active.get" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .reconcile_active_session(params, &settings)
                .map(|value| (value, None))
                .map_err(gfn_error)
        }
        "session.remote.list" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .remote_sessions(params, &settings)
                .map(|value| (value, None))
                .map_err(gfn_error)
        }
        "session.claim" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .claim_session(params, &settings)
                .map(|value| (value.clone(), Some(("session.changed", value))))
                .map_err(gfn_error)
        }
        "session.ad.report" => core
            .gfn
            .report_session_ad(params)
            .map(|value| (value.clone(), Some(("session.changed", value))))
            .map_err(gfn_error),
        "streamer.prepare" => {
            let settings = core.settings.lock().expect("settings poisoned").all();
            core.gfn
                .prepare_owned_stream(params, |owned| {
                    core.streamer
                        .prepare_embedded(owned, &settings)
                        .map_err(|error| gfn::ServiceError {
                            code: error.code,
                            message: error.message,
                        })
                })
                .inspect_err(|error| {
                    core.diagnostics.record(
                        "streamer",
                        "prepare_profile",
                        diagnostics::stream_profile_evidence(&params["session"]).to_string(),
                    );
                    core.diagnostics.record(
                        "streamer",
                        "prepare_rejected",
                        diagnostics::runtime_failure_reason(&error.message),
                    );
                })
                .map(|value| (value, None))
                .map_err(gfn_error)
        }
        "diagnostics.snapshot" => Ok((core.diagnostics.snapshot(), None)),
        _ => Err((
            "method_not_found".to_owned(),
            format!("Unknown core method: {method}"),
        )),
    }
}

fn gfn_error(error: gfn::ServiceError) -> (String, String) {
    (error.code.to_owned(), error.message)
}

fn streamer_error(error: streamer::StreamerError) -> (String, String) {
    (error.code.to_owned(), error.message)
}

fn write_json(output: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value).map_err(|error| error.to_string())?;
    output
        .write_all(b"\n")
        .and_then(|_| output.flush())
        .map_err(|error| error.to_string())
}

fn argument_value(name: &str) -> Option<String> {
    let arguments: Vec<String> = env::args().collect();
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
