//! 项目相关命令：打开/关闭/列出项目、文件树与文件读取。
//!
//! 打开项目 = 幂等注册 `projects` 记录 → 全量索引 → 启动增量监听。
//! 业务逻辑放在本模块的私有函数里（接收 `&AppState`，便于直接单测），
//! `#[tauri::command]` 只做参数转发与错误转 `String`。
//!
//! `read_file` 的安全策略与 `tools::read::ReadFileTool` 保持一致：
//! 路径解析后必须仍在项目根内、敏感文件（`.env`/`*.key`/`*.pem`）拒绝读取。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::mpsc;

use crate::errors::XaimError;
use crate::index::store as index_store;
use crate::index::{scanner, watcher};
use crate::state::AppState;
use crate::store::{project, Store};
use crate::types::{FileNode, Project};

/// 选择单个本地项目文件夹；取消返回 None，不改变当前项目。
#[tauri::command]
pub async fn pick_project_folder(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择项目文件夹")
        .pick_folder(move |folder| {
            let _ = sender.send(folder);
        });
    let folder = receiver
        .await
        .map_err(|_| "文件夹选择器已关闭".to_string())?;
    folder
        .map(|folder| {
            let path = folder.into_path().map_err(|error| error.to_string())?;
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "文件夹路径包含无法识别的字符".to_string())
        })
        .transpose()
}

/// 打开（或初始化）项目，返回项目元信息。
///
/// 目录不存在或不是目录时报错且不影响已打开的项目；重复打开同一目录
/// 幂等（同 id）；已打开其他项目时先替换关闭。命令返回即代表全量
/// 索引完成（大项目可能耗时数秒）。
#[tauri::command]
pub async fn open_project(path: String, state: State<'_, AppState>) -> Result<Project, String> {
    open(&state, &path).await.map_err(String::from)
}

/// 关闭当前项目并释放索引/监听资源。
#[tauri::command]
pub async fn close_project(state: State<'_, AppState>) -> Result<(), String> {
    close(&state).await.map_err(String::from)
}

/// 全部项目，按最近打开时间倒序。
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    list(&state).await.map_err(String::from)
}

/// 项目文件树：跳过 .gitignore/敏感/二进制文件，路径为项目内相对路径。
#[tauri::command]
pub async fn get_file_tree(
    project_id: i64,
    state: State<'_, AppState>,
) -> Result<FileNode, String> {
    tree(&state, project_id).await.map_err(String::from)
}

/// 读取项目内文本文件，可按 1 起始的双端包含行区间截取；
/// `start`/`end` 均缺省返回全文，`end` 超出文件末尾时截断。
#[tauri::command]
pub async fn read_file(
    project_id: i64,
    path: String,
    start: Option<u32>,
    end: Option<u32>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    read(&state, project_id, &path, start, end)
        .await
        .map_err(String::from)
}

/// `open_project` 的实现：注册 → 启动监听 → 全量索引 → 记为当前项目。
///
/// 监听先于索引启动：索引期间的文件改动不丢事件；索引失败时显式
/// 停掉监听，不留无人管理的后台任务。失败路径不改已打开的项目。
pub(super) async fn open(state: &AppState, raw_path: &str) -> Result<Project, XaimError> {
    let root = normalize_root(raw_path)?;
    if !root.is_dir() {
        return Err(XaimError::Config(format!(
            "项目根目录不存在或不是目录: {}",
            root.display()
        )));
    }
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    let project_id = {
        let conn = state.store.conn();
        project::insert_project(&conn, &root.to_string_lossy(), &name)?
    };

    let (tx, rx) = mpsc::channel(256);
    let watcher = watcher::start_watcher(root.clone(), tx)?;
    spawn_index_worker(state.store.clone(), project_id, rx);
    if let Err(err) =
        index_store::index_project_async(state.store.clone(), project_id, root.clone()).await
    {
        drop(watcher);
        return Err(err);
    }

    let project = {
        let conn = state.store.conn();
        project::update_last_opened(&conn, project_id)?;
        project::get_project(&conn, project_id)?
            .ok_or_else(|| XaimError::Unknown(format!("项目 {project_id} 刚注册却查询不到")))?
    };
    state.set_current_project(project.clone(), watcher);
    Ok(project)
}

