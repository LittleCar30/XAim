use super::*;
use serde_json::json;
use std::collections::VecDeque;

fn options(mode: EvalMode, output: &Path) -> EvalOptions {
    EvalOptions {
        mode,
        output_dir: Some(output.to_path_buf()),
        ..Default::default()
    }
}

fn fixture(root: &Path, id: &str, verify: &str) -> PathBuf {
    let task = root.join(id);
    fs::create_dir_all(task.join("repo")).unwrap();
    fs::write(task.join("repo/input.txt"), "original\n").unwrap();
    fs::write(task.join("task.md"), "# Task\nFix the fixture\n").unwrap();
    fs::write(task.join("verify.sh"), verify).unwrap();
    task
}

async fn streamed_fixture(interrupted: bool) -> EvalReport {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(
        suite.path(),
        "001-stream",
        "set -eu\ngrep -Fq ready \"$1/result.txt\"\ntest ! -e \"$1/incomplete.txt\"\n",
    );
    let mut server = mockito::Server::new_async().await;
    let mock = server.mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJson(json!({"stream":true})))
        .with_header("content-type", "text/event-stream")
        .with_body_from_request(move |request| {
            let body: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let second = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
            let (name, args, content) = if !second {
                ("create_file", json!({"path":"result.txt","content":"ready"}), "Creating file")
            } else if interrupted {
                ("create_file", json!({"path":"incomplete.txt","content":"must not execute"}), "Partial reply")
            } else {
                ("update_task_state", json!({"completed":["Created file"],"pending":[],"ready_for_review":true,"summary":"Completed"}), "Completed")
            };
            let packet = json!({"choices":[{"index":0,"delta":{"role":"assistant","content":content,
                "tool_calls":[{"index":0,"id":if second {"finish"} else {"write"},"type":"function",
                    "function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]});
            let mut response = format!("data: {packet}\n\n");
            if !second || !interrupted {
                response += &format!("data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}));
            }
            response.into_bytes()
        }).expect(2).create_async().await;
    let provider = Arc::new(xaim_lib::model::openai::OpenAiProvider::new(
        server.url(),
        "test-key".into(),
    ));
    let report = Box::pin(run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(provider),
    ))
    .await;
    mock.assert_async().await;
    assert!(!suite.path().join("001-stream/repo/result.txt").exists());
    report
}

#[tokio::test]
async fn real_streamed_tools_flow_through_counting_provider_and_independent_acceptance() {
    let report = Box::pin(streamed_fixture(false)).await;
    let task = &report.details[0];
    assert!(task.passed, "{}", task.error.as_deref().unwrap_or("failed"));
    assert_eq!(task.model_calls, 2);
    assert_eq!((task.input_tokens, task.output_tokens), (22, 14));
    assert!(task.task_log.contains("modelText") && task.task_log.contains("Creating file"));
    assert!(task
        .diagnostics
        .iter()
        .any(|e| e["kind"] == "apiAttempt" && e["data"]["data"]["stage"] == "streamEnd"));
}

#[tokio::test]
async fn broken_stream_saves_completed_candidate_and_text_without_executing_new_tools() {
    let report = Box::pin(streamed_fixture(true)).await;
    let task = &report.details[0];
    assert!(!task.passed);
    assert_eq!(task.candidate_verify_status.as_deref(), Some("passed"));
    assert_eq!(
        (task.model_calls, task.input_tokens, task.output_tokens),
        (2, 11, 7)
    );
    assert!(task.checkpoint["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["path"] == "result.txt"));
    assert!(task.task_log.contains("Partial reply") && task.task_log.contains("interrupted"));
    assert!(!task
        .diagnostics
        .iter()
        .any(|e| e["kind"] == "apiAttempt" && e["data"]["data"]["stage"] == "streamFallback"));
}

#[tokio::test]
async fn completed_results_are_saved_before_the_batch_finishes() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(suite.path(), "001-pass", "exit 0\n");
    fixture(suite.path(), "002-slow", "sleep 10\n");
    let root = suite.path().to_owned();
    let opts = options(EvalMode::Baseline, out.path());
    let running = tokio::spawn(async move { run_eval_with_options(&root, &opts).await });
    let saved = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(bytes) = fs::read(out.path().join("baseline.json")) {
                if let Ok(report) = serde_json::from_slice::<EvalReport>(&bytes) {
                    break report;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    running.abort();
    let _ = running.await;
    let saved = saved.expect("first completed task must be durable during the next task");
    assert_eq!(saved.total, 1);
    assert_eq!(saved.metadata["expected_tasks"], 2);
    assert_eq!(saved.metadata["complete"], false);
    assert_eq!(saved.details[0].id, "001-pass");
    assert!(fs::read_to_string(out.path().join("baseline.md"))
        .unwrap()
        .contains("批次未完成"));
}

#[tokio::test]
async fn baseline_isolated_sorted_and_records_malformed_tasks() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let pass = fixture(suite.path(), "002-pass", "set -eu\ntest -f \"$1/input.txt\"\nprintf '%s\\n' \"$1\"\necho changed > \"$1/input.txt\"\n");
    fixture(
        suite.path(),
        "001-fail",
        "echo assertion-failed >&2\nexit 9\n",
    );
    let missing = fixture(suite.path(), "003-missing-verify", "exit 0\n");
    fs::remove_file(missing.join("verify.sh")).unwrap();
    let missing = fixture(suite.path(), "004-missing-repo", "exit 0\n");
    fs::remove_dir_all(missing.join("repo")).unwrap();
    let report =
        run_eval_with_options(suite.path(), &options(EvalMode::Baseline, out.path())).await;
    assert_eq!((report.total, report.passed, report.failed), (4, 1, 3));
    assert_eq!(
        report
            .details
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        [
            "001-fail",
            "002-pass",
            "003-missing-verify",
            "004-missing-repo"
        ]
    );
    assert_eq!(
        fs::read_to_string(pass.join("repo/input.txt")).unwrap(),
        "original\n"
    );
    assert!(
        !Path::new(report.details[1].verify_log.trim()).exists(),
        "temporary repo must be removed"
    );
    assert!(report.details[0].verify_log.contains("assertion-failed"));
    let stored: EvalReport =
        serde_json::from_slice(&fs::read(out.path().join("baseline.json")).unwrap()).unwrap();
    assert_eq!(stored.failed, 3);
    let markdown = fs::read_to_string(out.path().join("baseline.md")).unwrap();
    assert!(markdown.contains("25.00%"));
    assert!(markdown.contains("assertion-failed"));
}

#[tokio::test]
async fn empty_missing_and_report_write_errors_are_visible() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let opts = options(EvalMode::Baseline, out.path());
    let report = run_eval_with_options(suite.path(), &opts).await;
    assert_eq!(
        (report.total, report.avg_tokens, report.avg_duration_ms),
        (0, 0, 0)
    );
    assert!(!report.errors.is_empty());
    let report = run_eval_with_options(&suite.path().join("missing"), &opts).await;
    assert!(report.errors[0].contains("Read tasks"));
    fixture(suite.path(), "001-pass", "exit 0\n");
    let output_file = out.path().join("file");
    fs::write(&output_file, "occupied").unwrap();
    let report =
        run_eval_with_options(suite.path(), &options(EvalMode::Baseline, &output_file)).await;
    assert_eq!(report.passed, 1);
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("Write reports")));
}

#[tokio::test]
async fn reference_applies_patch_and_verifies_candidate_path_with_spaces() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let task = fixture(
        suite.path(),
        "001 task with spaces",
        "set -eu\ngrep -Fq 'fixed' \"$1/input.txt\"\n",
    );
    fs::write(task.join("expected.diff"), "diff --git a/input.txt b/input.txt\n--- a/input.txt\n+++ b/input.txt\n@@ -1 +1 @@\n-original\n+fixed\n").unwrap();
    let report =
        run_eval_with_options(suite.path(), &options(EvalMode::Reference, out.path())).await;
    assert_eq!(report.passed, 1, "{:?}", report.details);
    assert_eq!(report.avg_tokens, 0);
    assert_eq!(
        fs::read_to_string(task.join("repo/input.txt")).unwrap(),
        "original\n"
    );
}

#[tokio::test]
async fn command_timeout_and_missing_program_fail_without_hanging() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(
        suite.path(),
        "001-timeout",
        "echo before-timeout\nsleep 10\n",
    );
    let mut opts = options(EvalMode::Baseline, out.path());
    opts.verify_timeout = Duration::from_secs(1);
    let started = Instant::now();
    let report = run_eval_with_options(suite.path(), &opts).await;
    assert!(report.details[0]
        .error
        .as_ref()
        .unwrap()
        .contains("timed out"));
    assert!(report.details[0].verify_log.contains("before-timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout must stop the process tree promptly"
    );
    opts.bash = suite.path().join("missing-bash");
    let report = run_eval_with_options(suite.path(), &opts).await;
    assert!(report.details[0]
        .error
        .as_ref()
        .unwrap()
        .contains("Spawn command"));
}

struct ScriptedProvider(Mutex<VecDeque<String>>);

struct ToolProvider;
struct TimeoutToolProvider(AtomicU64);
#[async_trait]
impl ModelProvider for TimeoutToolProvider {
    fn supports_tools(&self) -> bool {
        true
    }
    async fn complete_tools(&self, req: ToolRequest) -> XaimResult<ToolResponse> {
        if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            ToolProvider.complete_tools(req).await
        } else {
            std::future::pending().await
        }
    }
    async fn complete(&self, _: CompletionRequest) -> XaimResult<CompletionResponse> {
        unreachable!()
    }
    async fn stream(
        &self,
        _: CompletionRequest,
    ) -> XaimResult<Pin<Box<dyn Stream<Item = XaimResult<String>> + Send>>> {
        unreachable!()
    }
    fn name(&self) -> &str {
        "timeout-tools"
    }
    fn max_context(&self) -> usize {
        128_000
    }
}

#[tokio::test]
async fn timeout_preserves_candidate_and_diagnostics_without_passing_strict_completion() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(
        suite.path(),
        "001-partial",
        "set -eu\ngrep -Fq ready \"$1/result.txt\"\n",
    );
    let mut opts = options(EvalMode::Agent, out.path());
    opts.task_timeout = Duration::from_secs(3);
    let report = run_with_provider(
        suite.path(),
        &opts,
        Some(Arc::new(TimeoutToolProvider(AtomicU64::new(0)))),
    )
    .await;
    let task = &report.details[0];
    assert!(!task.passed);
    assert_eq!(task.task_status, "failed");
    assert_eq!(task.verify_status, "failed");
    assert_eq!(task.candidate_verify_status.as_deref(), Some("passed"));
    assert!(!task.diagnostics.is_empty());
    assert!(task.checkpoint["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "result.txt"));
    assert!(task.artifacts.iter().all(|path| Path::new(path).is_file()));
    let diff = fs::read_to_string(out.path().join("artifacts/001-partial/partial.diff")).unwrap();
    assert!(diff.contains("ready"));
    assert!(!suite.path().join("001-partial/repo/result.txt").exists());
}
#[async_trait]
impl ModelProvider for ToolProvider {
    fn supports_tools(&self) -> bool {
        true
    }
    fn supports_checkpoint_context(&self) -> bool {
        true
    }
    async fn complete_tools(
        &self,
        req: xaim_lib::model::provider::ToolRequest,
    ) -> XaimResult<xaim_lib::model::provider::ToolResponse> {
        use xaim_lib::model::provider::{FunctionCall, ToolCall, ToolMessage, ToolResponse, Usage};
        let already_created = req.messages.iter().any(|m| m.role == "tool");
        let mut message = ToolMessage::text("assistant", "Completed");
        if !already_created {
            message.tool_calls.push(ToolCall {
                id: "create".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "create_file".into(),
                    arguments: r#"{"path":"result.txt","content":"ready"}"#.into(),
                },
            });
        }
        Ok(ToolResponse {
            message,
            finish_reason: if already_created {
                "stop"
            } else {
                "tool_calls"
            }
            .into(),
            usage: Usage::new(13, 9),
            usage_reported: true,
            reasoning_tokens: None,
        })
    }
    async fn complete(&self, _: CompletionRequest) -> XaimResult<CompletionResponse> {
        panic!("native evaluation must not use patch pipeline")
    }
    async fn stream(
        &self,
        _: CompletionRequest,
    ) -> XaimResult<Pin<Box<dyn Stream<Item = XaimResult<String>> + Send>>> {
        unreachable!()
    }
    fn name(&self) -> &str {
        "native-eval-test"
    }
    fn max_context(&self) -> usize {
        128_000
    }
}

#[tokio::test]
async fn native_eval_preserves_tool_capabilities_and_counts_tool_usage() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(
        suite.path(),
        "001-native",
        "set -eu\ngrep -Fq ready \"$1/result.txt\"\n",
    );
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(Arc::new(ToolProvider)),
    )
    .await;
    assert_eq!(report.passed, 1, "{:?}", report.details);
    assert_eq!(report.details[0].model_calls, 2);
    assert_eq!(report.details[0].input_tokens, 26);
    assert_eq!(report.details[0].output_tokens, 18);
    assert_eq!(report.metadata["supports_tools"], true);
    let counted = CountingProvider::new(Arc::new(ToolProvider));
    assert!(counted.supports_checkpoint_context());
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    async fn complete(&self, _req: CompletionRequest) -> XaimResult<CompletionResponse> {
        let content = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| XaimError::Model("unexpected request".into()))?;
        Ok(CompletionResponse {
            content,
            usage: xaim_lib::model::provider::Usage::new(11, 7),
        })
    }
    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> XaimResult<Pin<Box<dyn Stream<Item = XaimResult<String>> + Send>>> {
        unreachable!()
    }
    fn name(&self) -> &str {
        "eval-test"
    }
    fn max_context(&self) -> usize {
        8192
    }
}

