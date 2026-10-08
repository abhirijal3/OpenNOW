use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ENTRY_LIMIT: usize = 1_200;
const LOG_LIMIT_BYTES: u64 = 5 * 1024 * 1024;

pub fn stream_profile_evidence(session: &Value) -> Value {
    let profile = &session["negotiatedStreamProfile"];
    let mut evidence = json!({
        "codec": profile["codec"].as_str().filter(|value| matches!(*value, "H264" | "H265" | "HEVC" | "AV1")),
        "codecSource": profile["codecSource"].as_str().filter(|value| matches!(*value, "request" | "server" | "unreported")),
        "colorQuality": profile["colorQuality"].as_str().filter(|value| matches!(*value, "8bit_420" | "8bit_444" | "10bit_420" | "10bit_444")),
        "enableHdr": profile["enableHdr"].as_bool()
    });
    for section in ["requestedStreamingFeatures", "finalizedStreamingFeatures"] {
        let mut fields = serde_json::Map::new();
        for field in ["codec", "bitDepth", "chromaFormat"] {
            if let Some(value) = session[section].get(field) {
                let number = value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()));
                fields.insert(field.to_owned(), json!(number));
            }
        }
        evidence[section] = Value::Object(fields);
    }
    evidence
}

pub fn native_runtime_evidence(capabilities: &Value) -> Value {
    let mut evidence = serde_json::Map::new();
    for field in [
        "supportsVideoDecode",
        "supportsVideoPresent",
        "nativeHdrSupported",
    ] {
        if let Some(value) = capabilities[field].as_bool() {
            evidence.insert(field.to_owned(), json!(value));
        }
    }
    if let Some(version) = capabilities["protocolVersion"]
        .as_u64()
        .filter(|v| *v <= u32::MAX as u64)
    {
        evidence.insert("protocolVersion".to_owned(), json!(version));
    }
    let mut backends = Vec::new();
    for backend in capabilities["videoBackends"]
        .as_array()
        .into_iter()
        .flatten()
        .take(16)
    {
        let Some(name) = backend["backend"].as_str().filter(|name| {
            matches!(
                *name,
                "vulkan"
                    | "cuda"
                    | "vaapi"
                    | "v4l2"
                    | "d3d11"
                    | "d3d12"
                    | "videotoolbox"
                    | "software"
                    | "ffmpeg"
            )
        }) else {
            continue;
        };
        let mut entry = serde_json::Map::from_iter([("backend".to_owned(), json!(name))]);
        if let Some(platform) = backend["platform"]
            .as_str()
            .filter(|name| matches!(*name, "linux" | "windows" | "macos" | "cross-platform"))
        {
            entry.insert("platform".to_owned(), json!(platform));
        }
        if let Some(value) = backend["available"].as_bool() {
            entry.insert("available".to_owned(), json!(value));
        }
        if let Some(reason) = backend["reason"].as_str() {
            entry.insert("reason".to_owned(), json!(runtime_failure_reason(reason)));
        }
        let mut codecs = Vec::new();
        for codec in backend["codecs"].as_array().into_iter().flatten().take(8) {
            let Some(name) = codec["codec"]
                .as_str()
                .filter(|name| matches!(*name, "h264" | "h265" | "av1"))
            else {
                continue;
            };
            let mut item = serde_json::Map::from_iter([("codec".to_owned(), json!(name))]);
            for field in ["available", "hdrSupported"] {
                if let Some(value) = codec[field].as_bool() {
                    item.insert(field.to_owned(), json!(value));
                }
            }
            if let Some(reason) = codec["reason"].as_str() {
                item.insert("reason".to_owned(), json!(runtime_failure_reason(reason)));
            }
            codecs.push(Value::Object(item));
        }
        entry.insert("codecs".to_owned(), json!(codecs));
        backends.push(Value::Object(entry));
    }
    evidence.insert("videoBackends".to_owned(), json!(backends));
    if let Some(adapters) = graphics_adapter_evidence(capabilities) {
        evidence.insert("graphicsAdapters".to_owned(), Value::Array(adapters));
    }
    Value::Object(evidence)
}

