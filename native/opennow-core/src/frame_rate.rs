use serde_json::Value;

pub fn resolution_ceiling(width: i64, height: i64) -> i64 {
    if width == 1920 && matches!(height, 1080 | 1200) {
        360
    } else {
        240
    }
}

fn hardware_decode_available(settings: &Value, capabilities: &Value) -> bool {
    let requested_backend = crate::streamer::requested_embedded_backend(settings);
    let requested_codec = settings["codec"]
        .as_str()
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase();
    let explicit_codec = match requested_codec.as_str() {
        "" | "auto" => None,
        value => Some(value),
    };
    capabilities["videoBackends"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|backend| backend["available"].as_bool().unwrap_or(false))
        .filter(|backend| {
            let name = backend["backend"].as_str().unwrap_or("");
            !matches!(name, "software" | "ffmpeg")
                && (requested_backend == "auto"
                    || requested_backend == name
                    || (requested_backend == "nvdec" && name == "cuda"))
        })
        .flat_map(|backend| backend["codecs"].as_array().into_iter().flatten())
        .any(|entry| {
            if entry["available"].as_bool() != Some(true) {
                return false;
            }
            let name = entry["codec"].as_str().unwrap_or("");
            match &explicit_codec {
                Some(codec) => name.eq_ignore_ascii_case(codec),
                None => ["h264", "h265", "av1"]
                    .iter()
                    .any(|codec| name.eq_ignore_ascii_case(codec)),
            }
        })
}

const BASE_FRAME_RATE_CEILING: i64 = 240;

pub fn request_frame_rate(settings: &Value, params: &Value, width: i64, height: i64) -> i64 {
    let requested = settings["fps"].as_i64().unwrap_or(60).clamp(30, 360);
    let mut rate = requested.min(resolution_ceiling(width, height));
    if rate > BASE_FRAME_RATE_CEILING {
        let capabilities = &params["runtimeCapabilities"];
        let entitled = params["maxEntitledFps"].as_i64().unwrap_or(0).clamp(0, 360);
        let cap = if entitled > 0 {
            entitled.min(BASE_FRAME_RATE_CEILING)
        } else {
            BASE_FRAME_RATE_CEILING
        };
        if !hardware_decode_available(settings, capabilities) || entitled < rate {
            rate = cap;
        }
    }
    rate
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capabilities(backend: &str, codec: &str) -> Value {
        json!({"protocolVersion":7, "videoBackends":[{"backend":backend, "platform":"linux",
            "available":true, "codecs":[{"codec":codec, "available":true,
                "colorQualities":["8bit_420"]}]}]})
    }

    #[test]
    fn resolution_ceiling_is_documented_full_hd_only() {
        assert_eq!(resolution_ceiling(1920, 1080), 360);
        assert_eq!(resolution_ceiling(1920, 1200), 360);
        for (width, height) in [
            (1280, 720),
            (1600, 900),
            (1920, 1440),
            (2560, 1440),
            (2560, 1600),
            (3440, 1440),
            (3840, 2160),
            (5120, 1440),
            (3840, 1080),
            (2560, 1080),
        ] {
            assert_eq!(
                resolution_ceiling(width, height),
                240,
                "{width}x{height} must not request the full-HD-only tier"
            );
        }
    }

    #[test]
    fn request_rate_applies_the_resolution_device_and_entitlement_ceiling() {
        let hardware = capabilities("vaapi", "h265");
        let software = json!({"videoBackends":[{"backend":"software", "available":true,
            "codecs":[{"codec":"h265", "available":true}]}]});
        let settings = json!({"resolution":"1920x1080", "fps":360, "codec":"h265",
            "nativeVideoBackend":"auto"});
        let request = |capabilities: &Value, entitled: i64, width: i64, height: i64| {
            request_frame_rate(
                &settings,
                &json!({"runtimeCapabilities":capabilities, "maxEntitledFps":entitled}),
                width,
                height,
            )
        };
        assert_eq!(request(&hardware, 360, 1920, 1080), 360);
        assert_eq!(
            request(&software, 360, 1920, 1080),
            240,
            "a reported software-only probe cannot request the top tier"
        );
        assert_eq!(
            request(&json!({}), 360, 1920, 1080),
            240,
            "an unreported probe is not affirmative capability"
        );
        assert_eq!(
            request(&json!({"videoBackends":[]}), 360, 1920, 1080),
            240,
            "a probe that reported no backend cannot request the top tier"
        );
        assert_eq!(
            request(&hardware, 0, 1920, 1080),
            240,
            "unconfirmed entitlement cannot request the top tier"
        );
        assert_eq!(
            request(&hardware, 240, 1920, 1080),
            240,
            "a 240 FPS entitlement cannot request the top tier"
        );
        assert_eq!(
            request(&hardware, 120, 1920, 1080),
            120,
            "a lower entitlement bounds the request"
        );
        assert_eq!(
            request(&hardware, 360, 2560, 1440),
            240,
            "the resolution ceiling applies regardless of entitlement"
        );
        assert_eq!(
            request_frame_rate(
                &json!({"resolution":"1920x1080", "fps":999, "codec":"h265"}),
                &json!({"runtimeCapabilities":hardware, "maxEntitledFps":360}),
                1920,
                1080
            ),
            360,
            "a stored value above the ceiling is still bounded"
        );
        assert_eq!(
            request_frame_rate(
                &json!({"resolution":"1920x1080", "fps":120, "codec":"h265"}),
                &json!({"runtimeCapabilities":software, "maxEntitledFps":0}),
                1920,
                1080
            ),
            120,
            "base rates ignore the capability and entitlement verdicts"
        );
    }

    #[test]
    fn legacy_software_preference_never_qualifies_for_the_top_tier() {
        let hardware = capabilities("vaapi", "h264");
        let settings = json!({"resolution":"1920x1080", "fps":360, "codec":"h264",
            "nativeVideoBackend":"auto", "decoderPreference":"software"});
        let params = json!({"runtimeCapabilities":hardware, "maxEntitledFps":360});
        assert_eq!(
            request_frame_rate(&settings, &params, 1920, 1080),
            240,
            "the legacy software preference is a software decode path"
        );
        assert_eq!(
            request_frame_rate(
                &json!({"resolution":"1920x1080", "fps":360, "codec":"h264",
                    "nativeVideoBackend":"auto", "decoderPreference":"auto"}),
                &params,
                1920,
                1080
            ),
            360,
            "the same hardware profile stays eligible without the software preference"
        );
    }
}