#[tokio::test]
async fn agent_uses_real_orchestrator_applies_patch_and_counts_all_usage() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let task = fixture(
        suite.path(),
        "001-add",
        "set -eu\ngrep -Fq 'a + b' \"$1/src/lib.rs\"\n",
    );
    fs::create_dir_all(task.join("repo/src")).unwrap();
    fs::write(
        task.join("repo/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a - b }\n",
    )
    .unwrap();
    fs::write(
        task.join("task.md"),
        "Fix add in src/lib.rs to add its arguments",
    )
    .unwrap();
    let provider = Arc::new(ScriptedProvider(Mutex::new(VecDeque::from([
        r#"{"action":"fix_bug","target":"add","description":"fix add","constraints":[],"verify":[]}"#.into(),
        r#"{"steps":[{"action":"modify","file":"src/lib.rs","reason":"add"}],"files_to_modify":["src/lib.rs"],"risks":[]}"#.into(),
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn add(a: i32, b: i32) -> i32 { a - b }\n+pub fn add(a: i32, b: i32) -> i32 { a + b }\n".into(),
        r#"{"confidence":0.99}"#.into(),
    ]))));
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(provider.clone()),
    )
    .await;
    assert_eq!(report.passed, 1, "{:?}", report.details);
    assert_eq!(report.details[0].model_calls, 4);
    assert_eq!(report.details[0].input_tokens, 44);
    assert_eq!(report.details[0].output_tokens, 28);
    assert_eq!(report.avg_tokens, 72);
    assert!(provider.0.lock().unwrap().is_empty());
    assert!(fs::read_to_string(task.join("repo/src/lib.rs"))
        .unwrap()
        .contains("a - b"));
}

