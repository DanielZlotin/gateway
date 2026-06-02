use crate::logs;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const ANTHROPIC_CHAT_COMPLETIONS_URL: &str = "https://api.anthropic.com/v1/chat/completions";
const MAX_TOKENS: u64 = 4096;

pub struct AnthropicProxy {
    base_url: String,
    server: Arc<Server>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AnthropicProxy {
    pub fn start(timeout: Duration) -> Result<Self, String> {
        let server = Arc::new(
            Server::http("127.0.0.1:0").map_err(|err| format!("start Anthropic proxy: {err}"))?,
        );
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "Anthropic proxy must listen on TCP".to_string())?
            .port();
        let running = Arc::new(AtomicBool::new(true));
        let thread_server = Arc::clone(&server);
        let thread_running = Arc::clone(&running);
        let handle = thread::spawn(move || serve(thread_server, thread_running, timeout));

        Ok(Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            server,
            running,
            handle: Some(handle),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for AnthropicProxy {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.server.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(server: Arc<Server>, running: Arc<AtomicBool>, timeout: Duration) {
    while running.load(Ordering::Relaxed) {
        match server.recv() {
            Ok(request) => {
                if let Err(err) = handle_request(request, timeout) {
                    logs::warn(format_args!("Anthropic proxy request failed: {err}"));
                }
            }
            Err(_) => break,
        }
    }
}

fn handle_request(mut request: Request, timeout: Duration) -> Result<(), String> {
    if request.method() != &Method::Post || request.url() != "/v1/responses" {
        return respond(request, 404, "text/plain", "not found");
    }
    let api_key = bearer_token(request.headers())
        .ok_or_else(|| "ANTHROPIC_API_KEY is required for Claude.".to_string())?;
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|err| format!("read Codex request: {err}"))?;
    let responses_request: Value =
        serde_json::from_str(&body).map_err(|err| format!("parse Codex request: {err}"))?;
    let chat_request = chat_request_from_responses(&responses_request);
    let chat_response = call_anthropic(&api_key, &chat_request, timeout)?;
    let events = responses_sse_from_chat_completion(&chat_response)?;
    respond(request, 200, "text/event-stream", events)
}

fn call_anthropic(api_key: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let response = agent
        .post(ANTHROPIC_CHAT_COMPLETIONS_URL)
        .set("Authorization", &format!("Bearer {api_key}"))
        .send_json(body.clone());
    match response {
        Ok(response) => response
            .into_json()
            .map_err(|err| format!("decode Anthropic response: {err}")),
        Err(ureq::Error::Status(status, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!(
                "Anthropic request failed with status {status}: {text}"
            ))
        }
        Err(err) => Err(format!("Anthropic request failed: {err}")),
    }
}

fn chat_request_from_responses(request: &Value) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) = request.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": instructions,
            }));
        }
    }
    if let Some(input) = request.get("input").and_then(Value::as_array) {
        for item in input {
            append_input_message(&mut messages, item);
        }
    }

    let mut body = json!({
        "model": request.get("model").and_then(Value::as_str).unwrap_or("claude-opus-4-8"),
        "max_tokens": MAX_TOKENS,
        "stream": false,
        "messages": messages,
    });
    if let Some(tools) = chat_tools(request) {
        body["tools"] = tools;
        body["tool_choice"] = json!("auto");
    }
    body
}

fn append_input_message(messages: &mut Vec<Value>, item: &Value) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = content_text(item.get("content").unwrap_or(&Value::Null));
            if !content.trim().is_empty() {
                messages.push(json!({
                    "role": role,
                    "content": content,
                }));
            }
        }
        Some("function_call") => {
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("call_unknown");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "tool_calls": [{
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                        },
                    }],
                }));
            }
        }
        Some("function_call_output") => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("call_unknown");
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": item.get("output").and_then(Value::as_str).unwrap_or(""),
            }));
        }
        _ => {}
    }
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn chat_tools(request: &Value) -> Option<Value> {
    let tools = request
        .get("tools")?
        .as_array()?
        .iter()
        .filter_map(chat_tool)
        .collect::<Vec<_>>();
    if tools.is_empty() {
        None
    } else {
        Some(Value::Array(tools))
    }
}

fn chat_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    Some(json!({
        "type": "function",
        "function": {
            "name": tool.get("name")?.clone(),
            "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
            "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object" })),
        },
    }))
}