fn graphics_adapter_evidence(capabilities: &Value) -> Option<Vec<Value>> {
    let source = capabilities.get("graphicsAdapters")?.as_array()?;
    if source.is_empty() {
        return None;
    }
    let mut adapters = Vec::new();
    for adapter in source.iter().take(8) {
        let mut entry = serde_json::Map::new();
        if let Some(name) = adapter["name"].as_str() {
            entry.insert("name".to_owned(), json!(runtime_failure_reason(name)));
        }
        if let Some(active) = adapter["active"].as_bool() {
            entry.insert("active".to_owned(), json!(active));
        }
        if let Some(main10) = adapter["h265Main10"].as_bool() {
            entry.insert("h265Main10".to_owned(), json!(main10));
        }
        let codecs = adapter["codecs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|codec| {
                codec
                    .as_str()
                    .filter(|name| matches!(*name, "h264" | "h265" | "av1"))
                    .map(|name| json!(name))
            })
            .take(4)
            .collect::<Vec<_>>();
        entry.insert("codecs".to_owned(), json!(codecs));
        if let Some(reason) = adapter["reason"].as_str() {
            entry.insert("reason".to_owned(), json!(runtime_failure_reason(reason)));
        }
        adapters.push(Value::Object(entry));
    }
    Some(adapters)
}

