//! Assemble one complete tool response before exposing executable calls.
use std::collections::BTreeMap;

use serde::Deserialize;

use super::provider::{FunctionCall, ToolCall, ToolMessage, ToolResponse, Usage};
use crate::errors::{Result, XaimError};

#[derive(Default)]
pub(super) struct ToolStream {
    content: String,
    reasoning: String,
    calls: BTreeMap<usize, ToolCall>,
    finish: Option<String>,
    usage: Option<StreamUsage>,
    done: bool,
}

impl ToolStream {
    pub fn push(&mut self, data: &str) -> Result<Option<String>> {
        if self.done {
            return Err(invalid("结束标记之后仍有数据"));
        }
        if data == "[DONE]" {
            self.done = true;
            return Ok(None);
        }
        // Never attach response data to errors: it can contain reasoning or args.
        let packet: Packet = serde_json::from_str(data).map_err(|_| invalid("JSON 无效"))?;
        if let Some(usage) = packet.usage {
            self.usage = Some(usage);
        }
        let mut text = String::new();
        for choice in packet.choices {
            if choice.index != 0 {
                return Err(invalid("不支持多个响应选择"));
            }
            if self.finish.is_some() {
                return Err(invalid("结束原因之后仍有响应选择"));
            }
            if choice
                .delta
                .role
                .as_deref()
                .is_some_and(|r| r != "assistant")
            {
                return Err(invalid("响应角色不是 assistant"));
            }
            if let Some(content) = choice.delta.content {
                self.content.push_str(&content);
                text.push_str(&content);
            }
            if let Some(reasoning) = choice.delta.reasoning_content {
                self.reasoning.push_str(&reasoning);
            }
            for delta in choice.delta.tool_calls.unwrap_or_default() {
                if delta.index >= 128 {
                    return Err(invalid("工具索引超过上限"));
                }
                let call = self.calls.entry(delta.index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: String::new(),
                        arguments: String::new(),
                    },
                });
                if let Some(id) = delta.id {
                    call.id.push_str(&id);
                }
                if let Some(kind) = delta.kind {
                    if kind != "function" {
                        return Err(invalid("工具类型无效"));
                    }
                }
                if let Some(function) = delta.function {
                    if let Some(name) = function.name {
                        call.function.name.push_str(&name);
                    }
                    if let Some(args) = function.arguments {
                        call.function.arguments.push_str(&args);
                    }
                }
            }
            self.finish = choice.finish_reason;
        }
        Ok((!text.is_empty()).then_some(text))
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn finish(self, keep_reasoning: bool) -> Result<ToolResponse> {
        if !self.done {
            return Err(invalid("连接在结束标记前关闭，未执行本轮工具"));
        }
        let reason = self.finish.ok_or_else(|| invalid("缺少结束原因"))?;
        if !matches!(reason.as_str(), "stop" | "tool_calls" | "length") {
            return Err(invalid("结束原因不允许执行工具"));
        }
        if reason == "stop" && !self.calls.is_empty()
            || reason == "tool_calls" && self.calls.is_empty()
        {
            return Err(invalid("结束原因与工具调用不一致"));
        }
        let mut calls = Vec::with_capacity(self.calls.len());
        for (expected, (index, call)) in self.calls.into_iter().enumerate() {
            // Length is handed to the existing truncation recovery, never executed.
            if reason != "length"
                && (index != expected
                    || call.function.name.is_empty()
                    || serde_json::from_str::<serde_json::Value>(&call.function.arguments).is_err())
            {
                return Err(invalid("工具调用不完整，未执行本轮工具"));
            }
            calls.push(call);
        }
        let reasoning_tokens = self
            .usage
            .as_ref()
            .and_then(|u| u.completion_tokens_details.as_ref())
            .and_then(|u| u.reasoning_tokens);
        Ok(ToolResponse {
            message: ToolMessage {
                responses_output: vec![],
                role: "assistant".into(),
                content: (!self.content.is_empty()).then_some(self.content),
                tool_calls: calls,
                tool_call_id: None,
                reasoning_content: (keep_reasoning && !self.reasoning.is_empty())
                    .then_some(self.reasoning),
            },
            finish_reason: reason,
            usage_reported: self.usage.is_some(),
            reasoning_tokens,
            usage: self
                .usage
                .map(|u| Usage::new(u.prompt_tokens, u.completion_tokens))
                .unwrap_or_default(),
        })
    }
}

