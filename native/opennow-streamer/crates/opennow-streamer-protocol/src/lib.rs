use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub mod log;
pub mod text_input;

pub const PROTOCOL_VERSION: u64 = 7;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub protocol_version: Option<u64>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub payload_base64: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub session: Session,
    pub settings: Value,
    pub shortcuts: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nvst_video: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_session_id: Option<String>,
    pub server_ip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_connection_info: Option<MediaConnectionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_info: Option<Vec<ConnectionInfo>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    pub port: u32,
    pub usage: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_level_protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_path: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaConnectionInfo {
    pub ip: String,
    pub port: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub protocol_version: u64,
    pub backend: &'static str,
    pub supports_input: bool,
    pub supports_microphone: bool,
    pub supports_owned_nvst_negotiation: bool,
}

pub fn response(id: impl Into<String>, kind: &str) -> Value {
    serde_json::json!({ "id": id.into(), "type": kind })
}

pub fn error(id: Option<&str>, code: &str, message: impl Into<String>) -> Value {
    let mut value = serde_json::json!({
        "type": "error",
        "code": code,
        "message": message.into(),
    });
    if let Some(id) = id {
        value["id"] = Value::String(id.to_owned());
    }
    value
}

pub fn event(kind: &str, fields: Value) -> Value {
    let mut object = fields.as_object().cloned().unwrap_or_default();
    object.insert("type".to_owned(), Value::String(kind.to_owned()));
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forward_compatible_commands() {
        let command: Command = serde_json::from_value(serde_json::json!({
            "id": "1",
            "type": "start",
            "context": { "session": { "sessionId": "session" } },
            "futureField": true
        }))
        .expect("command");
        assert_eq!(command.kind, "start");
        assert!(command.context.is_some());
    }

    #[test]
    fn unsolicited_errors_do_not_serialize_a_null_request_id() {
        let value = error(None, "invalid-command", "bad JSON");
        assert!(value.get("id").is_none());
        assert_eq!(value["type"], "error");
    }

    #[test]
    fn session_context_round_trips_required_and_forward_compatible_fields() {
        let fixture = serde_json::json!({
            "session": {
                "sessionId": "synthetic-session",
                "subSessionId": "synthetic-subsession",
                "serverIp": "127-0-0-1.synthetic.invalid",
                "iceServers": [],
                "mediaConnectionInfo": {
                    "ip": "198.51.100.20",
                    "port": 18_784,
                    "usage": 17,
                    "futureEndpointField": true
                },
                "connectionInfo": [
                    {
                        "ip": "198.51.100.10",
                        "port": 443,
                        "usage": 14,
                        "protocol": 1,
                        "resourcePath": "/nvst/"
                    },
                    {
                        "ip": "198.51.100.20",
                        "port": 48322,
                        "usage": 16,
                        "protocol": 1,
                        "appLevelProtocol": 6,
                        "resourcePath": "rtsps://198.51.100.20:48322/session",
                        "futureConnectionField": true
                    }
                ],
                "futureSessionField": "preserved"
            },
            "settings": { "codec": "H264", "fps": 60 },
            "shortcuts": { "stopStream": "Ctrl+Shift+Q" },
            "futureContextField": 42
        });

        let context: SessionContext = serde_json::from_value(fixture.clone()).expect("context");
        assert_eq!(context.session.session_id, "synthetic-session");
        assert_eq!(
            serde_json::to_value(context).expect("serializable context"),
            fixture
        );
    }
}