/// `close_project` 的实现：停掉监听并清空当前项目；未打开时幂等成功。
pub(super) async fn close(state: &AppState) -> Result<(), XaimError> {
    state.take_current_project();
    Ok(())
}

/// `list_projects` 的实现。
pub(super) async fn list(state: &AppState) -> Result<Vec<Project>, XaimError> {
    let conn = state.store.conn();
    project::list_projects(&conn)
}

/// `get_file_tree` 的实现：任意已注册项目均可查询，不要求处于打开状态。
pub(super) async fn tree(state: &AppState, project_id: i64) -> Result<FileNode, XaimError> {
    let root = project_root(&state.store, project_id)?;
    scanner::scan_project_async(root, Vec::new()).await
}

/// `read_file` 的实现，语义与 `tools::read::ReadFileTool` 一致。
pub(super) async fn read(
    state: &AppState,
    project_id: i64,
    path: &str,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<String, XaimError> {
    if path.trim().is_empty() {
        return Err(XaimError::Unknown("path 不能为空".into()));
    }
    for (key, value) in [("start", start), ("end", end)] {
        if value == Some(0) {
            return Err(XaimError::Unknown(format!("{key} 必须是不小于 1 的行号")));
        }
    }
    if let (Some(s), Some(e)) = (start, end) {
        if s > e {
            return Err(XaimError::Unknown(format!(
                "start ({s}) 不能大于 end ({e})"
            )));
        }
    }

    let root = project_root(&state.store, project_id)?;
    let target = Path::new(path);
    let candidate = if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    };

    let meta = tokio::fs::metadata(&candidate).await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            XaimError::Unknown(format!("文件不存在: {}", candidate.display()))
        } else {
            err.into()
        }
    })?;
    if meta.is_dir() {
        return Err(XaimError::Unknown(format!(
            "路径指向目录而非文件: {}",
            candidate.display()
        )));
    }

    // 越界检查：解析符号链接与 `..` 后必须仍在项目根目录内。
    let canonical_root = tokio::fs::canonicalize(&root).await?;
    let resolved = tokio::fs::canonicalize(&candidate).await?;
    if !resolved.starts_with(&canonical_root) {
        return Err(XaimError::Security(format!("路径越出项目目录: {path}")));
    }
    if let Some(name) = resolved.file_name().and_then(|name| name.to_str()) {
        if scanner::is_sensitive(name) {
            return Err(XaimError::Security(format!("拒绝读取敏感文件: {name}")));
        }
    }

    let content = tokio::fs::read_to_string(&resolved).await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::InvalidData {
            XaimError::Unknown(format!("文件不是有效的 UTF-8 文本: {}", resolved.display()))
        } else {
            err.into()
        }
    })?;

    slice_lines(&content, start, end)
}

/// 行区间截取：1 起始、双端包含；未给区间返回全文，end 超出文件末尾时
/// 截断，start 超出总行数报错。语义与 `tools::read` 保持一致。
fn slice_lines(content: &str, start: Option<u32>, end: Option<u32>) -> Result<String, XaimError> {
    if start.is_none() && end.is_none() {
        return Ok(content.to_string());
    }
    let total = content.lines().count();
    let s = start.unwrap_or(1) as usize;
    let e = end.unwrap_or(total as u32).min(total as u32) as usize;
    if s > total {
        return Err(XaimError::Unknown(format!(
            "start 超出文件行数: 文件共 {total} 行，请求从第 {s} 行开始"
        )));
    }
    Ok(content
        .lines()
        .skip(s - 1)
        .take(e - s + 1)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// 查项目根目录；项目不存在返回 [`XaimError::Config`]。
fn project_root(store: &Store, project_id: i64) -> Result<PathBuf, XaimError> {
    let conn = store.conn();
    let Some(project) = project::get_project(&conn, project_id)? else {
        return Err(XaimError::Config(format!("项目 {project_id} 不存在")));
    };
    Ok(PathBuf::from(project.path))
}

/// 规范化项目根路径：解析为绝对路径，并剥掉 Windows canonicalize 的
/// `\\?\` / `\\?\UNC\` 前缀——保证存储的路径与 watcher 事件、`files.path`
/// 形态一致，同一目录不会因盘符大小写等写法差异注册成多条记录。
fn normalize_root(raw: &str) -> Result<PathBuf, XaimError> {
    if raw.trim().is_empty() {
        return Err(XaimError::Config("项目路径不能为空".into()));
    }
    let canonical = std::fs::canonicalize(raw).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            XaimError::Config(format!("项目目录不存在: {raw}"))
        } else {
            err.into()
        }
    })?;
    Ok(strip_unc_prefix(canonical))
}