fn invalid(detail: &str) -> XaimError {
    XaimError::Model(format!("工具 SSE 响应无效：{detail}"))
}

#[derive(Deserialize)]
struct Packet {
    choices: Vec<Choice>,
    usage: Option<StreamUsage>,
}
#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    index: usize,
    delta: Delta,
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct Delta {
    role: Option<String>,
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<CallDelta>>,
}
#[derive(Deserialize)]
struct CallDelta {
    index: usize,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<FunctionDelta>,
}
#[derive(Deserialize)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}
#[derive(Deserialize)]
struct StreamUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    completion_tokens_details: Option<ReasoningUsage>,
}
#[derive(Deserialize)]
struct ReasoningUsage {
    reasoning_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(delta: serde_json::Value, finish: Option<&str>) -> String {
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}).to_string()
    }

    #[test]
    fn interleaved_tools_usage_and_private_reasoning_are_assembled() {
        let mut stream = ToolStream::default();
        assert!(stream
            .push(&chunk(
                json!({"role":"assistant","reasoning_content":"private"}),
                None
            ))
            .unwrap()
            .is_none());
        assert_eq!(
            stream
                .push(&chunk(
                    json!({"content":"你好","tool_calls":[
            {"index":1,"id":"b","function":{"name":"read_file","arguments":"{\"path\":"}},
            {"index":0,"id":"a","function":{"name":"list_dir","arguments":"{"}}]}),
                    None
                ))
                .unwrap()
                .as_deref(),
            Some("你好")
        );
        stream
            .push(&chunk(
                json!({"tool_calls":[
            {"index":0,"function":{"arguments":"}"}},
            {"index":1,"function":{"arguments":"\"中文.rs\"}"}}]}),
                Some("tool_calls"),
            ))
            .unwrap();
        stream
            .push(
                &json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":8,
            "completion_tokens_details":{"reasoning_tokens":3}}})
                .to_string(),
            )
            .unwrap();
        stream.push("[DONE]").unwrap();
        let response = stream.finish(false).unwrap();
        assert_eq!(response.message.tool_calls[0].id, "a");
        assert_eq!(
            response.message.tool_calls[1].function.arguments,
            "{\"path\":\"中文.rs\"}"
        );
        assert!(response.message.reasoning_content.is_none());
        assert_eq!(response.usage, Usage::new(12, 8));
        assert_eq!(response.reasoning_tokens, Some(3));
    }

    #[test]
    fn eof_missing_finish_and_incomplete_arguments_never_produce_executable_calls() {
        let mut stream = ToolStream::default();
        stream.push(&chunk(json!({"tool_calls":[{"index":0,"id":"x","function":{"name":"create_file","arguments":"{"}}]}), Some("tool_calls"))).unwrap();
        assert!(stream.finish(false).is_err());
        let mut stream = ToolStream::default();
        stream
            .push(&chunk(json!({"content":"partial"}), None))
            .unwrap();
        stream.push("[DONE]").unwrap();
        assert!(stream.finish(false).is_err());
        let mut stream = ToolStream::default();
        stream.push(&chunk(json!({"tool_calls":[{"index":0,"id":"x","function":{"name":"create_file","arguments":"{"}}]}), Some("tool_calls"))).unwrap();
        stream.push("[DONE]").unwrap();
        assert!(stream.finish(false).is_err());
    }

    #[test]
    fn length_is_returned_for_existing_budget_recovery_and_missing_usage_stays_unknown() {
        let mut stream = ToolStream::default();
        stream
            .push(&chunk(json!({"content":"partial"}), Some("length")))
            .unwrap();
        stream.push("[DONE]").unwrap();
        let response = stream.finish(false).unwrap();
        assert_eq!(response.finish_reason, "length");
        assert!(!response.usage_reported);
    }

    #[test]
    fn malformed_packet_error_does_not_echo_private_data() {
        let error = ToolStream::default()
            .push("{private secret")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
    }
}
