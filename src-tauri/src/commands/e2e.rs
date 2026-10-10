//! Full workflow tests and a test-only JSON bridge for the React integration test.

use std::fs;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use mockito::{Mock, ServerGuard};
use serde_json::{json, Value};

use super::{project, task};
use crate::agent::orchestrator::Orchestrator;
use crate::errors::{Result, XaimError};
use crate::events::XaimEvent;
use crate::repair::r#loop::EventSink;
use crate::state::AppState;
use crate::store::{self, Store};
use crate::types::{Config, ModelConfig, TaskStatus};
use crate::verify::detect::detect_verify_config;
use crate::verify::runner::{run_verify, VerifyKind};

const BROKEN: &str = "pub fn add(a: i32, b: i32) -> i32 { a - b }\n";
const FIXED: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
const PROMPT: &str = "Fix add so that it returns the sum of a and b";
const DIFF: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn add(a: i32, b: i32) -> i32 { a - b }\n+pub fn add(a: i32, b: i32) -> i32 { a + b }\n";

#[derive(Default)]
struct CollectSink(Mutex<Vec<Value>>);

impl EventSink for CollectSink {
    fn emit(&self, event: XaimEvent) -> Result<()> {
        self.0.lock().unwrap().push(serde_json::to_value(event)?);
        Ok(())
    }
}