fn responses_sse_from_chat_completion(response: &Value) -> Result<String, String> {
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| "Anthropic response missing message".to_string())?;
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_anthropic");
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        if !tool_calls.is_empty() {
            return Ok(function_call_events(response_id, tool_calls));
        }
    }
    Ok(text_events(
        response_id,
        message.get("content").and_then(Value::as_str).unwrap_or(""),
    ))
}

fn text_events(response_id: &str, text: &str) -> String {
    let item_id = "msg_anthropic_0";
    let done_item = json!({
        "id": item_id,
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": text,
            "annotations": [],
        }],
    });
    [
        sse(
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "response_id": response_id,
                "output_index": 0,
                "item": {
                    "id": item_id,
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                },
            }),
        ),
        sse(
            "response.output_text.delta",
            json!({
                "type": "response.output_text.delta",
                "response_id": response_id,
                "item_id": item_id,
                "output_index": 0,
                "content_index": 0,
                "delta": text,
            }),
        ),
        sse(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": response_id,
                "output_index": 0,
                "item": done_item.clone(),
            }),
        ),
        sse(
            "response.completed",
            json!({
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "object": "response",
                    "status": "completed",
                    "output": [done_item],
                },
            }),
        ),
    ]
    .join("")
}

fn function_call_events(response_id: &str, tool_calls: &[Value]) -> String {
    let mut events = String::new();
    let mut output = Vec::new();
    for (index, tool_call) in tool_calls.iter().enumerate() {
        let function = tool_call.get("function").unwrap_or(&Value::Null);
        let item_id = tool_call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("fc_anthropic");
        let call_id = tool_call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("call_anthropic");
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let empty_item = json!({
            "id": item_id,
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": "",
        });
        let done_item = json!({
            "id": item_id,
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": arguments,
        });
        events.push_str(&sse(
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "response_id": response_id,
                "output_index": index,
                "item": empty_item,
            }),
        ));
        events.push_str(&sse(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": response_id,
                "output_index": index,
                "item": done_item.clone(),
            }),
        ));
        output.push(done_item);
    }
    events.push_str(&sse(
        "response.completed",
        json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "object": "response",
                "status": "completed",
                "output": output,
            },
        }),
    ));
    events
}

fn sse(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn bearer_token(headers: &[Header]) -> Option<String> {
    headers
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .and_then(|header| bearer_token_value(header.value.as_str()))
}

fn bearer_token_value(value: &str) -> Option<String> {
    let (scheme, token) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn respond(
    request: Request,
    code: u16,
    content_type: &str,
    body: impl Into<String>,
) -> Result<(), String> {
    let header = Header::from_bytes("content-type", content_type)
        .map_err(|_| format!("build Anthropic proxy content-type header: {content_type}"))?;
    request
        .respond(
            Response::from_string(body)
                .with_status_code(StatusCode(code))
                .with_header(header),
        )
        .map_err(|err| format!("write Anthropic proxy response: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_maps_responses_messages_and_function_tools() {
        let request = json!({
            "model": "claude-opus-4-8",
            "instructions": "system text",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "hello" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "/work"
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "name": "exec_command",
                    "description": "run command",
                    "parameters": { "type": "object" }
                },
                { "type": "web_search" }
            ]
        });

        let body = chat_request_from_responses(&request);

        assert_eq!(body["model"], "claude-opus-4-8");
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hello");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["function"]["name"], "exec_command");
    }

    #[test]
    fn responses_sse_maps_text_completion() {
        let response = json!({
            "id": "chatcmpl_1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "hello"
                }
            }]
        });

        let events = responses_sse_from_chat_completion(&response).unwrap();

        assert!(events.contains("event: response.output_text.delta"));
        assert!(events.contains("\"delta\":\"hello\""));
        assert!(events.contains("event: response.completed"));
    }

    #[test]
    fn responses_sse_maps_function_call_completion() {
        let response = json!({
            "id": "chatcmpl_1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "exec_command",
                            "arguments": "{\"cmd\":\"pwd\"}"
                        }
                    }]
                }
            }]
        });

        let events = responses_sse_from_chat_completion(&response).unwrap();

        assert!(events.contains("event: response.output_item.done"));
        assert!(events.contains("\"name\":\"exec_command\""));
        assert!(events.contains("\"arguments\":\"{\\\"cmd\\\":\\\"pwd\\\"}\""));
    }

    #[test]
    fn bearer_token_accepts_case_insensitive_scheme() {
        assert_eq!(
            bearer_token_value("bearer anthropic-key").as_deref(),
            Some("anthropic-key")
        );
    }
}
