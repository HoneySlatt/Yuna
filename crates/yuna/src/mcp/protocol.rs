//! MCP's messages: JSON-RPC 2.0, one message per line, read into requests and notifications and
//! answered in the revision each request speaks ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md),
//! [survey](../../../../docs/survey.md#agents-and-mcp)). No input or output here: lines in,
//! JSON values out.
//!
//! Two revisions are served:
//!
//! - **2026-07-28**, stateless. Every request carries its revision and the client's capabilities
//!   in `_meta`; the server answers `server/discover`, `tools/list` and `tools/call`; each
//!   result has `resultType` and the server's identity in its `_meta`. A request naming another
//!   revision in `_meta` gets error -32022, which lists the one supported.
//! - **2025-11-25**, for clients that open with `initialize`. Once one has, a request without
//!   `_meta`'s revision is served under it: `tools/list`, `tools/call` and `ping`. That choice is
//!   the only state kept across requests.
//!
//! A request without a revision before any `initialize` is malformed (-32602), as 2026-07-28
//! has it. Notifications are never answered; `notifications/cancelled` names the request to
//! drop.

use serde_json::{Map, Value, json};

/// The stateless revision, named in each request's `_meta`.
pub const MODERN_REVISION: &str = "2026-07-28";

/// The revision a client that opens with `initialize` is answered with, whatever it asked: the
/// one before 2026-07-28, which the legacy handshake negotiates.
pub const LEGACY_REVISION: &str = "2025-11-25";