#[tokio::test]
async fn agent_failure_is_preserved_even_when_verification_passes() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(suite.path(), "001-agent-error", "exit 0\n");
    let provider = Arc::new(ScriptedProvider(Mutex::new(VecDeque::new())));
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(provider),
    )
    .await;
    assert_eq!(report.failed, 1);
    assert_eq!(report.details[0].verify_status, "passed");
    assert_eq!(report.details[0].task_status, "failed");
    assert!(report.details[0]
        .error
        .as_ref()
        .unwrap()
        .contains("unexpected request"));
}

#[tokio::test]
async fn agent_prepares_dependencies_and_records_preparation_failure() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let task = fixture(
        suite.path(),
        "001-prepare",
        "test -f \"$1/dependency-ready\"\n",
    );
    let metadata = serde_json::json!({
        "prepare": [["node", "-e", "require('node:fs').writeFileSync('dependency-ready', 'ready')"]]
    });
    fs::write(
        task.join("task.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let provider = Arc::new(ScriptedProvider(Mutex::new(VecDeque::new())));
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(provider.clone()),
    )
    .await;
    assert_eq!(report.details[0].verify_status, "passed");
    assert_eq!(report.details[0].model_calls, 1);
    assert!(report.details[0].task_log.contains("dependency-ready"));
    assert!(!task.join("repo/dependency-ready").exists());

    fs::write(
        task.join("task.json"),
        r#"{"prepare":[["node","-e","process.exit(3)"]]}"#,
    )
    .unwrap();
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(provider),
    )
    .await;
    assert_eq!(report.failed, 1);
    assert_eq!(report.details[0].model_calls, 0);
    assert!(report.details[0]
        .error
        .as_ref()
        .unwrap()
        .contains("Prepare task"));
}

