use super::*;
use serde_json::json;
use std::sync::Mutex;

fn request() -> ToolRequest {
    ToolRequest {
        model: "mock".into(),
        messages: vec![ToolMessage::text("user", "ping")],
        tools: vec![],
        max_tokens: 100,
    }
}

fn packet(delta: serde_json::Value, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}

fn final_text() -> String {
    packet(json!({"content":"done"}), Some("stop")) + "data: [DONE]\n\n"
}

fn json_response() -> String {
    json!({"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":9,"completion_tokens":2}})
    .to_string()
}

#[tokio::test]
async fn fragmented_sse_preserves_unicode_tools_usage_and_only_exposes_public_text() {
    let mut server = mockito::Server::new_async().await;
    let body = packet(
        json!({"role":"assistant","reasoning_content":"private reasoning"}),
        None,
    ) + &packet(
        json!({"content":"读取中文","tool_calls":[{"index":0,"id":"a","type":"function",
            "function":{"name":"read_file","arguments":"{\"path\":"}}]}),
        None,
    ) + &packet(
        json!({"tool_calls":[{"index":0,"function":{"arguments":"\"中文.rs\"}"}}]}),
        Some("tool_calls"),
    ) + &format!(
        "data: {}\n\n",
        json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":10}})
    ) + "data: [DONE]\n\n";
    let mock = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(
            json!({"stream":true,"stream_options":{"include_usage":true}}),
        ))
        .with_header("content-type", "text/event-stream")
        .with_chunked_body(move |writer| {
            for byte in body.as_bytes() {
                writer.write_all(&[*byte])?;
            }
            Ok(())
        })
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let visible = Mutex::new(String::new());
    let events = Mutex::new(Vec::new());
    let response = provider
        .interact_tools_observed(
            request(),
            None,
            &|e| events.lock().unwrap().push(e),
            &|text| {
                visible.lock().unwrap().push_str(text);
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(*visible.lock().unwrap(), "读取中文");
    assert_eq!(
        response.message.tool_calls[0].function.arguments,
        "{\"path\":\"中文.rs\"}"
    );
    assert!(response.message.reasoning_content.is_none());
    assert_eq!(response.usage, Usage::new(20, 10));
    let events = events.lock().unwrap();
    assert!(events.iter().any(|e| e["stage"] == "firstText"));
    assert!(events.iter().any(|e| e["stage"] == "streamEnd"));
    let diagnostics = serde_json::to_string(&*events).unwrap();
    assert!(
        !diagnostics.contains("private reasoning")
            && !diagnostics.contains("中文.rs")
            && !diagnostics.contains("test-key")
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn explicit_stream_rejection_falls_back_once_and_remembers_compatibility() {
    let mut server = mockito::Server::new_async().await;
    let rejected = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(json!({"stream":true})))
        .with_status(400)
        .with_body("stream is not supported")
        .expect(1)
        .create_async()
        .await;
    let fallback = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(json!({"stream":false})))
        .with_body(json_response())
        .expect(2)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let events = Mutex::new(Vec::new());
    for _ in 0..2 {
        let response = provider
            .interact_tools_observed(
                request(),
                None,
                &|e| events.lock().unwrap().push(e),
                &|_| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(response.message.content.as_deref(), Some("done"));
        assert_eq!(response.usage, Usage::new(9, 2));
    }
    assert_eq!(
        provider.interaction_request_metadata(&request())["stream"],
        false
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["stage"] == "streamFallback")
            .count(),
        1
    );
    rejected.assert_async().await;
    fallback.assert_async().await;
}

#[tokio::test]
async fn unsupported_usage_option_keeps_streaming_and_records_usage_as_unknown() {
    let mut server = mockito::Server::new_async().await;
    let rejected = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(
            json!({"stream_options":{"include_usage":true}}),
        ))
        .with_status(400)
        .with_body("Unsupported parameter: stream_options")
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let mut expected = provider.tool_payload(&request()).unwrap();
    expected["stream"] = json!(true);
    let stream = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Json(expected))
        .with_header("content-type", "text/event-stream")
        .with_body(final_text())
        .expect(2)
        .create_async()
        .await;
    for _ in 0..2 {
        let response = provider
            .interact_tools_observed(request(), None, &|_| {}, &|_| Ok(()))
            .await
            .unwrap();
        assert!(!response.usage_reported);
    }
    let metadata = provider.interaction_request_metadata(&request());
    assert_eq!(metadata["stream"], true);
    assert!(metadata["streamOptions"].is_null());
    rejected.assert_async().await;
    stream.assert_async().await;
}

