use crate::proxy::normalize_proxy_url;
use serde_json::{Map, Value, json};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct SettingsStore {
    path: PathBuf,
    values: Map<String, Value>,
    passthrough: Map<String, Value>,
}

impl SettingsStore {
    pub fn load(data_dir: Option<PathBuf>) -> io::Result<Self> {
        let path = data_dir
            .unwrap_or_else(default_data_dir)
            .join("settings.json");
        let defaults = defaults();
        let mut values = defaults.clone();
        let mut passthrough = Map::new();
        let mut recovered_backup = false;
        let backup = path.with_extension("json.bak");
        if path.exists() || backup.exists() {
            let persisted = read_persisted_settings(&path).or_else(|| {
                let persisted = read_persisted_settings(&backup);
                recovered_backup = persisted.is_some();
                persisted
            });
            match persisted {
                Some(persisted) => {
                    for (key, value) in persisted {
                        if defaults.contains_key(&key) {
                            values.insert(key, value);
                        } else if !matches!(key.as_str(), "nativeHdrSupported" | "nativeHdrDisplay")
                        {
                            passthrough.insert(key, value);
                        }
                    }
                }
                None => {
                    let corrupt_path = path.with_extension("json.corrupt");
                    let _ = fs::rename(&path, corrupt_path);
                }
            }
        }
        let mut store = Self {
            path,
            values,
            passthrough,
        };
        let codec_before_normalize = store.values["codec"].clone();
        let fallback_before_normalize = store.values["fallbackCodec"].clone();
        store.normalize();
        let codec_color_healed = store.values["codec"] != codec_before_normalize
            || store.values["fallbackCodec"] != fallback_before_normalize;
        if recovered_backup || (codec_color_healed && store.path.exists()) {
            store.save()?;
        }
        Ok(store)
    }

    pub fn all(&self) -> Value {
        Value::Object(self.values.clone())
    }

    pub fn set(&mut self, key: &str, mut value: Value) -> Result<Value, String> {
        if matches!(key, "providerRegions" | "regionProviderIdpId") {
            return Err("Provider region metadata is managed with the selected region".into());
        }
        if !defaults().contains_key(key) {
            return Err(format!("Unknown setting: {key}"));
        }
        if matches!(key, "codec" | "fallbackCodec") {
            if let Some(codec) = value.as_str() {
                let name = codec.trim().to_ascii_lowercase();
                let known_explicit =
                    matches!(name.as_str(), "h264" | "avc" | "h265" | "hevc" | "av1");
                if known_explicit {
                    let color = self.values["colorQuality"].as_str().unwrap_or("8bit_420");
                    if !crate::streamer::codec_supports_color_quality(&name, color) {
                        return Err(format!(
                            "{codec} cannot request {color}. Select Auto or H.265 for advanced color."
                        ));
                    }
                }
            }
        }
        if matches!(key, "gameLanguage" | "keyboardLayout") {
            crate::language::validate_setting(key, &value)?;
        }
        if key == "sessionProxyUrl" {
            let raw = value
                .as_str()
                .ok_or_else(|| "sessionProxyUrl must be a string".to_owned())?
                .trim();
            value = if raw.is_empty() {
                Value::String(String::new())
            } else {
                Value::String(normalize_proxy_url(raw)?.normalized_url)
            };
        }
        if key == "sessionProxyEnabled" && value.as_bool() == Some(true) {
            let raw = self.values["sessionProxyUrl"].as_str().unwrap_or("");
            normalize_proxy_url(raw)?;
        }
        let previous_values = self.values.clone();
        self.values.insert(key.to_owned(), value);
        self.normalize();
        if let Err(error) = self.save() {
            self.values = previous_values;
            return Err(format!("Could not save settings: {error}"));
        }
        Ok(self.values.get(key).cloned().unwrap_or(Value::Null))
    }

    pub fn reset(&mut self) -> Result<Value, String> {
        let previous_values = self.values.clone();
        self.values = defaults();
        self.normalize();
        if let Err(error) = self.save() {
            self.values = previous_values;
            return Err(format!("Could not reset settings: {error}"));
        }
        Ok(self.all())
    }

