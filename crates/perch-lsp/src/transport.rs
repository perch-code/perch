//! The base protocol: `Content-Length` framed JSON over a pipe.
//!
//! Hand-rolled rather than taken from a framework. The client needs seven
//! requests and three notifications, and owning the loop keeps the shutdown
//! path and the request-to-response matching explicit.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Read one message's header block and return the body length.
///
/// Returns `None` when the block has no length that parses. Headers are
/// ASCII, case-insensitive, and terminated by a blank line. A line that
/// is no header, a stray debug line a server printed, is skipped.
pub fn parse_header_block(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

/// Frame a JSON body for sending.
pub fn frame(body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 48);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

/// What a decoded message turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A response to a request we sent.
    Response { id: i64, result: Result<Value, String> },
    /// A notification from the server.
    Notification { method: String, params: Value },
    /// A request from the server, which must be answered.
    Request { id: Value, method: String },
    /// Something we do not model. Ignored rather than fatal.
    Other,
}

pub fn decode(body: &str) -> Incoming {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Incoming::Other;
    };

    let method = value.get("method").and_then(Value::as_str);
    let id = value.get("id");

    match (method, id) {
        // A request from the server carries both a method and an id.
        (Some(method), Some(id)) => Incoming::Request {
            id: id.clone(),
            method: method.to_string(),
        },
        (Some(method), None) => Incoming::Notification {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        },
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else {
                return Incoming::Other;
            };
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed")
                    .to_string();
                return Incoming::Response { id, result: Err(message) };
            }
            Incoming::Response {
                id,
                result: Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            }
        }
        (None, None) => Incoming::Other,
    }
}

/// Decode a response payload into a typed value.
///
/// A null result is `None` rather than an error: the protocol uses it for
/// "no hover here" and "no definition found", which are ordinary answers.
pub fn decode_result<T: DeserializeOwned>(value: Value) -> Option<T> {
    if value.is_null() {
        return None;
    }
    serde_json::from_value(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_is_read_case_insensitively() {
        assert_eq!(parse_header_block("Content-Length: 42"), Some(42));
        assert_eq!(parse_header_block("content-length: 7"), Some(7));
    }

    #[test]
    fn other_headers_are_skipped() {
        let headers = "Content-Type: application/vscode-jsonrpc\r\nContent-Length: 19";
        assert_eq!(parse_header_block(headers), Some(19));
    }

    #[test]
    fn a_header_block_without_a_length_is_rejected() {
        assert_eq!(parse_header_block("Content-Type: x"), None);
        assert_eq!(parse_header_block("starting up\r\nContent-Type: x"), None);
        assert_eq!(parse_header_block("Content-Length: lots"), None);
    }

    #[test]
    fn a_line_that_is_no_header_is_skipped() {
        assert_eq!(parse_header_block("starting up\r\nContent-Length: 19"), Some(19));
        assert_eq!(parse_header_block("\r\nContent-Length: 19"), Some(19));
        // Printed with a bare newline, before the headers proper.
        assert_eq!(parse_header_block("loading config\nContent-Length: 19"), Some(19));
    }

    #[test]
    fn framing_counts_bytes_not_characters() {
        // A multi-byte character must not make the length disagree with the
        // payload, which would desynchronise the stream permanently.
        let body = r#"{"a":"é"}"#;
        let framed = frame(body);
        let text = String::from_utf8(framed).unwrap();
        let (headers, payload) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(parse_header_block(headers), Some(body.len()));
        assert_eq!(payload, body);
        assert_ne!(body.len(), body.chars().count());
    }

    #[test]
    fn a_result_is_matched_to_its_request() {
        let m = decode(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#);
        match m {
            Incoming::Response { id, result } => {
                assert_eq!(id, 7);
                assert!(result.is_ok());
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn an_error_response_carries_its_message() {
        let m = decode(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"nope"}}"#);
        match m {
            Incoming::Response { result: Err(message), .. } => assert_eq!(message, "nope"),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn notifications_are_distinguished_from_requests_by_their_id() {
        let n = decode(r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{}}"#);
        assert!(matches!(n, Incoming::Notification { .. }));

        // rust-analyzer asks the client for things; those carry an id and
        // must be answered or the server waits forever.
        let r = decode(r#"{"jsonrpc":"2.0","id":1,"method":"window/workDoneProgress/create"}"#);
        assert!(matches!(r, Incoming::Request { .. }));
    }

    #[test]
    fn a_null_result_means_no_answer_not_a_failure() {
        assert_eq!(decode_result::<String>(Value::Null), None);
    }

    #[test]
    fn malformed_json_is_ignored_rather_than_fatal() {
        assert_eq!(decode("not json at all"), Incoming::Other);
    }
}
