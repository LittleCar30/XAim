//! Stateless Responses adaptation. Reuse the Chat transport's deadlines and retries.
use super::super::provider::{FunctionCall, ToolCall};
use super::*;
use serde_json::{json, Value};

impl OpenAiProvider {
    pub(super) fn is_responses(&self) -> bool {
        self.protocol == crate::types::ApiProtocol::Responses
    }

    pub(super) fn endpoint(&self) -> String {
        format!(
            "{}/{}",
            self.base_url,
            if self.is_responses() {
                "responses"
            } else {
                "chat/completions"
            }
        )
    }

    fn responses_payload(&self, req: &ToolRequest, stream: bool) -> Value {
        let mut input = vec![];
        for message in &req.messages {
            if message.role == "assistant" && !message.responses_output.is_empty() {
                input.extend(message.responses_output.iter().cloned());
                continue;
            }
            if message.role == "tool" {
                input.push(
                    json!({"type":"function_call_output","call_id":message.tool_call_id,
                    "output":message.content.as_deref().unwrap_or_default()}),
                );
                continue;
            }
            if let Some(content) = &message.content {
                input.push(json!({"role":message.role,"content":content}));
            }
            for call in &message.tool_calls {
                input.push(json!({"type":"function_call","call_id":call.id,
                    "name":call.function.name,"arguments":call.function.arguments}));
            }
        }
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut function = t["function"].clone();
                function["type"] = json!("function");
                // Existing tool schemas use optional fields. Responses defaults to strict;
                // explicitly keep the same schema semantics as Chat Completions.
                function["strict"] = json!(false);
                function
            })
            .collect();
        let mut payload = json!({"model":req.model,"input":input,"tools":tools,
            "max_output_tokens":self.output_limit.map_or(req.max_tokens, |n| n.min(req.max_tokens)),
            "stream":stream,"store":false});
        let deepseek = self.provider_name == "deepseek";
        if !deepseek {
            payload["include"] = json!(["reasoning.encrypted_content"]);
        }
        let effort = if deepseek && self.thinking == Some(false) {
            Some("none")
        } else {
            self.reasoning_effort.as_deref()
        };
        if let Some(effort) = effort {
            payload["reasoning"] = json!({"effort":effort});
        }
        let reasoning =
            self.reasoning_model || effort.is_some() || deepseek && self.thinking != Some(false);
        if !reasoning || deepseek && self.thinking == Some(false) {
            if let Some(temperature) = self.temperature {
                payload["temperature"] = json!(temperature);
            }
        }
        payload
    }

    pub(super) fn responses_metadata(&self, req: &ToolRequest, stream: bool) -> Value {
        let p = self.responses_payload(req, stream);
        json!({"provider":self.name(),"protocol":"responses","model":req.model,
            "endpoint":self.endpoint(),"auth":"bearer","stream":stream,"store":false,
            "maxOutputTokens":p["max_output_tokens"],"reasoningEffort":p["reasoning"]["effort"],
            "temperature":p["temperature"],"parameterSource":"wire_payload"})
    }

    pub(super) async fn responses_request(
        &self,
        req: ToolRequest,
        stream: bool,
        deadline: Option<Instant>,
        observer: &RequestObserver<'_>,
        text: &TextObserver<'_>,
    ) -> Result<ToolResponse> {
        let mut total = RequestObservation::new(Some(observer), "request", 0);
        let outcome = async {
            let payload = self.responses_payload(&req, stream);
            observer(json!({"stage":"requestParameters","parameters":self.responses_metadata(&req, stream)}));
            let response = self.send_with_retries(&payload,
                if stream { None } else { Some(TOOL_REQUEST_TIMEOUT) }, deadline, Some(observer)).await?;
            let json_response = !stream || response.headers().get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()).is_some_and(|s| s.starts_with("application/json"));
            let size = Arc::new(AtomicUsize::new(0));
            let limit = receive_limit(req.max_tokens);
            let bytes = counted_bytes(response, size.clone(), limit);
            let started = Instant::now();
            let result = async {
                if json_response {
                    futures::pin_mut!(bytes);
                    let mut body = vec![];
                    while let Some(chunk) = bytes.next().await { body.extend_from_slice(&chunk?); }
                    let value = serde_json::from_slice::<Value>(&body).map_err(|_| invalid("JSON 无效"))?;
                    observer(json!({"stage":"responseTerminal","event":"json","status":safe_status(&value["status"]),
                        "errorCode":failure_code(&value["error"])}));
                    let response = parse_response(&value)?;
                    if let Some(content) = &response.message.content { text(content)?; }
                    return Ok(response);
                }
                let events = bytes.eventsource();
                futures::pin_mut!(events);
                let mut first = true;
                let mut first_text = true;
                while let Some(event) = events.next().await {
                    let event = event.map_err(stream_error)?;
                    if event.data.trim().is_empty() { continue; }
                    if first { first = false; observer(json!({"stage":"firstChunk","durationMs":total.elapsed_ms()})); }
                    let value = event_value(&event.data)?;
                    if let Some(metadata) = terminal_metadata(&value) { observer(metadata); }
                    if value["type"] == "response.output_text.delta" {
                        let delta = value["delta"].as_str().ok_or_else(|| invalid("正文增量无效"))?;
                        if first_text { first_text = false; observer(json!({"stage":"firstText","durationMs":total.elapsed_ms()})); }
                        text(delta)?;
                    }
                    if let Some(response) = terminal(&value)? { return Ok(response); }
                }
                Err(interrupted())
            }.await;
            let transport = match &result { Err(XaimError::Http(e)) => Some(crate::errors::transport_error_metadata(e)), _ => None };
            observer(json!({"stage":if result.is_ok() {"streamEnd"} else {"streamError"},
                "durationMs":started.elapsed().as_millis() as u64,"bytes":size.load(Ordering::Relaxed),
                "receiveLimitBytes":limit,"success":result.is_ok(),"transport":transport}));
            result
        }.await;
        total.finish(
            json!({"stage":"total","success":outcome.is_ok(),"durationMs":total.elapsed_ms()}),
        );
        outcome
    }

    pub(super) async fn responses_complete(
        &self,
        req: CompletionRequest,
    ) -> Result<CompletionResponse> {
        let result = self
            .responses_request(completion_request(req), false, None, &|_| {}, &|_| Ok(()))
            .await?;
        if result.finish_reason == "length" {
            return Err(invalid("达到输出上限，请提高 Max Tokens"));
        }
        let content = result
            .message
            .content
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("缺少正文"))?;
        Ok(CompletionResponse {
            content,
            usage: result.usage,
        })
    }

    pub(super) async fn responses_stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let req = completion_request(req);
        let response = self
            .send_with_retries(&self.responses_payload(&req, true), None, None, None)
            .await?;
        let bytes = counted_bytes(
            response,
            Arc::new(AtomicUsize::new(0)),
            receive_limit(req.max_tokens),
        );
        let events = Box::pin(bytes.eventsource());
        let stream =
            futures::stream::try_unfold((events, false), |(mut events, done)| async move {
                if done {
                    return Ok(None);
                }
                while let Some(event) = events.next().await {
                    let event = event.map_err(stream_error)?;
                    if event.data.trim().is_empty() {
                        continue;
                    }
                    let value = event_value(&event.data)?;
                    if value["type"] == "response.output_text.delta" {
                        let delta = value["delta"]
                            .as_str()
                            .ok_or_else(|| invalid("正文增量无效"))?
                            .to_string();
                        return Ok(Some((delta, (events, false))));
                    }
                    if let Some(response) = terminal(&value)? {
                        if response.finish_reason == "length" {
                            return Err(invalid("达到输出上限"));
                        }
                        return Ok(None);
                    }
                }
                Err(interrupted())
            });
        Ok(Box::pin(stream))
    }
}