pub fn runtime_failure_reason(value: &str) -> String {
    static SENSITIVE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = SENSITIVE.get_or_init(|| {
        regex::Regex::new(r#"(?i)(?:https?://|wss://|/home/|/users/|[a-z]:\\+users\\+)\S+|\bbearer\s+[^\s,;]+|\b[a-z_]*(?:token|authorization|password|secret)[a-z_]*\s*[\"']?\s*[:=]?\s*[\"']?\s*(?:bearer\s+)?[^\s,;]+"#).unwrap()
    });
    let bounded: String = value
        .chars()
        .take(4096)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    redact(&pattern.replace_all(&bounded, "[redacted]"), 480)
}

#[derive(Clone)]
struct Entry {
    at_ms: u128,
    area: String,
    event: String,
    detail: String,
}

pub struct DiagnosticsService {
    current_path: PathBuf,
    previous_path: PathBuf,
    entries: Mutex<VecDeque<Entry>>,
}

impl DiagnosticsService {
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let directory = data_dir.join("diagnostics");
        fs::create_dir_all(&directory)?;
        let current_path = directory.join("current.log");
        let previous_path = directory.join("previous.log");
        let service = Self {
            current_path,
            previous_path,
            entries: Mutex::new(VecDeque::with_capacity(ENTRY_LIMIT)),
        };
        service.rotate_log(0)?;
        Ok(service)
    }

    fn rotate_log(&self, incoming_bytes: u64) -> io::Result<()> {
        let size = match self.current_path.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if size.saturating_add(incoming_bytes) <= LOG_LIMIT_BYTES {
            return Ok(());
        }
        match fs::remove_file(&self.previous_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if size <= LOG_LIMIT_BYTES {
            return fs::rename(&self.current_path, &self.previous_path);
        }
        let mut current = fs::File::open(&self.current_path)?;
        current.seek(SeekFrom::Start(size.saturating_sub(LOG_LIMIT_BYTES)))?;
        let mut tail = BufReader::new(current.take(LOG_LIMIT_BYTES));
        tail.skip_until(b'\n')?;
        let mut previous = fs::File::create(&self.previous_path)?;
        io::copy(&mut tail, &mut previous)?;
        OpenOptions::new()
            .write(true)
            .open(&self.current_path)?
            .set_len(0)
    }

    pub fn record(&self, area: &str, event: &str, detail: impl AsRef<str>) {
        let entry = Entry {
            at_ms: now_ms(),
            area: clean(area, 48),
            event: clean(event, 72),
            detail: redact(detail.as_ref(), 480),
        };
        let mut entries = self.entries.lock().expect("diagnostics poisoned");
        if entries.len() == ENTRY_LIMIT {
            entries.pop_front();
        }
        entries.push_back(entry.clone());
        let line = format_entry(&entry);
        if self.rotate_log(line.len() as u64).is_err() {
            return;
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.current_path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub fn snapshot(&self) -> Value {
        let entries = self.entries.lock().expect("diagnostics poisoned");
        let values = entries
            .iter()
            .rev()
            .take(200)
            .map(|entry| {
                json!({
                    "atMs": entry.at_ms.to_string(),
                    "area": entry.area,
                    "event": entry.event,
                    "detail": entry.detail
                })
            })
            .collect::<Vec<_>>();
        drop(entries);
        json!({
            "entries": values,
            "persistent": true,
            "redacted": true,
            "currentBytes": self.current_path.metadata().map(|value| value.len()).unwrap_or(0),
            "previousRunAvailable": self.previous_path.is_file()
        })
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|value| !value.is_control())
        .take(limit)
        .collect()
}

fn redact(value: &str, limit: usize) -> String {
    let mut result = String::with_capacity(value.len().min(limit));
    for token in value.split_whitespace() {
        let normalized = token.trim_start_matches(['"', '\'', '{', '[', '(', ',', ':']);
        let lower = normalized.to_ascii_lowercase();
        let sensitive = lower.contains("token")
            || lower.contains("authorization")
            || lower.contains("password")
            || lower.contains("secret")
            || lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("wss://")
            || normalized.contains('@')
            || lower.starts_with("/home/")
            || lower.starts_with("/users/")
            || lower.starts_with("c:\\users\\")
            || lower.starts_with("c:\\\\users\\\\");
        let rendered = if sensitive { "[redacted]" } else { token };
        if !result.is_empty() {
            result.push(' ');
        }
        if result.len() + rendered.len() > limit {
            result.push('…');
            break;
        }
        result.push_str(rendered);
    }
    result
}

fn format_entry(entry: &Entry) -> String {
    format!(
        "{} [{}] {}: {}\n",
        entry.at_ms, entry.area, entry.event, entry.detail
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_rotate_during_a_session_and_replace_the_previous_log() {
        let directory = tempfile::tempdir().unwrap();
        let service = DiagnosticsService::new(directory.path()).unwrap();
        for marker in *b"abc" {
            let mut current = fs::File::create(&service.current_path).unwrap();
            current.set_len(LOG_LIMIT_BYTES).unwrap();
            current.seek(SeekFrom::End(-1)).unwrap();
            current.write_all(&[marker]).unwrap();
            drop(current);

            service.record("test", "rotation", "new entry");

            let previous = fs::read(&service.previous_path).unwrap();
            assert_eq!(previous.len() as u64, LOG_LIMIT_BYTES);
            assert_eq!(previous.last(), Some(&marker));
            let current = fs::read_to_string(&service.current_path).unwrap();
            assert!(current.contains("new entry"));
            assert!((current.len() as u64) < LOG_LIMIT_BYTES);
            assert_eq!(
                fs::read_dir(service.current_path.parent().unwrap())
                    .unwrap()
                    .count(),
                2
            );
        }
    }

    #[test]
    fn diagnostics_startup_retains_a_bounded_tail_of_an_oversized_log() {
        let directory = tempfile::tempdir().unwrap();
        let diagnostics = directory.path().join("diagnostics");
        fs::create_dir_all(&diagnostics).unwrap();
        let mut current = fs::File::create(diagnostics.join("current.log")).unwrap();
        current.set_len(LOG_LIMIT_BYTES * 2).unwrap();
        current.seek(SeekFrom::Start(LOG_LIMIT_BYTES - 1)).unwrap();
        current.write_all("é\n".as_bytes()).unwrap();
        current.seek(SeekFrom::End(-5)).unwrap();
        current.write_all(b"tail\n").unwrap();
        drop(current);

        let service = DiagnosticsService::new(directory.path()).unwrap();

        let previous = fs::read_to_string(&service.previous_path).unwrap();
        assert_eq!(previous.len() as u64, LOG_LIMIT_BYTES - 2);
        assert!(previous.ends_with("tail\n"));
        assert_eq!(service.current_path.metadata().unwrap().len(), 0);
    }

    #[test]
    fn diagnostics_rotation_failure_does_not_grow_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let service = DiagnosticsService::new(directory.path()).unwrap();
        fs::File::create(&service.current_path)
            .unwrap()
            .set_len(LOG_LIMIT_BYTES)
            .unwrap();
        fs::create_dir(&service.previous_path).unwrap();

        service.record("test", "rotation", "still available in memory");

        assert_eq!(
            service.current_path.metadata().unwrap().len(),
            LOG_LIMIT_BYTES
        );
        assert_eq!(service.entries.lock().unwrap().len(), 1);
        fs::remove_dir(&service.previous_path).unwrap();
        service.record("test", "recovered", "disk logging resumed");
        assert!(
            fs::read_to_string(&service.current_path)
                .unwrap()
                .contains("disk logging resumed")
        );
    }

    #[test]
    fn diagnostics_concurrent_writers_rotate_without_losing_entries() {
        let directory = tempfile::tempdir().unwrap();
        let service = DiagnosticsService::new(directory.path()).unwrap();
        fs::File::create(&service.current_path)
            .unwrap()
            .set_len(LOG_LIMIT_BYTES - 1)
            .unwrap();

        std::thread::scope(|scope| {
            for worker in 0..8 {
                let service = &service;
                scope.spawn(move || {
                    for entry in 0..100 {
                        service.record(
                            "test",
                            "concurrent",
                            format!("worker-{worker}-entry-{entry}"),
                        );
                    }
                });
            }
        });

        let current = fs::read_to_string(&service.current_path).unwrap();
        assert_eq!(current.lines().count(), 800);
        for worker in 0..8 {
            for entry in 0..100 {
                assert!(current.contains(&format!("worker-{worker}-entry-{entry}\n")));
            }
        }
        assert!((current.len() as u64) <= LOG_LIMIT_BYTES);
        assert_eq!(
            service.previous_path.metadata().unwrap().len(),
            LOG_LIMIT_BYTES - 1
        );
    }

    #[test]
    fn native_runtime_evidence_allowlists_bounds_and_redacts_probe_results() {
        let capabilities = json!({
            "protocolVersion": 7, "supportsVideoDecode": false, "supportsVideoPresent": "yes",
            "sessionId": "private-session", "accessToken": "private-access",
            "videoBackends": [{
                "backend": "v4l2", "platform": "linux", "available": false,
                "devicePath": "/home/alice/device", "unknown": "private-extra",
                "reason": "HEVC topology probe failed path=/home/alice/private Authorization: Bearer abc123 token=xyz password: hunter2 https://example.com user@example.com",
                "codecs": [{"codec": "h265", "available": false,
                    "reason": "MEDIA_IOC_G_TOPOLOGY failed", "secret": "private-codec"},
                    {"codec": "private-unknown-codec", "available": true}]
            }, {"backend": "private-unknown-backend", "available": true}]
        });
        let evidence = native_runtime_evidence(&capabilities);
        assert_eq!(evidence["supportsVideoDecode"], false);
        assert!(evidence.get("supportsVideoPresent").is_none());
        assert_eq!(evidence["videoBackends"].as_array().unwrap().len(), 1);
        let backend = &evidence["videoBackends"][0];
        assert_eq!(backend["available"], false);
        assert_eq!(
            backend["codecs"],
            json!([{"codec":"h265", "available":false, "reason":"MEDIA_IOC_G_TOPOLOGY failed"}])
        );
        let rendered = evidence.to_string();
        assert!(rendered.contains("HEVC topology probe failed"));
        for sensitive in [
            "private",
            "alice",
            "abc123",
            "xyz",
            "hunter2",
            "example.com",
        ] {
            assert!(
                !rendered.contains(sensitive),
                "{sensitive} leaked: {rendered}"
            );
        }
        let oversized = json!({"protocolVersion": u64::MAX, "videoBackends": vec![json!({
            "backend":"v4l2", "reason":"x".repeat(10000), "codecs":vec![json!({
                "codec":"h265", "available":"true", "reason":"y".repeat(10000)
            }); 100]
        }); 100]});
        let evidence = native_runtime_evidence(&oversized);
        assert!(evidence.get("protocolVersion").is_none());
        let backends = evidence["videoBackends"].as_array().unwrap();
        assert_eq!(backends.len(), 16);
        assert!(backends[0]["reason"].as_str().unwrap().len() <= 483);
        assert_eq!(backends[0]["codecs"].as_array().unwrap().len(), 8);
        assert!(backends[0]["codecs"][0].get("available").is_none());
        assert_eq!(
            native_runtime_evidence(&Value::Null),
            json!({"videoBackends":[]})
        );
        let indexed = native_runtime_evidence(&json!({
            "videoBackends":[],
            "graphicsAdapters":[
                {"name":"NVIDIA GeForce MX110","active":true,"codecs":[],"h265Main10":false,
                    "reason":"no supported hardware decoder profile","luid":"private-luid"},
                {"name":"Intel(R) HD Graphics 620 path=/home/alice/gpu","active":false,
                    "codecs":["h264","h265","private-codec"],"h265Main10":true}
            ]
        }));
        assert!(indexed.get("graphicsAdapters").is_some());
        assert_eq!(indexed["graphicsAdapters"][0]["codecs"], json!([]));
        assert_eq!(
            indexed["graphicsAdapters"][1]["codecs"],
            json!(["h264", "h265"])
        );
        assert_eq!(indexed["graphicsAdapters"][1]["h265Main10"], true);
        let rendered = indexed.to_string();
        assert!(rendered.contains("NVIDIA GeForce MX110"));
        assert!(!rendered.contains("private-luid"));
        assert!(!rendered.contains("alice"));
        assert!(!rendered.contains("private-codec"));
    }

    #[test]
    fn stream_profile_evidence_preserves_only_codec_and_color_fields() {
        let session = json!({
            "sessionId":"private-session", "accessToken":"private-token",
            "negotiatedStreamProfile":{"codec":"H265", "codecSource":"request", "colorQuality":"10bit_420", "enableHdr":true},
            "requestedStreamingFeatures":{"codec":"2", "bitDepth":1, "chromaFormat":0, "token":"private-token"},
            "finalizedStreamingFeatures":{"bitDepth":null, "chromaFormat":1, "password":"private-password"}
        });
        let evidence = stream_profile_evidence(&session);
        assert_eq!(evidence["codec"], "H265");
        assert_eq!(evidence["codecSource"], "request");
        assert_eq!(evidence["colorQuality"], "10bit_420");
        assert_eq!(evidence["enableHdr"], true);
        assert_eq!(
            evidence["requestedStreamingFeatures"],
            json!({"codec":2,"bitDepth":1,"chromaFormat":0})
        );
        assert_eq!(
            evidence["finalizedStreamingFeatures"],
            json!({"bitDepth":null,"chromaFormat":1})
        );
        assert!(!evidence.to_string().contains("private"));
        let invalid = stream_profile_evidence(&json!({
            "negotiatedStreamProfile":{"codec":"private-token", "codecSource":"private-token", "colorQuality":"private-token", "enableHdr":"private-token"},
            "requestedStreamingFeatures":{"codec":"private-token","bitDepth":{},"chromaFormat":["private-token"]}
        }));
        assert_eq!(invalid["codec"], Value::Null);
        assert_eq!(invalid["codecSource"], Value::Null);
        assert_eq!(invalid["colorQuality"], Value::Null);
        assert_eq!(invalid["enableHdr"], Value::Null);
        assert!(!invalid.to_string().contains("private"));
    }

    #[test]
    fn runtime_failure_reasons_redact_bearer_values_and_quoted_credentials() {
        for reason in [
            "probe failed Bearer private-value",
            r#"probe failed {"Authorization": "Bearer private-value"}"#,
            r#"probe failed access_token: "private-value""#,
            r"probe failed device=C:\Users\Alice\video path=/Users/Alice/video",
        ] {
            let redacted = runtime_failure_reason(reason);
            assert!(redacted.starts_with("probe failed"));
            assert!(!redacted.contains("private-value"));
            assert!(!redacted.contains("Alice"));
        }
        let evidence = native_runtime_evidence(&json!({"videoBackends":[{
            "backend":"software", "platform":"cross-platform", "available":true,
            "codecs":[{"codec":"h265","available":true},{"codec":"hevc","available":true}]
        }]}));
        assert_eq!(evidence["videoBackends"][0]["platform"], "cross-platform");
        assert_eq!(
            evidence["videoBackends"][0]["codecs"],
            json!([{"codec":"h265","available":true}])
        );
    }
}
