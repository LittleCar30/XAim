//! 文件变更监听：基于 notify 的递归监听 + 500ms 防抖，为增量索引提供事件。
//!
//! - `RecommendedWatcher` 由调用方持有，drop 即停止监听，防抖线程随之退出；
//! - `.git`、node_modules 等内置忽略目录（复用 [`crate::index::scanner::DEFAULT_IGNORES`]）
//!   内的变化不上报；
//! - 防抖采用固定窗口：首批事件到达 500ms 后统一冲刷，期间事件合批，
//!   同一路径的连续修改折叠，rename 的 from/to 配对成 [`WatchEvent::Renamed`]；
//! - 事件经 `tokio::sync::mpsc` 发送。防抖线程是 std 线程（不占用 tokio worker、
//!   不阻塞 UI），用 `blocking_send` 投递——它不能在 async 上下文里调用，
//!   重构时注意保持本线程为纯 std 线程。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RenameMode};
use notify::Watcher as _;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode};
use tokio::sync::mpsc::Sender;

use crate::errors::{Result, XaimError};
use crate::index::scanner::DEFAULT_IGNORES;

/// 防抖窗口时长。
const DEBOUNCE: Duration = Duration::from_millis(500);

/// 文件变更事件；路径为触发变化的条目绝对路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed(PathBuf, PathBuf),
}

/// 启动对 `root` 的递归监听，把防抖后的事件发给 `tx`。
///
/// 生命周期由调用方管理：返回的 watcher 被 drop 后不再产生事件，
/// 后台防抖线程在通道关闭后自行退出。
pub fn start_watcher(root: PathBuf, tx: Sender<WatchEvent>) -> Result<RecommendedWatcher> {
    let (raw_tx, raw_rx) = channel::<Event>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let _ = raw_tx.send(event);
        }
    })
    .map_err(to_io)?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(to_io)?;

    thread::Builder::new()
        .name("xaim-index-watcher".to_string())
        .spawn(move || debounce_loop(raw_rx, tx))
        .map_err(|err| XaimError::Io(std::io::Error::other(err)))?;
    Ok(watcher)
}

/// notify 错误统一折算成 [`XaimError::Io`]，不 panic。
fn to_io(err: notify::Error) -> XaimError {
    std::io::Error::other(err).into()
}

/// 批内中间意图；rename 在冲刷阶段配对。
enum Intent {
    Create,
    Modify,
    Delete,
    RenameFrom,
    RenameTo,
}

/// 防抖主循环：阻塞等首批事件 → 固定 500ms 窗口收批 → 冲刷。
fn debounce_loop(rx: Receiver<Event>, tx: Sender<WatchEvent>) {
    let mut batch: Vec<(PathBuf, Intent)> = Vec::new();
    loop {
        match rx.recv() {
            Ok(event) => collect(&mut batch, event),
            // watcher 已被丢弃、通道关闭：冲刷剩余批次后退出线程。
            Err(_) => {
                flush(&mut batch, &tx);
                return;
            }
        }
        let deadline = Instant::now() + DEBOUNCE;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match rx.recv_timeout(remaining) {
                Ok(event) => collect(&mut batch, event),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    flush(&mut batch, &tx);
                    return;
                }
            }
        }
        flush(&mut batch, &tx);
    }
}

/// 把一次 notify 原始事件折算成批内意图，过滤忽略目录。
fn collect(batch: &mut Vec<(PathBuf, Intent)>, event: Event) {
    // 成对 rename（Windows 的 Both、macOS 的 Any 带旧/新两个路径）。
    if matches!(
        &event.kind,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both | RenameMode::Any))
    ) && event.paths.len() >= 2
    {
        let (from, to) = (event.paths[0].clone(), event.paths[1].clone());
        if !is_ignored(&from) && !is_ignored(&to) {
            batch.push((from, Intent::RenameFrom));
            batch.push((to, Intent::RenameTo));
        }
        return;
    }
    for path in event.paths {
        if is_ignored(&path) {
            continue;
        }
        let intent = match &event.kind {
            EventKind::Create(_) => Intent::Create,
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Intent::RenameFrom,
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Intent::RenameTo,
            EventKind::Modify(_) => Intent::Modify,
            EventKind::Remove(_) => Intent::Delete,
            // Access/Other/Any 对索引无意义。
            EventKind::Access(_) | EventKind::Other | EventKind::Any => continue,
        };
        batch.push((path, intent));
    }
}

/// 冲刷一批意图：配对 rename、折叠连续修改、发送事件。
fn flush(batch: &mut Vec<(PathBuf, Intent)>, tx: &Sender<WatchEvent>) {
    if batch.is_empty() {
        return;
    }
    let mut froms: VecDeque<PathBuf> = VecDeque::new();
    let mut out: Vec<WatchEvent> = Vec::new();
    for (path, intent) in batch.drain(..) {
        match intent {
            Intent::Create => out.push(WatchEvent::Created(path)),
            Intent::Modify => {
                // 同一路径连续的修改折叠成一个（一次保存常触发多条 Data 事件）。
                if matches!(out.last(), Some(WatchEvent::Modified(last)) if *last == path) {
                    continue;
                }
                out.push(WatchEvent::Modified(path));
            }
            Intent::Delete => out.push(WatchEvent::Deleted(path)),
            Intent::RenameFrom => froms.push_back(path),
            Intent::RenameTo => match froms.pop_front() {
                Some(from) => out.push(WatchEvent::Renamed(from, path)),
                // 目标路径不明（如被忽略目录吞掉）：按创建上报。
                None => out.push(WatchEvent::Created(path)),
            },
        }
    }
    // 只有 from 落地（目标被忽略等）：视为删除。
    for from in froms {
        out.push(WatchEvent::Deleted(from));
    }
    for event in out {
        // 接收端已关闭时静默丢弃；防抖线程继续消费直至 watcher 被 drop。
        let _ = tx.blocking_send(event);
    }
}