/// `_meta`'s key for a request's revision.
const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta`'s key for the client's capabilities, required on every 2026-07-28 request.
const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
/// `_meta`'s key for the server's identity, in every 2026-07-28 result.
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// How long a client may keep the list of tools, or the discovery result, before asking again:
/// an hour, as the specification's example. Neither changes while Yuna runs.
const CACHE_TTL_MS: u64 = 3_600_000;

/// JSON-RPC's code for a line that is not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC's code for JSON that is not a request or a notification.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC's code for a method the server does not have.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's code for parameters a method cannot take; MCP's for an unknown tool, and for a
/// request without its required `_meta`.
pub const INVALID_PARAMS: i64 = -32602;
/// MCP's code for a revision the server does not speak.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// The revision a request is served under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    /// 2026-07-28.
    Modern,
    /// 2025-11-25, after `initialize`.
    Legacy,
}

/// A message read from the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A request, to answer once, with its id: a string or an integer.
    Request {
        id: Value,
        method: String,
        params: Map<String, Value>,
    },
    /// A notification, never answered.
    Notification {
        method: String,
        params: Map<String, Value>,
    },
    /// A response: the server sends no request, so there is nothing to match it with.
    Response,
}

/// An error to answer a request with.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// What a request asks, once its revision is known.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// `server/discover` (2026-07-28).
    Discover,
    /// `initialize` (2025-11-25).
    Initialize,
    /// `ping` (2025-11-25).
    Ping,
    /// `tools/list`.
    ListTools,
    /// `tools/call`: a tool's name and its arguments, an empty object when none came.
    Tool {
        name: String,
        arguments: Map<String, Value>,
    },
}

/// Reads one line. A line that is not a JSON-RPC message gives the error response to send,
/// with the request's id when it could be read, `null` otherwise. Batches, which no revision
/// served here allows, are refused as invalid.
pub fn read_message(line: &[u8]) -> Result<Message, Value> {
    let value: Value = serde_json::from_slice(line).map_err(|error| {
        error_response(
            Value::Null,
            &RpcError::new(PARSE_ERROR, format!("not JSON: {error}")),
        )
    })?;
    let Value::Object(mut object) = value else {
        return Err(error_response(
            Value::Null,
            &RpcError::new(INVALID_REQUEST, "a message is one JSON object"),
        ));
    };
    let id = object.remove("id");
    let invalid = |id: Option<&Value>, message: &str| {
        let id = id
            .filter(|id| is_valid_id(id))
            .cloned()
            .unwrap_or(Value::Null);
        Err(error_response(id, &RpcError::new(INVALID_REQUEST, message)))
    };
    if object.get("jsonrpc") != Some(&Value::String("2.0".to_owned())) {
        return invalid(id.as_ref(), "\"jsonrpc\" must be \"2.0\"");
    }
    let method = match object.remove("method") {
        Some(Value::String(method)) => method,
        Some(_) => return invalid(id.as_ref(), "\"method\" must be a string"),
        None if object.contains_key("result") || object.contains_key("error") => {
            return Ok(Message::Response);
        }
        None => return invalid(id.as_ref(), "a request needs a \"method\""),
    };
    let params = match object.remove("params") {
        None => Map::new(),
        Some(Value::Object(params)) => params,
        Some(_) => return invalid(id.as_ref(), "\"params\" must be an object"),
    };
    match id {
        None => Ok(Message::Notification { method, params }),
        Some(id) if is_valid_id(&id) => Ok(Message::Request { id, method, params }),
        Some(id) => invalid(
            Some(&id),
            "a request's \"id\" must be a string or an integer, never null",
        ),
    }
}

/// Whether `id` may identify a request: a string or an integer.
fn is_valid_id(id: &Value) -> bool {
    match id {
        Value::String(_) => true,
        Value::Number(number) => number.is_i64() || number.is_u64(),
        _ => false,
    }
}

/// The revisions' state: whether a client opened with `initialize`.
#[derive(Debug, Default)]
pub struct Session {
    legacy: bool,
}

impl Session {
    /// The revision `method` is served under and what it asks, or the error to answer it
    /// with. See the module's documentation.
    pub fn route(
        &mut self,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<(Revision, Call), RpcError> {
        let meta = params.get("_meta").and_then(Value::as_object);
        match meta.and_then(|meta| meta.get(PROTOCOL_VERSION_KEY)) {
            Some(version) => {
                let version = version.as_str().ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("_meta's \"{PROTOCOL_VERSION_KEY}\" must be a string"),
                    )
                })?;
                if version != MODERN_REVISION {
                    return Err(unsupported_revision(version));
                }
                if !meta
                    .and_then(|meta| meta.get(CLIENT_CAPABILITIES_KEY))
                    .is_some_and(Value::is_object)
                {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!("_meta needs \"{CLIENT_CAPABILITIES_KEY}\", an object"),
                    ));
                }
                let call = match method {
                    "server/discover" => Call::Discover,
                    "tools/list" => Call::ListTools,
                    "tools/call" => read_tool_call(params)?,
                    _ => return Err(method_not_found(method)),
                };
                Ok((Revision::Modern, call))
            }
            None if method == "initialize" => {
                if !params.get("protocolVersion").is_some_and(Value::is_string) {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "initialize needs \"protocolVersion\", a string",
                    ));
                }
                self.legacy = true;
                Ok((Revision::Legacy, Call::Initialize))
            }
            None if self.legacy => {
                let call = match method {
                    "ping" => Call::Ping,
                    "tools/list" => Call::ListTools,
                    "tools/call" => read_tool_call(params)?,
                    _ => return Err(method_not_found(method)),
                };
                Ok((Revision::Legacy, call))
            }
            None => Err(RpcError::new(
                INVALID_PARAMS,
                format!(
                    "a request needs _meta's \"{PROTOCOL_VERSION_KEY}\" ({MODERN_REVISION}), \
                     unless the client opened with initialize ({LEGACY_REVISION})"
                ),
            )),
        }
    }
}

fn method_not_found(method: &str) -> RpcError {
    RpcError::new(METHOD_NOT_FOUND, format!("no method {method:?}"))
}

/// -32022, with the revisions a request may name in `_meta`: 2026-07-28 alone. 2025-11-25 is
/// reached through `initialize`, never through `_meta`; named here, it would lead a client to
/// retry with it in `_meta`, and be refused again.
fn unsupported_revision(requested: &str) -> RpcError {
    RpcError {
        code: UNSUPPORTED_PROTOCOL_VERSION,
        message: "Unsupported protocol version".to_owned(),
        data: Some(json!({ "supported": [MODERN_REVISION], "requested": requested })),
    }
}

fn read_tool_call(params: &Map<String, Value>) -> Result<Call, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call needs \"name\", a string"))?;
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(arguments)) => arguments.clone(),
        Some(_) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "tools/call's \"arguments\" must be an object",
            ));
        }
    };
    Ok(Call::Tool {
        name: name.to_owned(),
        arguments,
    })
}

/// The cancelled request's id, from `notifications/cancelled`'s parameters; none when they name
/// no valid id, which is ignored, as MCP asks.
pub fn cancelled_request(params: &Map<String, Value>) -> Option<&Value> {
    params.get("requestId").filter(|id| is_valid_id(id))
}

/// A JSON-RPC response carrying `result`.
pub fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC error response.
pub fn error_response(id: Value, error: &RpcError) -> Value {
    let mut body = json!({ "code": error.code, "message": error.message });
    if let Some(data) = &error.data {
        body["data"] = data.clone();
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": body })
}

/// The server's name and version.
fn server_info() -> Value {
    json!({ "name": "yuna", "version": env!("CARGO_PKG_VERSION") })
}

/// `result` as `revision` shapes it: in 2026-07-28, with `resultType` and the server's
/// identity; in 2025-11-25, as it is.
pub fn result(revision: Revision, mut result: Map<String, Value>) -> Value {
    if revision == Revision::Modern {
        result.insert("resultType".to_owned(), json!("complete"));
        let meta = result
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(meta) = meta {
            meta.insert(SERVER_INFO_KEY.to_owned(), server_info());
        }
    }
    Value::Object(result)
}

/// What the server tells a model about itself.
const INSTRUCTIONS: &str = "Yuna is a Wayland compositor of your own, without a screen. \
    `spawn` starts a program in it; its windows open on Yuna's virtual outputs (`outputs`) and \
    `windows` lists them, waiting for one if asked. Started with --manual-clock, Yuna's time \
    moves only through `clock`.";

/// `server/discover`'s result.
pub fn discover_result() -> Value {
    let mut body = Map::new();
    body.insert("supportedVersions".to_owned(), json!([MODERN_REVISION]));
    body.insert("capabilities".to_owned(), json!({ "tools": {} }));
    body.insert("instructions".to_owned(), json!(INSTRUCTIONS));
    body.insert("ttlMs".to_owned(), json!(CACHE_TTL_MS));
    body.insert("cacheScope".to_owned(), json!("public"));
    result(Revision::Modern, body)
}

/// `initialize`'s result: 2025-11-25, whatever the client asked, the one legacy revision whose
/// specification was read for Yuna ([survey](../../../../docs/survey.md#agents-and-mcp)); a
/// client that does not speak it disconnects, as that revision has it.
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": LEGACY_REVISION,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

/// `tools/list`'s result for `tools`; 2026-07-28 adds how long it may be cached.
pub fn tools_list_result(revision: Revision, tools: Value) -> Value {
    let mut body = Map::new();
    body.insert("tools".to_owned(), tools);
    if revision == Revision::Modern {
        body.insert("ttlMs".to_owned(), json!(CACHE_TTL_MS));
        body.insert("cacheScope".to_owned(), json!("public"));
    }
    result(revision, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modern_params(extra: Value) -> Map<String, Value> {
        let mut params = json!({
            "_meta": {
                PROTOCOL_VERSION_KEY: MODERN_REVISION,
                CLIENT_CAPABILITIES_KEY: {},
            }
        });
        if let (Value::Object(params), Value::Object(extra)) = (&mut params, extra) {
            params.extend(extra);
        }
        match params {
            Value::Object(params) => params,
            _ => unreachable!(),
        }
    }

    fn error_code(response: &Value) -> i64 {
        response["error"]["code"].as_i64().expect("an error code")
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error_without_an_id() {
        let response = read_message(b"{\"jsonrpc\": ").expect_err("refused");
        assert_eq!(error_code(&response), PARSE_ERROR);
        assert_eq!(response["id"], Value::Null);
    }

    #[test]
    fn batches_and_values_that_are_not_objects_are_invalid() {
        for line in [
            &b"[]"[..],
            b"[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x\"}]",
            b"7",
        ] {
            let response = read_message(line).expect_err("refused");
            assert_eq!(error_code(&response), INVALID_REQUEST);
        }
    }

    #[test]
    fn a_request_keeps_its_id_and_a_bad_one_is_answered_with_its_id_when_it_can() {
        let message = read_message(br#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#);
        assert_eq!(
            message,
            Ok(Message::Request {
                id: json!("a"),
                method: "tools/list".to_owned(),
                params: Map::new(),
            })
        );
        let response =
            read_message(br#"{"jsonrpc":"1.0","id":7,"method":"tools/list"}"#).expect_err("old");
        assert_eq!(error_code(&response), INVALID_REQUEST);
        assert_eq!(response["id"], json!(7));
        for id in ["null", "1.5", "{}", "true"] {
            let line = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#);
            let response = read_message(line.as_bytes()).expect_err("a bad id");
            assert_eq!(error_code(&response), INVALID_REQUEST, "{id}");
            assert_eq!(response["id"], Value::Null, "{id}");
        }
        let response = read_message(br#"{"jsonrpc":"2.0","id":3,"method":"x","params":[1]}"#)
            .expect_err("positional parameters");
        assert_eq!(error_code(&response), INVALID_REQUEST);
    }

    #[test]
    fn notifications_and_responses_are_told_apart() {
        assert!(matches!(
            read_message(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Ok(Message::Notification { .. })
        ));
        assert_eq!(
            read_message(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            Ok(Message::Response)
        );
    }

    #[test]
    fn a_modern_request_is_served_without_any_earlier_one() {
        let mut session = Session::default();
        assert_eq!(
            session.route("tools/list", &modern_params(json!({}))),
            Ok((Revision::Modern, Call::ListTools))
        );
        assert_eq!(
            session.route("server/discover", &modern_params(json!({}))),
            Ok((Revision::Modern, Call::Discover))
        );
        assert_eq!(
            session.route(
                "tools/call",
                &modern_params(json!({"name": "clock", "arguments": {"action": "now"}}))
            ),
            Ok((
                Revision::Modern,
                Call::Tool {
                    name: "clock".to_owned(),
                    arguments: json!({"action": "now"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                }
            ))
        );
    }

    #[test]
    fn another_revision_in_meta_gets_32022_with_the_one_supported() {
        let mut session = Session::default();
        let params =
            json!({"_meta": {PROTOCOL_VERSION_KEY: "1900-01-01", CLIENT_CAPABILITIES_KEY: {}}});
        let error = session
            .route("tools/list", params.as_object().expect("an object"))
            .expect_err("refused");
        assert_eq!(error.code, UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(
            error.data,
            Some(json!({"supported": [MODERN_REVISION], "requested": "1900-01-01"}))
        );
    }

    #[test]
    fn a_modern_request_without_capabilities_or_any_request_without_meta_is_malformed() {
        let mut session = Session::default();
        let params = json!({"_meta": {PROTOCOL_VERSION_KEY: MODERN_REVISION}});
        let error = session
            .route("tools/list", params.as_object().expect("an object"))
            .expect_err("no capabilities");
        assert_eq!(error.code, INVALID_PARAMS);
        let error = session
            .route("tools/list", &Map::new())
            .expect_err("no revision");
        assert_eq!(error.code, INVALID_PARAMS);
        // `ping` is 2025-11-25's: before `initialize`, a request without `_meta` is malformed.
        assert_eq!(
            session.route("ping", &Map::new()).map_err(|e| e.code),
            Err(INVALID_PARAMS)
        );
    }

    #[test]
    fn after_initialize_requests_without_meta_are_served_as_2025_11_25() {
        let mut session = Session::default();
        let params = json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {}});
        assert_eq!(
            session.route("initialize", params.as_object().expect("an object")),
            Ok((Revision::Legacy, Call::Initialize))
        );
        assert_eq!(
            initialize_result()["protocolVersion"],
            json!(LEGACY_REVISION)
        );
        assert_eq!(
            session.route("ping", &Map::new()),
            Ok((Revision::Legacy, Call::Ping))
        );
        assert_eq!(
            session.route("tools/list", &Map::new()),
            Ok((Revision::Legacy, Call::ListTools))
        );
        assert_eq!(
            session
                .route("server/discover", &Map::new())
                .map_err(|e| e.code),
            Err(METHOD_NOT_FOUND)
        );
        // A modern request is still served statelessly beside it.
        assert_eq!(
            session.route("tools/list", &modern_params(json!({}))),
            Ok((Revision::Modern, Call::ListTools))
        );
    }

    #[test]
    fn unknown_methods_and_malformed_tool_calls_are_refused() {
        let mut session = Session::default();
        assert_eq!(
            session
                .route("ping", &modern_params(json!({})))
                .map_err(|e| e.code),
            Err(METHOD_NOT_FOUND)
        );
        for params in [
            json!({}),
            json!({"name": 3}),
            json!({"name": "clock", "arguments": [1]}),
        ] {
            assert_eq!(
                session
                    .route("tools/call", &modern_params(params))
                    .map_err(|e| e.code),
                Err(INVALID_PARAMS)
            );
        }
        assert_eq!(
            session.route("initialize", &Map::new()).map_err(|e| e.code),
            Err(INVALID_PARAMS)
        );
    }

    #[test]
    fn results_are_shaped_by_their_revision() {
        let modern = tools_list_result(Revision::Modern, json!([]));
        assert_eq!(modern["resultType"], json!("complete"));
        assert_eq!(modern["ttlMs"], json!(CACHE_TTL_MS));
        assert_eq!(modern["cacheScope"], json!("public"));
        assert_eq!(modern["_meta"][SERVER_INFO_KEY]["name"], json!("yuna"));
        let legacy = tools_list_result(Revision::Legacy, json!([]));
        assert_eq!(legacy, json!({"tools": []}));
        let discover = discover_result();
        assert_eq!(discover["supportedVersions"], json!([MODERN_REVISION]));
        assert_eq!(discover["capabilities"], json!({"tools": {}}));
        assert_eq!(discover["resultType"], json!("complete"));
    }

    #[test]
    fn a_cancellation_names_a_valid_id_or_nothing() {
        let params = json!({"requestId": 4, "reason": "timeout"});
        assert_eq!(
            cancelled_request(params.as_object().expect("an object")),
            Some(&json!(4))
        );
        let params = json!({"requestId": null});
        assert_eq!(
            cancelled_request(params.as_object().expect("an object")),
            None
        );
    }

    #[test]
    fn an_error_response_carries_its_data() {
        let response = error_response(json!(1), &unsupported_revision("x"));
        assert_eq!(response["error"]["data"]["requested"], json!("x"));
        assert_eq!(response["jsonrpc"], json!("2.0"));
    }
}