fn completion_request(req: CompletionRequest) -> ToolRequest {
    ToolRequest {
        model: req.model,
        messages: req
            .messages
            .into_iter()
            .map(|m| ToolMessage::text(&m.role, m.content))
            .collect(),
        tools: vec![],
        max_tokens: req.max_tokens,
    }
}
fn receive_limit(tokens: u32) -> usize {
    (tokens as usize)
        .saturating_mul(512)
        .clamp(65_536, 16 * 1024 * 1024)
}
fn counted_bytes(
    response: reqwest::Response,
    size: Arc<AtomicUsize>,
    limit: usize,
) -> impl Stream<Item = Result<bytes::Bytes>> + Send {
    response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(|e| XaimError::Http(e.without_url()))?;
        if size
            .fetch_add(chunk.len(), Ordering::Relaxed)
            .saturating_add(chunk.len())
            > limit
        {
            return Err(invalid("超过接收上限，未执行本轮工具"));
        }
        Ok(chunk)
    })
}
fn invalid(detail: &str) -> XaimError {
    XaimError::Model(format!("Responses 响应无效：{detail}"))
}
fn interrupted() -> XaimError {
    XaimError::Model("Responses SSE 连接在结束事件前关闭，未执行本轮工具".into())
}
fn stream_error(error: EventStreamError<XaimError>) -> XaimError {
    match error {
        EventStreamError::Transport(error) => error,
        EventStreamError::Utf8(_) => invalid("UTF-8 无效"),
        EventStreamError::Parser(_) => invalid("SSE 格式无效"),
    }
}
fn event_value(data: &str) -> Result<Value> {
    serde_json::from_str(data).map_err(|_| invalid("SSE JSON 无效"))
}
fn terminal(value: &Value) -> Result<Option<ToolResponse>> {
    match value["type"].as_str() {
        Some("response.completed" | "response.incomplete" | "response.failed") => {
            let expected = value["type"]
                .as_str()
                .unwrap()
                .trim_start_matches("response.");
            let response = &value["response"];
            if expected == "failed" || response["status"] == "failed" {
                return Err(provider_failure(&response["error"], &response["usage"]));
            }
            if response["status"] != expected {
                // Some Chat-to-Responses gateways label every final event completed,
                // including output-limit responses. Honor explicit truncation without
                // ever making its partial function calls executable.
                if expected == "completed"
                    && response["status"] == "incomplete"
                    && response["incomplete_details"]["reason"] == "max_output_tokens"
                {
                    return parse_response(response).map(Some);
                }
                return Err(invalid(&format!(
                    "结束事件与状态不一致（事件={}，状态={}）；未执行本轮工具。请检查网关 Responses 转换，或切回 Chat Completions",
                    expected, safe_status(&response["status"]),
                )));
            }
            parse_response(response).map(Some)
        }
        Some("error") => Err(provider_failure(
            value.get("error").unwrap_or(value),
            &Value::Null,
        )),
        _ => Ok(None),
    }
}
fn safe_status(value: &Value) -> &'static str {
    match value.as_str() {
        Some("completed") => "completed",
        Some("incomplete") => "incomplete",
        Some("failed") => "failed",
        Some("in_progress") => "in_progress",
        Some("queued") => "queued",
        Some("cancelled") => "cancelled",
        None if value.is_null() => "missing",
        _ => "invalid",
    }
}
fn terminal_metadata(value: &Value) -> Option<Value> {
    let event = match value["type"].as_str()? {
        "response.completed" => "completed",
        "response.incomplete" => "incomplete",
        "response.failed" => "failed",
        "error" => "error",
        _ => return None,
    };
    let response = &value["response"];
    let status = safe_status(&response["status"]);
    let reason = match response["incomplete_details"]["reason"].as_str() {
        Some("max_output_tokens") => "max_output_tokens",
        Some("content_filter") => "content_filter",
        None => "missing",
        _ => "other",
    };
    let error = if event == "error" {
        value.get("error").unwrap_or(value)
    } else {
        &response["error"]
    };
    Some(
        json!({"stage":"responseTerminal","event":event,"status":status,
        "statusMatches":event == status,"incompleteReason":reason,
        "errorCode":failure_code(error),
        "outputItems":response["output"].as_array().map(Vec::len)}),
    )
}