#[tokio::test]
async fn final_verifier_receives_candidate_sources_without_seeded_dependency_metadata() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let task = fixture(
        suite.path(),
        "001-upgrade-environment",
        "set -eu\ntest -f \"$1/result.txt\"\ntest ! -f \"$1/.venv/lib/version-1.dist-info\"\n",
    );
    fs::write(task.join("task.json"), serde_json::json!({"prepare":[["node","-e",
        "const fs=require('node:fs');fs.mkdirSync('.venv/lib',{recursive:true});fs.writeFileSync('.venv/lib/version-1.dist-info','seeded-v1')"]]}).to_string()).unwrap();
    let report = run_with_provider(
        suite.path(),
        &options(EvalMode::Agent, out.path()),
        Some(Arc::new(ToolProvider)),
    )
    .await;
    assert_eq!(report.passed, 1, "{:?}", report.details);
    assert_eq!(report.details[0].model_calls, 2);
    assert!(!task.join("repo/.venv").exists());
}

#[tokio::test]
async fn agent_without_configuration_cannot_pass_on_verify_alone() {
    let suite = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(suite.path(), "001-config", "exit 0\n");
    let report = run_eval_with_options(suite.path(), &options(EvalMode::Agent, out.path())).await;
    assert_eq!((report.total, report.passed, report.failed), (1, 0, 1));
    assert_eq!(report.details[0].verify_status, "passed");
    assert!(report.details[0]
        .error
        .as_ref()
        .unwrap()
        .contains("requires --config"));
}