    pub fn set_provider_region(&mut self, provider: &str, value: Value) -> Result<Value, String> {
        validate_bounded_string(&value, "region", 256)?;
        if provider.is_empty() || provider.len() > 256 {
            return Err("Invalid region provider".into());
        }
        let previous_values = self.values.clone();
        let mut regions = self.values["providerRegions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if !regions.contains_key(provider) && regions.len() >= 32 {
            return Err("Too many saved region providers".into());
        }
        regions.insert(provider.to_owned(), value.clone());
        self.values
            .insert("providerRegions".into(), Value::Object(regions));
        self.values
            .insert("regionProviderIdpId".into(), json!(provider));
        self.values.insert("region".into(), value.clone());
        if let Err(error) = self.save() {
            self.values = previous_values;
            return Err(format!("Could not save region: {error}"));
        }
        Ok(value)
    }

    fn normalize(&mut self) {
        normalize_types(&mut self.values);
        normalize_resolution(&mut self.values);
        normalize_choice(
            &mut self.values,
            "nativeVideoBackend",
            &[
                "auto",
                "d3d11",
                "d3d12",
                "nvdec",
                "cuda",
                "vaapi",
                "v4l2",
                "vulkan",
                "videotoolbox",
                "software",
            ],
            "auto",
        );
        normalize_choice(
            &mut self.values,
            "nativeCloudGsyncMode",
            &["auto", "disabled", "forced"],
            "auto",
        );
        normalize_choice(
            &mut self.values,
            "codec",
            &["auto", "av1", "h264", "h265"],
            "auto",
        );
        normalize_choice(
            &mut self.values,
            "fallbackCodec",
            &["auto", "h264", "h265"],
            "auto",
        );
        normalize_choice(
            &mut self.values,
            "colorQuality",
            &["8bit_420", "10bit_420", "8bit_444", "10bit_444"],
            "8bit_420",
        );
        let color = self.values["colorQuality"]
            .as_str()
            .unwrap_or("8bit_420")
            .to_owned();
        for key in ["codec", "fallbackCodec"] {
            let codec = self.values[key].as_str().unwrap_or("auto");
            if !crate::streamer::codec_supports_color_quality(codec, &color) {
                self.values.insert(key.to_owned(), json!("auto"));
            }
        }
        normalize_choice(
            &mut self.values,
            "decoderPreference",
            &["auto", "hardware", "software"],
            "auto",
        );
        clamp_integer(&mut self.values, "fps", 30, 360, 60);
        clamp_bitrate_mbps(&mut self.values);
        normalize_bounded_strings(&mut self.values);
    }

    fn save(&self) -> io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        let backup = self.path.with_extension("json.bak");
        let mut persisted = self.passthrough.clone();
        persisted.extend(self.values.clone());
        let data = serde_json::to_vec_pretty(&persisted).map_err(io::Error::other)?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        if read_persisted_settings(&self.path).is_some() {
            fs::copy(&self.path, &backup)?;
            fs::OpenOptions::new()
                .write(true)
                .open(&backup)?
                .sync_all()?;
        }
        fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

fn read_persisted_settings(path: &Path) -> Option<Map<String, Value>> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn validate_bounded_string(value: &Value, key: &str, maximum_bytes: usize) -> Result<(), String> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.len() > maximum_bytes || value.contains('\0') {
        return Err(format!(
            "{key} must be at most {maximum_bytes} bytes without NUL characters"
        ));
    }
    Ok(())
}

fn normalize_resolution(values: &mut Map<String, Value>) {
    let valid = values["resolution"]
        .as_str()
        .and_then(|value| value.split_once('x'))
        .and_then(|(width, height)| Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?)))
        .is_some_and(|(width, height)| {
            (640..=7680).contains(&width)
                && (480..=4320).contains(&height)
                && width % 2 == 0
                && height % 2 == 0
        });
    if !valid {
        values.insert("resolution".to_owned(), json!("1920x1080"));
    }
}