impl CollectSink {
    fn drain(&self) -> Vec<Value> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct Harness {
    state: AppState,
    sink: Arc<CollectSink>,
    fixture: tempfile::TempDir,
    _data: tempfile::TempDir,
    _server: ServerGuard,
    mocks: Vec<Mock>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Harness {
    async fn new() -> Self {
        let fixture = tempfile::tempdir().unwrap();
        fs::create_dir_all(fixture.path().join("src")).unwrap();
        fs::create_dir_all(fixture.path().join("tests")).unwrap();
        fs::write(fixture.path().join(".gitignore"), "/target\n/Cargo.lock\n").unwrap();
        fs::write(
            fixture.path().join("Cargo.toml"),
            "[package]\nname = \"xaim-e2e-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(fixture.path().join("src/lib.rs"), BROKEN).unwrap();
        fs::write(
            fixture.path().join("tests/add.rs"),
            "#[test]\nfn adds_numbers() { assert_eq!(xaim_e2e_fixture::add(2, 3), 5); }\n",
        )
        .unwrap();
        crate::git::ops::commit(fixture.path(), "Fixture baseline").unwrap();

        let mut server = mockito::Server::new_async().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut mocks = Vec::new();
        let call = |id: &str, name: &str, args: Value| {
            json!({"role":"assistant","content":null,
            "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
        };
        for (previous, response, finish) in [
            (
                None,
                call("list", "list_dir", json!({"path":"."})),
                "tool_calls",
            ),
            (
                Some("list"),
                call("read", "read_file", json!({"path":"src/lib.rs","offset":0})),
                "tool_calls",
            ),
            (
                Some("read"),
                call(
                    "edit",
                    "edit_file",
                    json!({"path":"src/lib.rs","expected_hash":blake3::hash(BROKEN.as_bytes()).to_hex().to_string(),"old_text":"a - b","new_text":"a + b"}),
                ),
                "tool_calls",
            ),
            (
                Some("edit"),
                json!({"role":"assistant","content":"Fixed addition. Program must verify the result."}),
                "stop",
            ),
        ] {
            let captured = requests.clone();
            let mock = server
                .mock("POST", "/v1/chat/completions")
                .match_header("authorization", "Bearer e2e-only")
                .match_request(move |request| {
                    let Some(body) = request
                        .body()
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                    else {
                        return false;
                    };
                    let messages = body["messages"].as_array().unwrap();
                    // Ignore the loop's transient budget notice when matching
                    // the last actionable message of the tool protocol.
                    let last = messages
                        .iter()
                        .rev()
                        .find(|message| {
                            !(message["role"] == "user"
                                && message["content"]
                                    .as_str()
                                    .is_some_and(|text| text.starts_with("程序预算状态：")))
                        })
                        .unwrap();
                    body["stream"] == true
                        && body["tools"].is_array()
                        && messages[1]["content"] == PROMPT
                        && match previous {
                            None => last["role"] == "user",
                            Some(id) => last["role"] == "tool" && last["tool_call_id"] == id,
                        }
                })
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body_from_request(move |request| {
                    captured
                        .lock()
                        .unwrap()
                        .push(serde_json::from_slice(request.body().unwrap()).unwrap());
                    let mut delta = response.clone();
                    if let Some(calls) = delta.get_mut("tool_calls").and_then(Value::as_array_mut) {
                        for (index, call) in calls.iter_mut().enumerate() {
                            call["index"] = json!(index);
                        }
                    }
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}),
                        json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":10}})
                    )
                    .into_bytes()
                })
                .expect(1)
                .create_async()
                .await;
            mocks.push(mock);
        }
        let data = tempfile::tempdir().unwrap();
        let home = data.path().join("home");
        for directory in ["", "workspaces", "backups"] {
            fs::create_dir_all(home.join(directory)).unwrap();
        }
        let mut state = AppState::with_store(Store::new(&home.join("xaim.db")).unwrap());
        state.data_paths = Some(crate::data::DataPaths {
            home,
            bootstrap: data.path().to_path_buf(),
            overridden: false,
            notice: None,
        });
        let config = Config {
            model: ModelConfig {
                provider: "openai".into(),
                name: "mock-model".into(),
                api_key: Some("e2e-only".into()),
                base_url: Some(format!("{}/v1", server.url())),
                temperature: 0.0,
                max_tokens: Some(1024),
            },
            ..Config::default()
        };
        crate::config::model::save(&state.store, &config).unwrap();
        Self {
            state,
            sink: Arc::new(CollectSink::default()),
            fixture,
            _data: data,
            _server: server,
            mocks,
            requests,
        }
    }

    async fn verify(&self) -> Result<Value> {
        let detected = detect_verify_config(self.fixture.path());
        let command = detected
            .test
            .ok_or_else(|| XaimError::Verify("No test command detected".into()))?;
        let result = run_verify(VerifyKind::Test, &command, self.fixture.path(), 60).await?;
        Ok(json!({ "command": command, "passed": result.passed, "output": result.output }))
    }

    async fn dispatch(&self, command: &str, args: &Value) -> Result<Value> {
        let id = |key| {
            args[key]
                .as_i64()
                .ok_or_else(|| XaimError::Config(format!("Missing {key}")))
        };
        match command {
            "open_project" => {
                let path = args["path"]
                    .as_str()
                    .ok_or_else(|| XaimError::Config("Missing path".into()))?;
                Ok(serde_json::to_value(
                    project::open(&self.state, path).await?,
                )?)
            }
            "list_projects" => Ok(serde_json::to_value(project::list(&self.state).await?)?),
            "get_file_tree" => Ok(serde_json::to_value(
                project::tree(&self.state, id("projectId")?).await?,
            )?),
            "get_model_config" => Ok(serde_json::to_value(
                crate::config::model::load(&self.state.store).model,
            )?),
            "get_config" => Ok(serde_json::to_value(crate::config::model::load(
                &self.state.store,
            ))?),
            "set_config" => {
                super::config::set(&self.state, serde_json::from_value(args["config"].clone())?)
                    .await?;
                Ok(Value::Null)
            }
            "get_preferences" => Ok(serde_json::to_value(
                super::data::preferences(&self.state.store).map_err(XaimError::Config)?,
            )?),
            "set_preference" => {
                let key = args["key"]
                    .as_str()
                    .ok_or_else(|| XaimError::Config("Missing key".into()))?;
                let value = args["value"]
                    .as_str()
                    .ok_or_else(|| XaimError::Config("Missing value".into()))?;
                super::data::save_preference(&self.state.store, key, value)
                    .map_err(XaimError::Config)?;
                Ok(Value::Null)
            }
            "get_storage_info" => Ok(serde_json::to_value(
                self.state
                    .data_paths
                    .as_ref()
                    .unwrap()
                    .info(&self.state.store)?,
            )?),
            "create_data_backup" => Ok(json!(self
                .state
                .data_paths
                .as_ref()
                .unwrap()
                .backup(&self.state.store)?)),
            "get_runtime_settings" => Ok(serde_json::to_value(crate::config::runtime::load(
                &self.state.store,
                args["projectId"].as_i64(),
            )?)?),
            "set_runtime_settings" => {
                crate::config::runtime::save(
                    &self.state.store,
                    args["projectId"].as_i64(),
                    serde_json::from_value(args["settings"].clone())?,
                )?;
                Ok(Value::Null)
            }
            "list_tasks" => Ok(serde_json::to_value(
                task::list(&self.state, id("projectId")?).await?,
            )?),
            "get_task" => Ok(serde_json::to_value(
                task::get(&self.state, id("taskId")?).await?,
            )?),
            "list_task_steps" => Ok(serde_json::to_value(
                task::steps(&self.state, id("taskId")?).await?,
            )?),
            "list_conversation_tasks" => Ok(serde_json::to_value(
                store::task::conversation_tasks(&self.state.store.conn(), id("taskId")?)?,
            )?),
            "list_patches" => Ok(serde_json::to_value(
                task::patches(&self.state, id("taskId")?).await?,
            )?),
            "run_task" => {
                let mut config = task::orchestrator_config(&self.state)?;
                config.parent_task_id = args["parentTaskId"].as_i64();
                let runtime =
                    crate::config::runtime::load(&self.state.store, Some(id("projectId")?))?
                        .effective;
                config.task_timeout_sec = runtime.task_timeout_sec.unwrap();
                config.tool_loop.max_turns = runtime.max_turns.unwrap();
                config.candidate_count = 1;
                config.step_timeout_sec = 60;
                config.tool_loop.allow_verification =
                    args["allowVerification"].as_bool().unwrap_or(false);
                let provider = Arc::from(crate::model::create_provider(&config.model)?);
                let orchestrator = Orchestrator::with_sink(
                    Arc::new(self.state.store.clone()),
                    config,
                    self.sink.clone(),
                    provider,
                );
                let prompt = args["prompt"]
                    .as_str()
                    .ok_or_else(|| XaimError::Config("Missing prompt".into()))?;
                Ok(json!(
                    task::run(&self.state, orchestrator, id("projectId")?, prompt).await?
                ))
            }
            "apply_patch" => {
                task::apply(&self.state, id("patchId")?).await?;
                Ok(Value::Null)
            }
            "close_project" => {
                project::close(&self.state).await?;
                Ok(Value::Null)
            }
            // These diagnostics are test-only; application IPC remains unchanged.
            "e2e_verify" => self.verify().await,
            "e2e_inspect" => {
                let model_requests = self.requests.lock().unwrap().clone();
                let conn = self.state.store.conn();
                Ok(json!({
                    "files": store::file::list_files(&conn, id("projectId")?)?,
                    "content": fs::read_to_string(self.fixture.path().join("src/lib.rs"))?,
                    "gitStatus": crate::git::ops::status(self.fixture.path())?,
                    "modelCalls": model_requests.len(),
                    "modelRequests": model_requests,
                }))
            }
            "e2e_assert_model" => {
                for mock in &self.mocks {
                    mock.assert_async().await;
                }
                Ok(Value::Null)
            }
            _ => Err(XaimError::Config(format!(
                "Unsupported test command: {command}"
            ))),
        }
    }
}

#[tokio::test]
async fn complete_project_to_index_task_diff_apply_and_verify_flow() {
    let harness = Harness::new().await;
    let baseline = harness.verify().await.unwrap();
    assert_eq!(
        baseline["passed"], false,
        "Fixture must fail before the patch"
    );
    assert!(baseline["output"]
        .as_str()
        .unwrap()
        .contains("adds_numbers"));
    let opened = harness
        .dispatch("open_project", &json!({"path": harness.fixture.path()}))
        .await
        .unwrap();
    let project_id = opened["id"].as_i64().unwrap();
    let indexed = harness
        .dispatch("e2e_inspect", &json!({"projectId": project_id}))
        .await
        .unwrap();
    let file = indexed["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/lib.rs")
        .unwrap();
    assert_eq!(file["language"], "rust");
    assert!(file["hash"].as_str().is_some_and(|hash| !hash.is_empty()));
    let task_id = harness
        .dispatch(
            "run_task",
            &json!({"projectId": project_id, "prompt": PROMPT, "allowVerification":true}),
        )
        .await
        .unwrap()
        .as_i64()
        .unwrap();
    let waiting = task::get(&harness.state, task_id).await.unwrap();
    assert_eq!(waiting.status, TaskStatus::WaitingConfirm);
    assert_eq!(
        fs::read_to_string(harness.fixture.path().join("src/lib.rs")).unwrap(),
        BROKEN
    );
    let patches = task::patches(&harness.state, task_id).await.unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].diff, DIFF);
    assert!(!patches[0].applied);
    assert!(patches[0].score.is_none(), "单执行路径不再使用模型评分");
    task::apply(&harness.state, patches[0].id).await.unwrap();
    assert_eq!(
        task::get(&harness.state, task_id).await.unwrap().status,
        TaskStatus::Done
    );
    assert!(task::patches(&harness.state, task_id).await.unwrap()[0].applied);
    assert_eq!(
        fs::read_to_string(harness.fixture.path().join("src/lib.rs")).unwrap(),
        FIXED
    );
    let verified = harness.verify().await.unwrap();
    assert_eq!(verified["passed"], true, "{}", verified["output"]);
    assert!(verified["output"].as_str().unwrap().contains("1 passed"));
    assert!(
        crate::git::ops::status(harness.fixture.path())
            .unwrap()
            .iter()
            .any(|s| s.ends_with("src/lib.rs")),
        "确认保留未提交的任务变更"
    );
    let events = harness.sink.drain();
    assert!(events
        .iter()
        .any(|event| event["type"] == "patch" && event["diff"] == DIFF));
    assert!(events
        .iter()
        .any(|event| event["type"] == "done" && event["status"] == "done"));
    assert!(!events.iter().any(|event| event["type"] == "error"));
    assert!(events
        .iter()
        .any(|event| event["type"] == "modelText" && event["status"] == "complete"));
    assert!(harness
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request["stream"] == true));
    for mock in &harness.mocks {
        mock.assert_async().await;
    }
    project::close(&harness.state).await.unwrap();
}

fn write_message(message: Value) {
    let mut output = std::io::stdout().lock();
    writeln!(output, "XAIM_E2E {message}").unwrap();
    output.flush().unwrap();
}

#[tokio::test]
#[ignore = "Started by pnpm test:e2e; requires the JSON stdin bridge client"]
async fn frontend_bridge() {
    assert_eq!(std::env::var("XAIM_E2E_BRIDGE").as_deref(), Ok("1"));
    let harness = Harness::new().await;
    write_message(json!({"ready": true, "projectPath": harness.fixture.path()}));
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let command = request["command"].as_str().unwrap();
        if command == "shutdown" {
            break;
        }
        let response = match harness.dispatch(command, &request["args"]).await {
            Ok(result) => {
                json!({"id": request["id"], "result": result, "events": harness.sink.drain()})
            }
            Err(error) => {
                json!({"id": request["id"], "error": error.to_string(), "events": harness.sink.drain()})
            }
        };
        write_message(response);
    }
    project::close(&harness.state).await.unwrap();
}
