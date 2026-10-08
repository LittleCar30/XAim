use super::*;
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