pub fn resolve_data_dir(data_dir: Option<PathBuf>) -> PathBuf {
    data_dir.unwrap_or_else(|| {
        let primary = default_data_dir();
        select_existing_data_dir(primary.clone(), legacy_data_dirs(&primary))
    })
}

fn select_existing_data_dir(
    primary: PathBuf,
    legacy_candidates: impl IntoIterator<Item = PathBuf>,
) -> PathBuf {
    if primary.exists() {
        return primary;
    }
    legacy_candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .unwrap_or(primary)
}

fn normalize_choice(values: &mut Map<String, Value>, key: &str, choices: &[&str], fallback: &str) {
    let valid = values
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| choices.contains(&value));
    if !valid {
        values.insert(key.to_owned(), Value::String(fallback.to_owned()));
    }
}

fn clamp_integer(
    values: &mut Map<String, Value>,
    key: &str,
    minimum: i64,
    maximum: i64,
    fallback: i64,
) {
    let value = values
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or(fallback)
        .clamp(minimum, maximum);
    values.insert(key.to_owned(), Value::Number(value.into()));
}

fn clamp_bitrate_mbps(values: &mut Map<String, Value>) {
    // 0.22 Mbps is 220 kbps. Whole numbers stay integers so existing settings
    // and the 10–200 Mbps slider keep their previous JSON shape.
    let raw = values
        .get("maxBitrateMbps")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(75.0);
    let value = (raw.clamp(0.22, 200.0) * 100.0).round() / 100.0;
    let stored = if (value - value.round()).abs() < 1e-9 {
        Value::from(value.round() as i64)
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::from(75))
    };
    values.insert("maxBitrateMbps".to_owned(), stored);
}

fn normalize_types(values: &mut Map<String, Value>) {
    let expected = defaults();
    for (key, default) in expected {
        let valid = values.get(&key).is_some_and(|value| match &default {
            Value::Bool(_) => value.is_boolean(),
            Value::String(_) => value.is_string(),
            Value::Number(_) => value.is_number(),
            Value::Array(_) => value.is_array(),
            Value::Object(_) => value.is_object(),
            Value::Null => value.is_null() || value.is_number(),
        });
        if !valid {
            values.insert(key, default);
        }
    }
}

fn normalize_bounded_strings(values: &mut Map<String, Value>) {
    for (key, maximum) in [
        ("region", 256_usize),
        ("regionProviderIdpId", 256_usize),
        ("sessionProxyUrl", 2_048),
    ] {
        let value = values
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(maximum)
            .collect::<String>();
        values.insert(key.to_owned(), Value::String(value));
    }
}

fn default_data_dir() -> PathBuf {
    if let Some(path) = env::var_os("OPENNOW_DATA_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("OpenNOW");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/OpenNOW")
}

fn legacy_data_dirs(primary: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut candidates = Vec::new();
        if let Some(parent) = primary.parent() {
            candidates.push(parent.join("opennow"));
        }
        candidates
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = primary;
        Vec::new()
    }
}