/// 路径任一层级落在内置忽略目录内（.git、node_modules 等）。
fn is_ignored(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        DEFAULT_IGNORES.iter().any(|ignored| name == *ignored)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    /// 单个事件的期望等待上限（验收：1 秒内收到）。
    const WAIT: Duration = Duration::from_secs(1);
    /// 安静阈值：足够防抖窗口冲刷完毕。
    const QUIET: Duration = Duration::from_millis(800);

    /// 事件对应的路径（Renamed 取目标路径）。
    fn event_path(event: &WatchEvent) -> &Path {
        match event {
            WatchEvent::Created(path) | WatchEvent::Modified(path) | WatchEvent::Deleted(path) => {
                path
            }
            WatchEvent::Renamed(_, to) => to,
        }
    }

    /// 等待下一个路径以 `name` 结尾的事件（跳过无关事件）。
    async fn next_for(rx: &mut mpsc::Receiver<WatchEvent>, name: &str) -> WatchEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("1 秒内应收到目标事件");
            let event = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("1 秒内应收到事件")
                .expect("通道不应关闭");
            if event_path(&event).ends_with(name) {
                return event;
            }
        }
    }

    /// 连续收集事件直到安静（防抖窗口冲刷完、无新事件）。
    async fn drain_until_quiet(rx: &mut mpsc::Receiver<WatchEvent>) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        loop {
            match tokio::time::timeout(QUIET, rx.recv()).await {
                Ok(Some(event)) => events.push(event),
                _ => return events,
            }
        }
    }

    #[tokio::test]
    async fn modified_file_emits_event_within_one_second() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let _watcher = start_watcher(tmp.path().to_path_buf(), tx).unwrap();

        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let first = drain_until_quiet(&mut rx).await;
        assert!(
            first.iter().any(|e| event_path(e).ends_with("a.txt")),
            "应看到 a.txt 的创建事件: {first:?}"
        );

        // 安静后改文件，1 秒内应收到 Modified。
        fs::write(tmp.path().join("a.txt"), b"world").unwrap();
        let event = next_for(&mut rx, "a.txt").await;
        assert!(
            matches!(event, WatchEvent::Modified(_)),
            "应为 Modified: {event:?}"
        );
    }

    #[tokio::test]
    async fn deleted_file_emits_event() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let _watcher = start_watcher(tmp.path().to_path_buf(), tx).unwrap();

        fs::write(tmp.path().join("b.txt"), b"bye").unwrap();
        let _ = drain_until_quiet(&mut rx).await;

        fs::remove_file(tmp.path().join("b.txt")).unwrap();
        let event = next_for(&mut rx, "b.txt").await;
        assert!(
            matches!(event, WatchEvent::Deleted(_)),
            "应为 Deleted: {event:?}"
        );
    }

    #[tokio::test]
    async fn rename_emits_renamed_event() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let _watcher = start_watcher(tmp.path().to_path_buf(), tx).unwrap();

        fs::write(tmp.path().join("old.txt"), b"x").unwrap();
        let _ = drain_until_quiet(&mut rx).await;

        fs::rename(tmp.path().join("old.txt"), tmp.path().join("new.txt")).unwrap();
        let events = drain_until_quiet(&mut rx).await;
        let renamed = events.iter().any(|e| {
            matches!(e, WatchEvent::Renamed(from, to)
                if from.ends_with("old.txt") && to.ends_with("new.txt"))
        });
        // 个别平台把 rename 拆成 from/to 两条事件时的等价形态。
        let fallback = events
            .iter()
            .any(|e| matches!(e, WatchEvent::Deleted(p) if p.ends_with("old.txt")))
            && events
                .iter()
                .any(|e| matches!(e, WatchEvent::Created(p) if p.ends_with("new.txt")));
        assert!(
            renamed || fallback,
            "rename 应产生 Renamed 事件: {events:?}"
        );
    }

    #[tokio::test]
    async fn ignores_default_dirs_but_reports_others() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let _watcher = start_watcher(tmp.path().to_path_buf(), tx).unwrap();

        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join(".git").join("HEAD"), b"ref").unwrap();
        fs::write(tmp.path().join("node_modules").join("pkg.js"), b"x").unwrap();
        // 对照组：普通文件事件应正常送达。
        fs::write(tmp.path().join("visible.txt"), b"x").unwrap();

        let events = drain_until_quiet(&mut rx).await;
        assert!(
            events
                .iter()
                .any(|e| event_path(e).ends_with("visible.txt")),
            "普通文件事件应送达: {events:?}"
        );
        for event in &events {
            assert!(
                !is_ignored(event_path(event)),
                "忽略目录内的事件不应上报: {event:?}"
            );
        }
    }

    #[tokio::test]
    async fn missing_root_returns_error_without_panic() {
        let tmp = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(4);
        let err = start_watcher(tmp.path().join("nope"), tx).unwrap_err();
        assert!(matches!(err, XaimError::Io(_)), "应返回 Io 错误: {err:?}");
    }

    #[tokio::test]
    async fn dropping_watcher_stops_events() {
        let tmp = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let watcher = start_watcher(tmp.path().to_path_buf(), tx).unwrap();

        fs::write(tmp.path().join("a.txt"), b"1").unwrap();
        let _ = drain_until_quiet(&mut rx).await;

        drop(watcher);
        fs::write(tmp.path().join("b.txt"), b"2").unwrap();
        let result = tokio::time::timeout(QUIET, rx.recv()).await;
        assert!(
            result.is_err() || matches!(result, Ok(None)),
            "drop 后不应再有新事件: {result:?}"
        );
    }
}