fn failure_code(error: &Value) -> &'static str {
    fn known(value: &Value) -> Option<&'static str> {
        Some(match value.as_str()? {
            "server_error" | "api_error" | "internal_error" | "internal_server_error" => {
                "server_error"
            }
            "server_is_overloaded" | "overloaded_error" => "overloaded_error",
            "rate_limit_exceeded" | "rate_limit_error" | "slow_down" => "rate_limit_exceeded",
            "timeout" | "timeout_error" | "request_timeout" => "timeout",
            "service_unavailable" => "service_unavailable",
            "context_length_exceeded" => "context_length_exceeded",
            "insufficient_quota"
            | "credit_balance_exhausted"
            | "organization_spend_limit_exceeded"
            | "project_spend_limit_exceeded" => "insufficient_quota",
            "invalid_request_error" | "invalid_prompt" | "invalid_parameter" => "invalid_request",
            "authentication_error" | "invalid_api_key" => "authentication_error",
            "permission_error" | "permission_denied" => "permission_error",
            "content_filter"
            | "content_policy_violation"
            | "policy_violation"
            | "cyber_policy"
            | "bio_policy"
            | "misalignment_policy_violation" => "policy_violation",
            _ => return None,
        })
    }
    known(&error["code"])
        .or_else(|| known(&error["type"]))
        .unwrap_or(if error["code"].is_null() && error["type"].is_null() {
            "missing"
        } else {
            "other"
        })
}