#[tokio::test]
async fn ignored_stream_flag_accepts_json_without_replaying_request() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .with_header("content-type", "application/json; charset=utf-8")
        .with_body(json_response())
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let response = provider
        .interact_tools_observed(request(), None, &|_| {}, &|_| Ok(()))
        .await
        .unwrap();
    assert_eq!(response.usage, Usage::new(9, 2));
    assert_eq!(
        provider.interaction_request_metadata(&request())["stream"],
        false
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn incomplete_stream_never_falls_back_or_returns_executable_tools() {
    let mut server = mockito::Server::new_async().await;
    let body = packet(
        json!({"content":"partial","tool_calls":[{"index":0,"id":"x",
        "function":{"name":"create_file","arguments":"{\"path\":\"x\",\"content\":\"x\"}"}}]}),
        Some("tool_calls"),
    );
    let mock = server
        .mock("POST", "/chat/completions")
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let visible = Mutex::new(String::new());
    let events = Mutex::new(Vec::new());
    assert!(provider
        .interact_tools_observed(
            request(),
            None,
            &|e| events.lock().unwrap().push(e),
            &|text| {
                visible.lock().unwrap().push_str(text);
                Ok(())
            }
        )
        .await
        .is_err());
    assert_eq!(*visible.lock().unwrap(), "partial");
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "streamFallback"));
    mock.assert_async().await;
}

#[tokio::test]
async fn unauthorized_and_unrelated_client_errors_do_not_trigger_stream_fallback() {
    for (status, body) in [
        (401, "stream unsupported"),
        (400, "context length exceeded"),
    ] {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let provider = OpenAiProvider::new(server.url(), "test-key".into());
        assert!(provider
            .interact_tools_observed(request(), None, &|_| {}, &|_| Ok(()))
            .await
            .is_err());
        assert_eq!(
            provider.interaction_request_metadata(&request())["stream"],
            true
        );
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn a_continuously_active_stream_still_obeys_task_deadline() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .with_header("content-type", "text/event-stream")
        .with_chunked_body(|writer| {
            for _ in 0..100 {
                writer.write_all(packet(json!({"content":"x"}), None).as_bytes())?;
                std::thread::sleep(Duration::from_millis(10));
            }
            writer.write_all(final_text().as_bytes())
        })
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let events = Mutex::new(Vec::new());
    let started = Instant::now();
    let result = provider
        .interact_tools_observed(
            request(),
            Some(started + Duration::from_millis(150)),
            &|e| events.lock().unwrap().push(e),
            &|_| Ok(()),
        )
        .await;
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "streamFallback"));
    mock.assert_async().await;
}

#[tokio::test]
async fn text_arrives_before_completion_and_dropping_request_records_stream_cancellation() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .with_header("content-type", "text/event-stream")
        .with_chunked_body(|writer| {
            for _ in 0..100 {
                writer.write_all(packet(json!({"content":"visible"}), None).as_bytes())?;
                std::thread::sleep(Duration::from_millis(10));
            }
            writer.write_all(final_text().as_bytes())
        })
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let events = Mutex::new(Vec::new());
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let observer = |e| events.lock().unwrap().push(e);
    let text = |delta: &str| {
        sender.send(delta.to_owned()).unwrap();
        Ok(())
    };
    {
        let request = provider.interact_tools_observed(request(), None, &observer, &text);
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("response completed before public delta: {result:?}"),
            delta = receiver.recv() => assert_eq!(delta.as_deref(), Some("visible")),
            _ = tokio::time::sleep(Duration::from_secs(3)) => panic!("no live delta"),
        }
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["stage"] == "streamEnd"));
    }
    let events = events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| e["stage"] == "stream" && e["cancelled"] == true));
    assert!(!events.iter().any(|e| e["stage"] == "streamFallback"));
    mock.assert_async().await;
}

#[tokio::test]
async fn oversized_event_without_delimiter_is_bounded() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .with_header("content-type", "text/event-stream")
        .with_body(format!("data: {}", "x".repeat(100_000)))
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiProvider::new(server.url(), "test-key".into());
    let events = Mutex::new(Vec::new());
    assert!(provider
        .interact_tools_observed(
            request(),
            None,
            &|e| events.lock().unwrap().push(e),
            &|_| panic!("no complete delta expected")
        )
        .await
        .is_err());
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["stage"] == "streamError"));
    mock.assert_async().await;
}