/// 剥掉 Windows 扩展长度路径前缀（`\\?\UNC\server\share` → `\\server\share`）。
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

/// 启动增量索引后台任务：消费 watcher 事件维护 `files`/`symbols` 表，
/// watcher 停止（事件通道关闭）后任务自然退出。失败只记日志不中断。
fn spawn_index_worker(store: Store, project_id: i64, mut rx: mpsc::Receiver<watcher::WatchEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let result = match &event {
                watcher::WatchEvent::Created(path) | watcher::WatchEvent::Modified(path) => {
                    index_store::update_file_index(&store, project_id, path)
                }
                watcher::WatchEvent::Deleted(path) => {
                    index_store::remove_file_index(&store, project_id, path)
                }
                watcher::WatchEvent::Renamed(from, to) => {
                    index_store::remove_file_index(&store, project_id, from)
                        .and_then(|()| index_store::update_file_index(&store, project_id, to))
                }
            };
            if let Err(err) = result {
                tracing::warn!(?event, %err, "增量索引失败");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use crate::store::file as file_store;

    /// 内存库 + 空 current 的测试状态。
    fn test_state() -> AppState {
        AppState::with_store(Store::open_in_memory().unwrap())
    }

    /// 在 `root` 下创建相对路径 `rel` 的文件（自动补建父目录）。
    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 项目已索引的文件数。
    fn file_count(store: &Store, project_id: i64) -> i64 {
        let conn = store.conn();
        conn.query_row(
            "SELECT COUNT(*) FROM files WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// 在树中按相对路径递归查找节点。
    fn find_node<'a>(node: &'a FileNode, path: &str) -> Option<&'a FileNode> {
        if node.path == path {
            return Some(node);
        }
        node.children.iter().find_map(|c| find_node(c, path))
    }

    #[tokio::test]
    async fn open_registers_indexes_and_sets_current() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/main.rs", "fn alpha() {}\n");
        write_file(dir.path(), "readme.md", "# hi\n");
        let state = test_state();

        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        assert_eq!(
            project.name,
            dir.path().file_name().unwrap().to_string_lossy(),
            "项目名应取目录末段"
        );
        assert!(Path::new(&project.path).is_absolute());
        assert_eq!(state.current_project().map(|p| p.id), Some(project.id));

        assert_eq!(
            file_count(&state.store, project.id),
            2,
            "打开时应自动全量索引"
        );
        let conn = state.store.conn();
        let record = file_store::get_file(&conn, project.id, "src/main.rs")
            .unwrap()
            .unwrap();
        assert_eq!(record.language.as_deref(), Some("rust"));
    }

    #[tokio::test]
    async fn open_is_idempotent_for_same_directory() {
        let dir = tempdir().unwrap();
        let state = test_state();
        let path = dir.path().to_str().unwrap();

        let first = open(&state, path).await.unwrap();
        let second = open(&state, path).await.unwrap();

        assert_eq!(first.id, second.id, "重复打开应复用同一记录");
        assert_eq!(list(&state).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn open_missing_dir_fails_without_touching_current() {
        let dir = tempdir().unwrap();
        let state = test_state();
        let first = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let missing = dir.path().join("does-not-exist");
        let err = open(&state, missing.to_str().unwrap()).await.unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "应为 Config 错误: {err:?}"
        );
        assert_eq!(
            state.current_project().map(|p| p.id),
            Some(first.id),
            "打开失败不应影响已打开的项目"
        );
    }

    #[tokio::test]
    async fn open_rejects_empty_and_file_paths() {
        let dir = tempdir().unwrap();
        let state = test_state();

        let err = open(&state, "   ").await.unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));

        let file = dir.path().join("plain.txt");
        fs::write(&file, "x").unwrap();
        let err = open(&state, file.to_str().unwrap()).await.unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "文件路径不是项目根: {err:?}"
        );
    }

    #[tokio::test]
    async fn open_replaces_previously_opened_project() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let state = test_state();
        open(&state, a.path().to_str().unwrap()).await.unwrap();
        let second = open(&state, b.path().to_str().unwrap()).await.unwrap();

        assert_eq!(state.current_project().map(|p| p.id), Some(second.id));
        let listed = list(&state).await.unwrap();
        assert_eq!(listed.len(), 2, "两个项目都应保留在记录里");
    }

    #[tokio::test]
    async fn close_clears_current_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let state = test_state();
        open(&state, dir.path().to_str().unwrap()).await.unwrap();

        close(&state).await.unwrap();
        assert!(state.current_project().is_none(), "关闭后当前项目应为空");
        close(&state).await.unwrap(); // 未打开时再关闭应幂等成功
    }

    #[tokio::test]
    async fn list_orders_projects_by_last_opened_desc() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let state = test_state();
        let first = open(&state, a.path().to_str().unwrap()).await.unwrap();
        let second = open(&state, b.path().to_str().unwrap()).await.unwrap();

        let ids: Vec<i64> = list(&state)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![second.id, first.id], "最近打开的应排前面");
    }

    #[tokio::test]
    async fn tree_returns_relative_paths_from_project_root() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/deep/mod.rs", "fn main() {}\n");
        write_file(dir.path(), "docs/readme.md", "# doc\n");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let tree = tree(&state, project.id).await.unwrap();

        assert_eq!(tree.path, ".");
        assert!(tree.is_dir);
        let mod_rs = find_node(&tree, "src/deep/mod.rs").expect("文件树应包含 src/deep/mod.rs");
        assert!(!mod_rs.is_dir);
        assert_eq!(mod_rs.size, Some("fn main() {}\n".len() as u64));
        assert!(find_node(&tree, "docs/readme.md").is_some());
    }

    #[tokio::test]
    async fn tree_for_unknown_project_fails() {
        let state = test_state();
        let err = tree(&state, 999).await.unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
    }

    #[tokio::test]
    async fn read_returns_whole_file_by_default() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", "l1\nl2\nl3\nl4\nl5");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let content = read(&state, project.id, "src/lib.rs", None, None)
            .await
            .unwrap();
        assert_eq!(content, "l1\nl2\nl3\nl4\nl5");
    }

    #[tokio::test]
    async fn read_slices_inclusive_line_range() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "note.txt", "l1\nl2\nl3\nl4\nl5");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let mid = read(&state, project.id, "note.txt", Some(2), Some(4))
            .await
            .unwrap();
        assert_eq!(mid, "l2\nl3\nl4");

        let tail = read(&state, project.id, "note.txt", Some(4), None)
            .await
            .unwrap();
        assert_eq!(tail, "l4\nl5");

        let head = read(&state, project.id, "note.txt", None, Some(2))
            .await
            .unwrap();
        assert_eq!(head, "l1\nl2");

        // end 超出文件末尾时截断而非报错。
        let clamped = read(&state, project.id, "note.txt", Some(3), Some(99))
            .await
            .unwrap();
        assert_eq!(clamped, "l3\nl4\nl5");
    }

    #[tokio::test]
    async fn read_rejects_bad_ranges() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "note.txt", "l1\nl2");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let err = read(&state, project.id, "note.txt", Some(3), None)
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
        assert!(err.to_string().contains("共 2 行"), "应提示总行数: {err}");

        let err = read(&state, project.id, "note.txt", Some(2), Some(1))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)), "start > end 应报错");

        let err = read(&state, project.id, "note.txt", Some(0), None)
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)), "行号 0 应报错");
    }

    #[tokio::test]
    async fn read_rejects_empty_path_and_unknown_project() {
        let state = test_state();

        let err = read(&state, 1, "  ", None, None).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));

        let err = read(&state, 999, "a.rs", None, None).await.unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
    }

    #[tokio::test]
    async fn read_reports_missing_file_and_directory() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "note.txt", "hi");
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let err = read(&state, project.id, "ghost.txt", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
        assert!(err.to_string().contains("文件不存在"));

        let err = read(&state, project.id, "sub", None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("目录"));
    }

    #[tokio::test]
    async fn read_blocks_traversal_and_outside_absolute_paths() {
        // 兄弟目录 project/ 与 outside/：相对路径 `../outside/x.txt` 越界。
        let parent = tempdir().unwrap();
        let project_dir = tempfile::tempdir_in(parent.path()).unwrap();
        let outside = tempfile::tempdir_in(parent.path()).unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "top secret").unwrap();
        let state = test_state();
        let project = open(&state, project_dir.path().to_str().unwrap())
            .await
            .unwrap();

        let rel = format!(
            "../{}/secret.txt",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let err = read(&state, project.id, &rel, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Security(_)), "{err:?}");

        let err = read(
            &state,
            project.id,
            &secret.display().to_string(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, XaimError::Security(_)),
            "项目外绝对路径应拒绝: {err:?}"
        );
    }

    #[tokio::test]
    async fn read_refuses_sensitive_files() {
        let dir = tempdir().unwrap();
        for name in [".env", ".env.local", "server.key", "cert.pem"] {
            write_file(dir.path(), name, "secret");
        }
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        for name in [".env", ".env.local", "server.key", "cert.pem"] {
            let err = read(&state, project.id, name, None, None)
                .await
                .unwrap_err();
            assert!(
                matches!(err, XaimError::Security(_)),
                "{name} 应被安全策略拒绝: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn read_reports_non_utf8_content() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("blob.bin"), [0xffu8, 0xfe]).unwrap();
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        let err = read(&state, project.id, "blob.bin", None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }

    #[tokio::test]
    async fn watcher_keeps_index_incremental_after_open() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.rs", "fn a() {}\n");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(file_count(&state.store, project.id), 1);

        // 防抖窗口 500ms：新文件应在数秒内被增量索引进库。
        write_file(dir.path(), "b.rs", "fn b() {}\n");
        let deadline = Instant::now() + Duration::from_secs(5);
        while file_count(&state.store, project.id) < 2 {
            assert!(Instant::now() < deadline, "5 秒内应增量索引新建文件");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn close_stops_incremental_indexing() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.rs", "fn a() {}\n");
        let state = test_state();
        let project = open(&state, dir.path().to_str().unwrap()).await.unwrap();

        close(&state).await.unwrap();
        write_file(dir.path(), "late.rs", "fn late() {}\n");
        // 留足防抖窗口：关闭后新文件不应再进索引。
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(file_count(&state.store, project.id), 1);
    }

    #[test]
    fn slice_lines_matches_tool_semantics() {
        let five = "l1\nl2\nl3\nl4\nl5";
        assert_eq!(slice_lines(five, None, None).unwrap(), five);
        assert_eq!(slice_lines(five, Some(2), Some(4)).unwrap(), "l2\nl3\nl4");
        assert_eq!(slice_lines(five, Some(4), None).unwrap(), "l4\nl5");
        assert_eq!(slice_lines(five, None, Some(2)).unwrap(), "l1\nl2");
        assert_eq!(slice_lines(five, Some(3), Some(99)).unwrap(), "l3\nl4\nl5");
        assert_eq!(slice_lines("", None, None).unwrap(), "", "空文件全文为空串");
    }

    #[test]
    fn slice_lines_reports_start_beyond_eof() {
        let err = slice_lines("l1\nl2", Some(10), None).unwrap_err();
        assert!(err.to_string().contains("共 2 行"), "{err}");
    }

    #[test]
    fn normalize_root_resolves_and_stays_stable() {
        let dir = tempdir().unwrap();
        let normalized = normalize_root(dir.path().to_str().unwrap()).unwrap();

        assert!(normalized.is_absolute());
        assert!(
            !normalized.to_string_lossy().contains(r"\\?\"),
            "不应带 Windows 扩展长度前缀: {}",
            normalized.display()
        );
        // 已规范化路径再走一遍结果不变（存储比较区分大小写，要求稳定）。
        let again = normalize_root(&normalized.to_string_lossy()).unwrap();
        assert_eq!(again, normalized);
    }

    #[test]
    fn normalize_root_rejects_missing_and_empty() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        let err = normalize_root(missing.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));

        let err = normalize_root("  ").unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
    }
}
