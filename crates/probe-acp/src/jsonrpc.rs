//! Minimal JSON-RPC 2.0 framing for newline-delimited ACP streams. Sans-I/O:
//! lines in, lines out; the transport (stdio in probe-bin, a JS host in
//! probe-wasm) moves the bytes.

use serde::{Deserialize, Serialize};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// The code the sarah-computer-controller treats as "authentication
/// required" and retries exactly once, non-interactively.
pub const AUTH_REQUIRED: i64 = -32000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// One inbound line, classified.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request { id: RequestId, method: String, params: serde_json::Value },
    Notification { method: String, params: serde_json::Value },
    Response { id: RequestId, result: Option<serde_json::Value>, error: Option<ErrorObject> },
    Invalid { error: ErrorObject },
}

pub fn parse_line(line: &str) -> Incoming {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Incoming::Invalid {
                error: ErrorObject { code: PARSE_ERROR, message: "parse error".into(), data: None },
            }
        }
    };
    let id = value.get("id").and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok());
    let method = value.get("method").and_then(|method| method.as_str()).map(str::to_string);
    let params = value.get("params").cloned().unwrap_or(serde_json::Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Incoming::Request { id, method, params },
        (None, Some(method)) => Incoming::Notification { method, params },
        (Some(id), None) => Incoming::Response {
            id,
            result: value.get("result").cloned(),
            error: value
                .get("error")
                .and_then(|error| serde_json::from_value::<ErrorObject>(error.clone()).ok()),
        },
        (None, None) => Incoming::Invalid {
            error: ErrorObject { code: INVALID_REQUEST, message: "invalid request".into(), data: None },
        },
    }
}

pub fn request_line(id: &RequestId, method: &str, params: &serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("serialize request")
}

pub fn notification_line(method: &str, params: &serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("serialize notification")
}

pub fn result_line(id: &RequestId, result: &serde_json::Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .expect("serialize result")
}

pub fn error_line(id: &RequestId, error: &ErrorObject) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    }))
    .expect("serialize error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_requests_notifications_and_responses() {
        assert!(matches!(
            parse_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
            Incoming::Request { .. }
        ));
        assert!(matches!(
            parse_line(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#),
            Incoming::Notification { .. }
        ));
        assert!(matches!(
            parse_line(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#),
            Incoming::Response { .. }
        ));
        assert!(matches!(parse_line("{nope"), Incoming::Invalid { .. }));
    }
}