fn provider_failure(error: &Value, usage: &Value) -> XaimError {
    let code = failure_code(error);
    let (message, retryable) = match code {
        "server_error" => ("上游内部错误", true),
        "overloaded_error" => ("上游暂时过载", true),
        "rate_limit_exceeded" => ("上游请求限流", true),
        "timeout" => ("上游生成超时", true),
        "service_unavailable" => ("上游暂时不可用", true),
        "context_length_exceeded" => ("输入超过上游上下文上限，请缩小历史或文件读取范围", false),
        "insufficient_quota" => ("上游余额或额度不足，请检查渠道账单", false),
        "invalid_request" => ("上游拒绝请求参数，请检查渠道模型与 Responses 兼容性", false),
        "authentication_error" => ("上游认证失败，请检查渠道凭据", false),
        "permission_error" => ("上游拒绝访问该模型或资源", false),
        "policy_violation" => ("上游策略拒绝请求", false),
        _ => ("上游未提供可识别错误码，请使用请求 ID 查询网关日志", true),
    };
    // Only allowlisted classifications are exposed; gateway messages can echo
    // credentials, prompts, function arguments or private reasoning.
    let retry_after_ms = error["headers"].as_object().and_then(|headers| {
        headers.iter().find_map(|(name, value)| {
            if !name.eq_ignore_ascii_case("retry-after") {
                return None;
            }
            let delay = value.as_str().and_then(retry_after_delay)?;
            u64::try_from(delay.as_millis()).ok()
        })
    });
    XaimError::ProviderFailure {
        code,
        message,
        retryable,
        retry_after_ms,
        input_tokens: usage["input_tokens"]
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64),
        output_tokens: usage["output_tokens"]
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64),
    }
}
fn parse_response(value: &Value) -> Result<ToolResponse> {
    let truncated = match value["status"].as_str() {
        Some("completed") => false,
        Some("incomplete") if value["incomplete_details"]["reason"] == "max_output_tokens" => true,
        Some("incomplete") => return Err(invalid("服务端未完成响应，未执行本轮工具")),
        Some("failed") => return Err(provider_failure(&value["error"], &value["usage"])),
        _ => return Err(invalid("缺少完整结束状态")),
    };
    let items = value["output"]
        .as_array()
        .ok_or_else(|| invalid("缺少 output"))?;
    let mut message = ToolMessage::text("assistant", "");
    let mut content = String::new();
    let mut ids = std::collections::HashSet::new();
    for item in items {
        if !truncated
            && item
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "completed")
        {
            return Err(invalid("输出项未完成"));
        }
        match item["type"].as_str() {
            Some("message") => {
                if item["role"] != "assistant" {
                    return Err(invalid("输出角色无效"));
                }
                for part in item["content"]
                    .as_array()
                    .ok_or_else(|| invalid("消息内容无效"))?
                {
                    match part["type"].as_str() {
                        Some("output_text") => content
                            .push_str(part["text"].as_str().ok_or_else(|| invalid("正文无效"))?),
                        Some("refusal") => return Err(invalid("模型拒绝请求，未执行本轮工具")),
                        _ => return Err(invalid("不支持的消息内容")),
                    }
                }
            }
            Some("function_call") if !truncated => {
                if item
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s != "completed")
                {
                    return Err(invalid("工具调用未完成"));
                }
                let id = item["call_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| invalid("缺少 call_id"))?;
                let name = item["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| invalid("缺少函数名"))?;
                let arguments = item["arguments"]
                    .as_str()
                    .ok_or_else(|| invalid("缺少工具参数"))?;
                if !ids.insert(id)
                    || !serde_json::from_str::<Value>(arguments).is_ok_and(|v| v.is_object())
                {
                    return Err(invalid("工具参数或调用标识无效"));
                }
                message.tool_calls.push(ToolCall {
                    id: id.into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: name.into(),
                        arguments: arguments.into(),
                    },
                });
            }
            Some("function_call" | "reasoning") => {}
            _ => return Err(invalid("不支持的输出类型，未执行本轮工具")),
        }
    }
    message.content = (!content.is_empty()).then_some(content);
    if !truncated {
        message.responses_output = items.clone();
    }
    let usage = &value["usage"];
    let input_tokens = usage["input_tokens"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok());
    let output_tokens = usage["output_tokens"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok());
    Ok(ToolResponse {
        finish_reason: if truncated {
            "length"
        } else if message.tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        }
        .into(),
        message,
        usage: Usage::new(input_tokens.unwrap_or(0), output_tokens.unwrap_or(0)),
        usage_reported: input_tokens.is_some() && output_tokens.is_some(),
        reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    fn request() -> ToolRequest {
        ToolRequest {
            model: "custom-model".into(),
            messages: vec![
                ToolMessage::text("system", "instructions"),
                ToolMessage::text("user", "private prompt"),
            ],
            tools: vec![
                json!({"type":"function","function":{"name":"read_file","description":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}),
            ],
            max_tokens: 8192,
        }
    }
    fn response() -> Value {
        json!({"status":"completed","output":[
        {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"private encrypted state"},
        {"type":"function_call","id":"fc_1","status":"completed","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"test.txt\"}"}],
        "usage":{"input_tokens":12,"output_tokens":30,"output_tokens_details":{"reasoning_tokens":20}}})
    }
    fn provider(url: String) -> OpenAiProvider {
        OpenAiProvider::new(url, "test-secret".into())
            .with_protocol(crate::types::ApiProtocol::Responses)
    }
    fn event(v: &Value) -> String {
        format!("event: {}\ndata: {}\n\n", v["type"].as_str().unwrap(), v)
    }
    fn final_event(r: Value) -> String {
        event(&json!({"type":format!("response.{}",r["status"].as_str().unwrap()),"response":r}))
    }

    #[tokio::test]
    async fn stream_tool_round_trip_preserves_private_state_and_function_pairing() {
        let mut server = Server::new_async().await;
        let r = response();
        let first = server.mock("POST","/responses").match_header("authorization","Bearer test-secret")
            .match_body(Matcher::PartialJson(json!({"store":false,"stream":true,"max_output_tokens":4096,
                "include":["reasoning.encrypted_content"],"reasoning":{"effort":"low"},
                "tools":[{"type":"function","name":"read_file","strict":false,"description":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]})))
            .with_header("content-type","text/event-stream").with_body(
                event(&json!({"type":"response.reasoning_text.delta","delta":"secret thought"})) +
                &event(&json!({"type":"response.function_call_arguments.delta","delta":"partial arguments"})) + &final_event(r.clone()))
            .create_async().await;
        let p = provider(server.url())
            .with_output_limit(Some(4096))
            .with_reasoning_effort(Some("low".into()));
        let public = std::sync::Mutex::new(String::new());
        let diagnostics = std::sync::Mutex::new(vec![]);
        let result = p
            .interact_tools_observed(
                request(),
                None,
                &|v| diagnostics.lock().unwrap().push(v),
                &|s| {
                    public.lock().unwrap().push_str(s);
                    Ok(())
                },
            )
            .await
            .unwrap();
        first.assert_async().await;
        assert_eq!(result.finish_reason, "tool_calls");
        assert_eq!(result.message.tool_calls[0].id, "call_1");
        assert_eq!(result.reasoning_tokens, Some(20));
        assert_eq!(result.usage, Usage::new(12, 30));
        assert!(public.lock().unwrap().is_empty());
        let safe = serde_json::to_string(&*diagnostics.lock().unwrap()).unwrap();
        for private in [
            "test-secret",
            "private prompt",
            "secret thought",
            "private encrypted state",
            "partial arguments",
        ] {
            assert!(!safe.contains(private));
        }
        assert!(!serde_json::to_string(&result.message)
            .unwrap()
            .contains("private encrypted state"));
        let mut req = request();
        req.messages.push(result.message);
        req.messages
            .push(ToolMessage::result("call_1", "contents".into()));
        let payload = p.responses_payload(&req, true);
        assert_eq!(payload["input"][2], r["output"][0]);
        assert_eq!(payload["input"][3], r["output"][1]);
        assert_eq!(
            payload["input"][4],
            json!({"type":"function_call_output","call_id":"call_1","output":"contents"})
        );
        assert!(payload.get("messages").is_none());
        let second = server.mock("POST","/responses").match_body(Matcher::Json(payload))
            .with_header("content-type","text/event-stream").with_body(
                event(&json!({"type":"response.output_text.delta","delta":"done"})) +
                &final_event(json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]})))
            .create_async().await;
        p.interact_tools_observed(req, None, &|_| {}, &|s| {
            public.lock().unwrap().push_str(s);
            Ok(())
        })
        .await
        .unwrap();
        second.assert_async().await;
        assert_eq!(*public.lock().unwrap(), "done");
    }

    #[tokio::test]
    async fn missing_terminal_event_discards_even_done_tool_items() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("POST", "/responses")
            .with_header("content-type", "text/event-stream")
            .with_body(event(
                &json!({"type":"response.output_item.done","item":response()["output"][1]}),
            ))
            .create_async()
            .await;
        let error = provider(server.url())
            .interact_tools_observed(request(), None, &|_| {}, &|_| Ok(()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("结束事件前关闭"));
        m.assert_async().await;
    }

    #[test]
    fn truncated_failed_and_invalid_calls_never_become_executable() {
        let mut r = response();
        r["status"] = json!("incomplete");
        r["incomplete_details"] = json!({"reason":"max_output_tokens"});
        r["output"][1]["arguments"] = json!("{broken");
        let result = parse_response(&r).unwrap();
        assert_eq!(result.finish_reason, "length");
        assert!(result.message.tool_calls.is_empty());
        assert!(result.message.responses_output.is_empty());
        for status in ["failed", "in_progress", "queued"] {
            r["status"] = json!(status);
            assert!(parse_response(&r).is_err());
        }
        r["status"] = json!("completed");
        assert!(parse_response(&r).is_err());
        let mut r = response();
        r["output"][1]["status"] = json!("in_progress");
        assert!(parse_response(&r).is_err());
        let mut r = response();
        let duplicate = r["output"][1].clone();
        r["output"].as_array_mut().unwrap().push(duplicate);
        assert!(parse_response(&r).is_err());
        assert!(
            terminal(&json!({"type":"response.completed","response":{"status":"incomplete"}}))
                .is_err()
        );
    }

    #[tokio::test]
    async fn gateway_completed_event_with_explicit_truncation_discards_tools_and_records_safe_metadata(
    ) {
        let mut server = Server::new_async().await;
        let mut r = response();
        r["status"] = json!("incomplete");
        r["incomplete_details"] = json!({"reason":"max_output_tokens"});
        r["output"][1]["arguments"] = json!("{unfinished private arguments");
        let m = server
            .mock("POST", "/responses")
            .with_header("content-type", "text/event-stream")
            .with_body(event(&json!({"type":"response.completed","response":r})))
            .create_async()
            .await;
        let diagnostics = std::sync::Mutex::new(vec![]);
        let result = provider(server.url())
            .interact_tools_observed(
                request(),
                None,
                &|v| diagnostics.lock().unwrap().push(v),
                &|_| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(result.finish_reason, "length");
        assert!(result.message.tool_calls.is_empty());
        assert!(result.message.responses_output.is_empty());
        let diagnostics = diagnostics.into_inner().unwrap();
        let metadata = diagnostics
            .iter()
            .find(|v| v["stage"] == "responseTerminal")
            .unwrap();
        assert_eq!(metadata["event"], "completed");
        assert_eq!(metadata["status"], "incomplete");
        assert_eq!(metadata["statusMatches"], false);
        assert_eq!(metadata["incompleteReason"], "max_output_tokens");
        let safe = serde_json::to_string(&diagnostics).unwrap();
        assert!(!safe.contains("private arguments"));
        assert!(!safe.contains("private encrypted state"));
        m.assert_async().await;
    }

    #[test]
    fn mismatched_terminal_states_still_reject_unproven_completion_without_exposing_payload() {
        for status in [
            json!("in_progress"),
            Value::Null,
            json!("private upstream error"),
        ] {
            let mut r = response();
            r["status"] = status.clone();
            let value = json!({"type":"response.completed","response":r});
            let error = terminal(&value).unwrap_err().to_string();
            assert!(error.contains("未执行本轮工具"));
            assert!(error.contains(safe_status(&status)));
            let safe = format!("{error}{}", terminal_metadata(&value).unwrap());
            assert!(!safe.contains("private upstream error"));
            assert!(!safe.contains("private encrypted state"));
        }
        let mut r = response();
        r["status"] = json!("incomplete");
        r["incomplete_details"] = json!({"reason":"content_filter"});
        assert!(terminal(&json!({"type":"response.completed","response":r})).is_err());
        assert!(terminal(&json!({"type":"response.failed","response":response()})).is_err());
    }

    #[test]
    fn provider_failures_classify_transient_and_fatal_causes_without_echoing_messages() {
        for (code, retryable) in [
            ("server_error", true),
            ("server_is_overloaded", true),
            ("rate_limit_exceeded", true),
            ("timeout_error", true),
            ("context_length_exceeded", false),
            ("insufficient_quota", false),
            ("invalid_api_key", false),
            ("invalid_request_error", false),
            ("content_policy_violation", false),
            ("misalignment_policy_violation", false),
        ] {
            let error = provider_failure(
                &json!({"code":code,"message":"private prompt Bearer secret-value",
                "headers":{"Retry-After":"11"}}),
                &json!({"input_tokens":10,"output_tokens":15}),
            );
            assert!(matches!(&error, XaimError::ProviderFailure { retryable: r,
                retry_after_ms: Some(11000), input_tokens: Some(10), output_tokens: Some(15), .. } if *r == retryable));
            assert!(!error.to_string().contains("private prompt"));
            assert!(!error.to_string().contains("secret-value"));
        }
        let error = terminal(&json!({"type":"error","code":"rate_limit_exceeded",
            "message":"private upstream body"}))
        .unwrap_err();
        assert!(matches!(
            error,
            XaimError::ProviderFailure {
                code: "rate_limit_exceeded",
                retryable: true,
                ..
            }
        ));
        let error = provider_failure(
            &json!({"type":"invalid_request_error","code":"unknown_custom_code"}),
            &Value::Null,
        );
        assert!(matches!(
            error,
            XaimError::ProviderFailure {
                retryable: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn json_completion_and_ignored_stream_request_use_responses_schema() {
        let mut server = Server::new_async().await;
        let body = json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}]});
        let m = server
            .mock("POST", "/responses")
            .match_body(Matcher::PartialJson(
                json!({"stream":false,"max_output_tokens":8}),
            ))
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;
        let p = provider(server.url());
        let mut req = CompletionRequest::new(
            "custom",
            vec![super::super::super::provider::Message::user("ping")],
        );
        req.max_tokens = 8;
        assert_eq!(p.complete(req).await.unwrap().content, "pong");
        m.assert_async().await;
        let m = server
            .mock("POST", "/responses")
            .match_body(Matcher::PartialJson(json!({"stream":true})))
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;
        assert_eq!(
            p.interact_tools_observed(request(), None, &|_| {}, &|_| Ok(()))
                .await
                .unwrap()
                .message
                .content
                .as_deref(),
            Some("pong")
        );
        m.assert_async().await;
    }

    #[tokio::test]
    async fn plain_stream_uses_semantic_terminal_events_without_done_sentinel() {
        let mut server = Server::new_async().await;
        let body = event(&json!({"type":"response.output_text.delta","delta":"hi"}))
            + &final_event(
                json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}]}),
            );
        let m = server
            .mock("POST", "/responses")
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;
        let p = provider(server.url());
        let mut stream = p
            .stream(CompletionRequest::new("custom", vec![]))
            .await
            .unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap(), "hi");
        assert!(stream.next().await.is_none());
        m.assert_async().await;
    }

    #[test]
    fn deepseek_uses_native_stateless_reasoning_parameters() {
        let p = provider("https://api.deepseek.com".into())
            .with_provider_name("deepseek")
            .with_thinking(Some(false))
            .with_temperature(Some(0.25));
        let payload = p.responses_payload(&request(), true);
        assert_eq!(payload["reasoning"], json!({"effort":"none"}));
        assert_eq!(payload["temperature"], 0.25);
        for absent in [
            "thinking",
            "reasoning_effort",
            "stream_options",
            "previous_response_id",
            "include",
            "max_tokens",
            "max_completion_tokens",
        ] {
            assert!(payload.get(absent).is_none());
        }
        let mut r = response();
        r["output"][0] = json!({"type":"reasoning","id":"rs_1","summary":[],"content":[{"type":"reasoning_text","text":"private thought"}]});
        let msg = parse_response(&r).unwrap().message;
        let mut req = request();
        req.messages.push(msg);
        assert_eq!(p.responses_payload(&req, true)["input"][2], r["output"][0]);
        assert_eq!(
            provider("https://example.test/v1/responses/".into()).endpoint(),
            "https://example.test/v1/responses"
        );
    }
}
