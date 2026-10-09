#[path = "../../../tests/eval/runner.rs"]
mod runner;

use std::path::PathBuf;
use std::time::Duration;

use runner::{run_eval_with_options, EvalMode, EvalOptions};

fn arguments() -> Result<(PathBuf, EvalOptions), String> {
    let mut tasks_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/eval/tasks");
    let mut options = EvalOptions::from_env()?;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("xaim-eval [tasks-dir] [--mode agent|reference|baseline] [--config config.json] [--output reports-dir] [--bash path] [--task-timeout seconds] [--verify-timeout seconds] [--allow-verification]");
            std::process::exit(0);
        }
        if !arg.starts_with('-') {
            tasks_dir = PathBuf::from(arg);
            continue;
        }
        if arg == "--allow-verification" {
            options.allow_verification = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {arg}"))?;
        match arg.as_str() {
            "--mode" => options.mode = value.parse::<EvalMode>()?,
            "--config" => options.config_path = Some(PathBuf::from(value)),
            "--output" => options.output_dir = Some(PathBuf::from(value)),
            "--bash" => options.bash = PathBuf::from(value),
            "--task-timeout" | "--verify-timeout" => {
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| format!("Invalid timeout: {value}"))?;
                if seconds == 0 {
                    return Err("Timeout must be positive".into());
                }
                if arg == "--task-timeout" {
                    options.task_timeout = Duration::from_secs(seconds);
                } else {
                    options.verify_timeout = Duration::from_secs(seconds);
                }
            }
            _ => return Err(format!("Unknown argument: {arg}")),
        }
    }
    Ok((tasks_dir, options))
}

fn main() {
    // The debug-build orchestrator future includes both native and legacy
    // execution paths. Windows executables have a smaller main-thread stack
    // than Rust's test workers, so run it on an explicitly sized thread.
    std::thread::Builder::new()
        .name("xaim-eval".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(8 * 1024 * 1024)
                .build()
                .expect("evaluation runtime")
                .block_on(run());
        })
        .expect("evaluation thread")
        .join()
        .expect("evaluation panicked");
}

async fn run() {
    let (tasks_dir, options) = arguments().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let report = run_eval_with_options(&tasks_dir, &options).await;
    let output = options.report_dir(&tasks_dir);
    println!(
        "Mode: {}\nPassed: {}/{} ({:.2}%)\nAverage: {} ms, {} tokens\nJSON: {}\nMarkdown: {}",
        options.mode,
        report.passed,
        report.total,
        report.success_rate() * 100.0,
        report.avg_duration_ms,
        report.avg_tokens,
        output.join(format!("{}.json", options.mode)).display(),
        output.join(format!("{}.md", options.mode)).display()
    );
    for error in &report.errors {
        eprintln!("{error}");
    }
    let success = report.errors.is_empty()
        && report.total > 0
        && if options.mode == EvalMode::Baseline {
            report.passed == 0
        } else {
            report.failed == 0
        };
    if !success {
        std::process::exit(1);
    }
}