fn defaults() -> Map<String, Value> {
    json!({
        "resolution":"1920x1080", "fps":60, "maxBitrateMbps":75, "saveBandwidth":false,
        "nativeVideoBackend":"auto", "nativeCloudGsyncMode":"auto", "enableCloudGsync":false,
        "codec":"auto", "fallbackCodec":"auto", "decoderPreference":"auto",
        "colorQuality":"8bit_420", "enableHdr":false,
        "region":"", "regionProviderIdpId":"", "providerRegions":{},
        "sessionProxyEnabled":false, "sessionProxyUrl":"", "networkTest":false,
        "keyboardLayout":"en-US", "gameLanguage":"en_US",
        "enablePersistingInGameSettings":true, "enableL4S":false, "identifyAsSteamDeck":false
    })
    .as_object()
    .cloned()
    .expect("settings defaults are an object")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn settings_backup_recovers_missing_and_corrupt_primary() {
        for corrupt in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("settings.json");
            let backup = path.with_extension("json.bak");
            let temporary = path.with_extension("json.tmp");
            let mut original = SettingsStore::load(Some(directory.path().to_owned())).unwrap();
            original.set("networkTest", json!(true)).unwrap();
            original.set("fps", json!(120)).unwrap();
            let expected = original.all();
            let bytes = fs::read(&path).unwrap();
            fs::rename(&path, &backup).unwrap();
            fs::write(&temporary, b"interrupted write").unwrap();
            if corrupt {
                fs::write(&path, b"{").unwrap();
            }
            assert_eq!(fs::read(&backup).unwrap(), bytes);
            assert_eq!(fs::read(&temporary).unwrap(), b"interrupted write");
            assert!(!path.with_extension("json.corrupt").exists());
            if corrupt {
                assert_eq!(fs::read(&path).unwrap(), b"{");
            } else {
                assert!(!path.exists());
            }
            let restored = SettingsStore::load(Some(directory.path().to_owned())).unwrap();
            assert_eq!(restored.all(), expected);
            assert_eq!(fs::read(&backup).unwrap(), bytes);
            assert_eq!(
                SettingsStore::load(Some(directory.path().to_owned()))
                    .unwrap()
                    .all(),
                expected
            );
        }
    }

    #[test]
    fn settings_recovery_failure_keeps_the_valid_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let backup = path.with_extension("json.bak");
        let mut store = SettingsStore::load(Some(directory.path().to_owned())).unwrap();
        store.set("fps", json!(120)).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::rename(&path, &backup).unwrap();
        fs::create_dir(path.with_extension("json.tmp")).unwrap();
        assert!(SettingsStore::load(Some(directory.path().to_owned())).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(&backup).unwrap(), bytes);
    }

    #[test]
    fn settings_backup_failure_keeps_the_primary_and_memory_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::load(Some(directory.path().to_owned())).unwrap();
        store.set("fps", json!(120)).unwrap();
        let expected = store.all();
        let bytes = fs::read(&path).unwrap();
        fs::create_dir(path.with_extension("json.bak")).unwrap();
        assert!(store.set("fps", json!(144)).is_err());
        assert_eq!(store.all(), expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn language_preferences_are_independent_and_rejected_writes_are_atomic() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SettingsStore::load(Some(directory.path().to_path_buf())).unwrap();
        for (key, value) in [("gameLanguage", "es_419"), ("keyboardLayout", "ja-JP")] {
            store.set(key, json!(value)).unwrap();
        }
        let saved = store.all();
        for (key, value) in [
            ("gameLanguage", "auto"),
            ("gameLanguage", "system"),
            ("gameLanguage", "en\nUS"),
            ("keyboardLayout", "en_US"),
            ("keyboardLayout", "m-us"),
        ] {
            assert!(store.set(key, json!(value)).is_err());
            assert_eq!(store.all(), saved);
        }
        assert_eq!(
            SettingsStore::load(Some(directory.path().to_path_buf()))
                .unwrap()
                .all(),
            saved
        );
        store.set("keyboardLayout", json!("en-GB")).unwrap();
        assert_eq!(store.all()["gameLanguage"], "es_419");
        store.set("gameLanguage", json!("future_001")).unwrap();
        assert_eq!(store.all()["keyboardLayout"], "en-GB");
        std::fs::create_dir(store.path.with_extension("json.tmp")).unwrap();
        let saved = store.all();
        assert!(store.set("gameLanguage", json!("pt_BR")).is_err());
        assert_eq!(store.all(), saved);
    }

    #[test]
    fn restored_language_ids_are_not_rewritten_but_corrupt_values_never_reach_requests() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            json!({
                "gameLanguage":"auto", "keyboardLayout":"unknown"
            })
            .to_string(),
        )
        .unwrap();
        let settings = SettingsStore::load(Some(directory.path().to_path_buf()))
            .unwrap()
            .all();
        assert_eq!(settings["gameLanguage"], "auto");
        assert_eq!(settings["keyboardLayout"], "unknown");
        let mut url = url::Url::parse("https://fixture.invalid/").unwrap();
        crate::language::append_session_preferences(&mut url, &settings);
        assert_eq!(url.query(), Some("keyboardLayout=en-US&languageCode=en_US"));
    }

    #[test]
    fn provider_region_preferences_are_atomic_isolated_and_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SettingsStore::load(Some(directory.path().to_path_buf())).unwrap();
        store
            .set_provider_region("nvidia", json!("https://nvidia-region.nvidiagrid.net/"))
            .unwrap();
        store
            .set_provider_region("alliance", json!("https://alliance-region.nvidiagrid.net/"))
            .unwrap();
        let restored = SettingsStore::load(Some(directory.path().to_path_buf()))
            .unwrap()
            .all();
        assert_eq!(
            restored["providerRegions"]["nvidia"],
            "https://nvidia-region.nvidiagrid.net/"
        );
        assert_eq!(restored["providerRegions"]["alliance"], restored["region"]);
        assert_eq!(restored["regionProviderIdpId"], "alliance");
        assert!(store.set("providerRegions", json!({})).is_err());
        assert!(
            store
                .set_provider_region("alliance", json!("x".repeat(257)))
                .is_err()
        );
        assert_eq!(store.all(), restored);
        std::fs::create_dir(store.path.with_extension("json.tmp")).unwrap();
        assert!(
            store
                .set_provider_region("nvidia", json!("changed"))
                .is_err()
        );
        assert_eq!(store.all(), restored);
    }

    #[test]
    fn in_game_settings_persistence_survives_restart_and_resets() {
        let directory = tempfile::tempdir().unwrap();
        let load = || SettingsStore::load(Some(directory.path().to_owned())).unwrap();
        let mut store = load();
        assert_eq!(store.all()["enablePersistingInGameSettings"], true);
        fs::write(directory.path().join("settings.json"), br#"{"fps":120}"#).unwrap();
        store = load();
        assert_eq!(store.all()["enablePersistingInGameSettings"], true);
        for enabled in [true, false, true] {
            store
                .set("enablePersistingInGameSettings", json!(enabled))
                .unwrap();
            store = load();
            assert_eq!(store.all()["enablePersistingInGameSettings"], enabled);
            store.set("fps", json!(120)).unwrap();
            store = load();
            assert_eq!(store.all()["enablePersistingInGameSettings"], enabled);
        }
        fs::create_dir(directory.path().join("settings.json.tmp")).unwrap();
        assert!(
            store
                .set("enablePersistingInGameSettings", json!(false))
                .is_err()
        );
        assert_eq!(store.all()["enablePersistingInGameSettings"], true);
        assert_eq!(load().all()["enablePersistingInGameSettings"], true);
        fs::remove_dir(directory.path().join("settings.json.tmp")).unwrap();
        store
            .set("enablePersistingInGameSettings", json!(false))
            .unwrap();
        store.reset().unwrap();
        assert_eq!(load().all()["enablePersistingInGameSettings"], true);
    }

    #[test]
    fn hdr_opt_in_persists_but_runtime_output_capability_does_not() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-hdr-settings-{unique}"));
        let mut store = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(store.all()["enableHdr"], false);
        assert!(store.set("nativeHdrSupported", json!(true)).is_err());
        assert!(
            store
                .set(
                    "nativeHdrDisplay",
                    json!({"minimumNits":0.005,"maximumNits":620,
                        "maximumFullFrameNits":400,"redX":0.64,"redY":0.33,
                        "greenX":0.30,"greenY":0.60,"blueX":0.15,"blueY":0.06,
                        "whiteX":0.3127,"whiteY":0.329})
                )
                .is_err()
        );
        assert_eq!(store.set("enableHdr", json!(true)).unwrap(), true);
        assert_eq!(
            SettingsStore::load(Some(directory.clone())).unwrap().all()["enableHdr"],
            true
        );
        let path = directory.join("settings.json");
        let mut persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        persisted["nativeHdrSupported"] = json!(true);
        persisted["nativeHdrDisplay"] = json!({"minimumNits":0.005,"maximumNits":620,
            "maximumFullFrameNits":400,"redX":0.64,"redY":0.33,
            "greenX":0.30,"greenY":0.60,"blueX":0.15,"blueY":0.06,
            "whiteX":0.3127,"whiteY":0.329});
        fs::write(&path, serde_json::to_vec(&persisted).unwrap()).unwrap();
        let mut loaded = SettingsStore::load(Some(directory.clone())).unwrap();
        assert!(loaded.all().get("nativeHdrSupported").is_none());
        assert!(loaded.all().get("nativeHdrDisplay").is_none());
        loaded.set("enableHdr", json!(true)).unwrap();
        let persisted: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(persisted.get("nativeHdrSupported").is_none());
        assert!(persisted.get("nativeHdrDisplay").is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persists_and_normalizes_settings() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-core-settings-{unique}"));
        let mut store = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(store.set("fps", json!(999)).unwrap(), json!(360));
        assert_eq!(store.set("fps", json!(360)).unwrap(), json!(360));
        assert_eq!(store.set("fps", json!(240)).unwrap(), json!(240));
        assert_eq!(store.set("maxBitrateMbps", json!(200)).unwrap(), json!(200));
        let low_bitrate = store.set("maxBitrateMbps", json!(0.22)).unwrap();
        assert!((low_bitrate.as_f64().unwrap() - 0.22).abs() < 0.001);
        let clamped_bitrate = store.set("maxBitrateMbps", json!(0.1)).unwrap();
        assert!((clamped_bitrate.as_f64().unwrap() - 0.22).abs() < 0.001);
        assert_eq!(store.set("maxBitrateMbps", json!(27)).unwrap(), json!(27));
        assert_eq!(store.set("maxBitrateMbps", json!(200)).unwrap(), json!(200));
        assert_eq!(
            store.set("saveBandwidth", json!(true)).unwrap(),
            json!(true)
        );
        assert_eq!(
            store.set("saveBandwidth", json!("yes")).unwrap(),
            json!(false)
        );
        assert_eq!(
            store.set("saveBandwidth", json!(true)).unwrap(),
            json!(true)
        );
        let loaded = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(loaded.all()["fps"], json!(240));
        assert_eq!(loaded.all()["maxBitrateMbps"], json!(200));
        assert_eq!(loaded.all()["saveBandwidth"], json!(true));
        assert!(store.set("notASetting", json!(true)).is_err());
        assert!(store.set("themePack", json!("chapel")).is_err());
        assert_eq!(store.set("codec", json!("invalid")).unwrap(), json!("auto"));
        assert_eq!(
            store.set("resolution", json!("3440x1440")).unwrap(),
            json!("3440x1440")
        );
        assert_eq!(
            store.set("resolution", json!("99999x1")).unwrap(),
            json!("1920x1080")
        );
        assert!(store.set("sessionProxyEnabled", json!(true)).is_err());
        assert_eq!(
            store
                .set("sessionProxyUrl", json!("proxy.example:8080"))
                .unwrap(),
            json!("http://proxy.example:8080/")
        );
        assert_eq!(
            store.set("sessionProxyEnabled", json!(true)).unwrap(),
            json!(true)
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn existing_legacy_profile_wins_only_when_primary_is_absent() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("opennow-core-profile-{unique}"));
        let primary = root.join("OpenNOW");
        let legacy = root.join("legacy-opennow");

        fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            select_existing_data_dir(primary.clone(), [legacy.clone()]),
            legacy
        );

        fs::create_dir_all(&primary).unwrap();
        assert_eq!(select_existing_data_dir(primary.clone(), [legacy]), primary);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn historical_profile_spelling_respects_filesystem_case_sensitivity() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("opennow-core-profile-case-{unique}"));
        let primary = root.join("OpenNOW");
        let legacy = root.join("opennow");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("settings.json"), b"{}").unwrap();

        let expected = if primary.canonicalize().is_ok() {
            primary.clone()
        } else {
            legacy.clone()
        };
        let selected = select_existing_data_dir(primary, [legacy.clone()]);
        assert_eq!(selected, expected);
        assert_eq!(fs::read(selected.join("settings.json")).unwrap(), b"{}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_fields_survive_a_save_without_being_exposed() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-core-legacy-settings-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("settings.json"),
            serde_json::to_vec_pretty(&json!({
                "fps": 120,
                "transportMode": "webrtc",
                "futureSetting": {"enabled": true}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut store = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(store.all()["fps"], json!(120));
        assert!(store.all().get("transportMode").is_none());
        assert!(store.all().get("futureSetting").is_none());
        store.set("codec", json!("h264")).unwrap();

        let persisted: Value =
            serde_json::from_slice(&fs::read(directory.join("settings.json")).unwrap()).unwrap();
        assert_eq!(persisted["futureSetting"], json!({"enabled": true}));
        assert_eq!(persisted["transportMode"], json!("webrtc"));
        assert_eq!(persisted["codec"], json!("h264"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn incompatible_saved_codec_color_combo_heals_to_auto_on_first_launch() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-codec-color-heal-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("settings.json"),
            r#"{"codec":"av1","fallbackCodec":"h264","colorQuality":"10bit_444"}"#,
        )
        .unwrap();

        let store = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(store.all()["colorQuality"], json!("10bit_444"));
        assert_eq!(store.all()["codec"], json!("auto"));
        assert_eq!(store.all()["fallbackCodec"], json!("auto"));

        // The repair persists so the next launch starts from a valid profile.
        let reloaded = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(reloaded.all()["codec"], json!("auto"));
        assert_eq!(reloaded.all()["fallbackCodec"], json!("auto"));
        assert_eq!(reloaded.all()["colorQuality"], json!("10bit_444"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn compatible_saved_codec_color_combo_survives_reload_unchanged() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-codec-color-keep-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("settings.json"),
            r#"{"codec":"h265","fallbackCodec":"auto","colorQuality":"10bit_444"}"#,
        )
        .unwrap();

        let store = SettingsStore::load(Some(directory.clone())).unwrap();
        assert_eq!(store.all()["codec"], json!("h265"));
        assert_eq!(store.all()["fallbackCodec"], json!("auto"));
        assert_eq!(store.all()["colorQuality"], json!("10bit_444"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn color_change_heals_an_incompatible_explicit_codec() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-color-change-heal-{unique}"));
        let mut store = SettingsStore::load(Some(directory.clone())).unwrap();
        store.set("codec", json!("av1")).unwrap();
        store.set("colorQuality", json!("10bit_444")).unwrap();
        assert_eq!(store.all()["colorQuality"], json!("10bit_444"));
        assert_eq!(store.all()["codec"], json!("auto"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_codec_selection_rejects_color_incompatible_values() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("opennow-codec-reject-{unique}"));
        let mut store = SettingsStore::load(Some(directory.clone())).unwrap();
        store.set("colorQuality", json!("10bit_444")).unwrap();
        for codec in ["av1", "h264"] {
            let error = store.set("codec", json!(codec)).unwrap_err();
            assert!(error.contains(codec), "{error}");
            assert_eq!(store.all()["codec"], json!("auto"));
        }
        for codec in ["auto", "h265"] {
            store.set("codec", json!(codec)).unwrap();
            assert_eq!(store.all()["codec"], json!(codec));
        }
        let error = store.set("fallbackCodec", json!("h264")).unwrap_err();
        assert!(error.contains("h264"), "{error}");
        store.set("colorQuality", json!("8bit_420")).unwrap();
        store.set("codec", json!("h264")).unwrap();
        store.set("fallbackCodec", json!("h264")).unwrap();
        assert_eq!(store.all()["codec"], json!("h264"));
        assert_eq!(store.all()["fallbackCodec"], json!("h264"));
        fs::remove_dir_all(directory).unwrap();
    }
}
