//! Task snapshots include dirty and untracked source files without changing Git state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Result, XaimError};
use crate::tools::patch::{generate_diff, resolve_in_root};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_REPORTED_SKIPS: usize = 32;

/// Files the snapshot could not copy because of the per-file, count or total
/// size limits. They stay in the original project and are excluded from the
/// task copy so a single oversized binary cannot fail the whole task.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub skipped_files: usize,
    pub skipped_bytes: u64,
    pub paths: Vec<String>,
}

pub struct TaskWorkspace {
    directory: PathBuf,
    _temporary: Option<tempfile::TempDir>,
    baseline: BTreeMap<String, Vec<u8>>,
    limits: SnapshotLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub files: Vec<FileChange>,
}

impl FileChange {
    fn expected(&self, reverse: bool) -> &Option<String> {
        if reverse {
            &self.after
        } else {
            &self.before
        }
    }
    fn desired(&self, reverse: bool) -> &Option<String> {
        if reverse {
            &self.before
        } else {
            &self.after
        }
    }
}

pub fn excluded(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".next"
            | ".cache"
            | ".ssh"
            | ".aws"
            | ".npmrc"
            | ".pypirc"
            | ".netrc"
            | "_netrc"
            | ".git-credentials"
    ) || name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".key")
        || name.ends_with(".pem")
        || name == "credentials"
}

pub fn safe_path(root: &Path, path: &str) -> Result<PathBuf> {
    let normalized = path.replace('\\', "/");
    if normalized.is_empty()
        || normalized
            .chars()
            .any(|c| c.is_control() || c == ':' || c == '"')
    {
        return Err(XaimError::Security("文件路径无效".into()));
    }
    let relative = Path::new(&normalized);
    for part in relative.components() {
        match part {
            Component::Normal(name) if !excluded(&name.to_string_lossy()) => {}
            _ => return Err(XaimError::Security(format!("路径不允许访问: {path}"))),
        }
    }
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        if fs::symlink_metadata(&current).is_ok_and(|m| is_link(&m)) {
            return Err(XaimError::Security(format!(
                "拒绝符号链接或联接路径: {path}"
            )));
        }
    }
    resolve_in_root(root, &normalized)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn record_skip(limits: &mut SnapshotLimits, path: &str, bytes: u64) {
    limits.skipped_files += 1;
    limits.skipped_bytes = limits.skipped_bytes.saturating_add(bytes);
    if limits.paths.len() < MAX_REPORTED_SKIPS {
        limits.paths.push(path.to_string());
    }
}

/// Copy the text source tree into memory. A file over any hard limit is
/// skipped and reported instead of failing the whole task, matching how
/// mainstream agents ignore large binaries and build artifacts.
fn collect(root: &Path) -> Result<(BTreeMap<String, Vec<u8>>, SnapshotLimits)> {
    fn walk(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
        size: &mut usize,
        limits: &mut SnapshotLimits,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if excluded(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_link(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                walk(root, &entry.path(), files, size, limits)?;
            } else if metadata.is_file() {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if metadata.len() > MAX_FILE_BYTES {
                    record_skip(limits, &path, metadata.len());
                    continue;
                }
                if files.len() >= MAX_FILES {
                    record_skip(limits, &path, metadata.len());
                    continue;
                }
                let content = fs::read(entry.path())?;
                if (*size).saturating_add(content.len()) > MAX_SNAPSHOT_BYTES {
                    record_skip(limits, &path, content.len() as u64);
                    continue;
                }
                *size += content.len();
                files.insert(path, content);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    let mut limits = SnapshotLimits::default();
    walk(root, root, &mut files, &mut 0, &mut limits)?;
    Ok((files, limits))
}

fn collect_candidate(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let (files, limits) = collect(root)?;
    if limits.skipped_files > 0 {
        return Err(XaimError::PatchConflict(format!(
            "任务副本有 {} 个文件超过快照上限，无法完整核对变更；请缩小或移除本轮生成的大文件后重试。文件：{}",
            limits.skipped_files,
            limits.paths.join(", ")
        )));
    }
    Ok(files)
}

impl TaskWorkspace {
    /// Project copies must never contain application data or another task copy.
    pub fn validate_parent(root: &Path, parent: &Path) -> Result<()> {
        fs::create_dir_all(parent)?;
        let root = root.canonicalize()?;
        let parent = parent.canonicalize()?;
        let home = parent
            .parent()
            .ok_or_else(|| XaimError::Config("数据目录无效".into()))?;
        if root.starts_with(home) || home.starts_with(&root) {
            return Err(XaimError::Config(
                "项目目录与 XAim 数据目录不能互相包含，请在设置中选择独立的数据目录".into(),
            ));
        }
        Ok(())
    }

    pub fn snapshot(root: &Path) -> Result<Self> {
        let (baseline, limits) = collect(root)?;
        let directory = tempfile::Builder::new().prefix("xaim-task-").tempdir()?;
        for (path, content) in &baseline {
            let target = safe_path(directory.path(), path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(&target, content)?;
            fs::set_permissions(&target, fs::metadata(root.join(path))?.permissions())?;
        }
        Ok(Self {
            directory: directory.path().to_path_buf(),
            _temporary: Some(directory),
            baseline,
            limits,
        })
    }

    /// Keep the task's files across process exits; the retention service owns cleanup.
    pub fn managed_snapshot(root: &Path, parent: &Path, task_id: i64) -> Result<Self> {
        Self::validate_parent(root, parent)?;
        let container = tempfile::Builder::new()
            .prefix(&format!("task-{task_id}-"))
            .tempdir_in(parent)?;
        let directory = container.path().join("files");
        fs::create_dir(&directory)?;
        let (baseline, limits) = collect(root)?;
        for (path, content) in &baseline {
            let target = safe_path(&directory, path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(&target, content)?;
            fs::set_permissions(&target, fs::metadata(root.join(path))?.permissions())?;
        }
        crate::data::atomic_json(
            &container.path().join("workspace.json"),
            &crate::data::WorkspaceManifest {
                task_id,
                project_path: root.to_path_buf(),
                created_at: crate::utils::now_ms(),
            },
        )?;
        let _ = container.keep();
        Ok(Self {
            directory,
            _temporary: None,
            baseline,
            limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.directory
    }

    /// Files excluded from the task copy because of snapshot limits.
    pub fn limits(&self) -> &SnapshotLimits {
        &self.limits
    }

    /// A delivered repository becomes the new baseline, not a text patch.
    /// Call only before any candidate edits; Git metadata stays in the original.
    pub fn adopt_import(&mut self, source: &Path) -> Result<()> {
        if !self.changes()?.files.is_empty() {
            return Err(XaimError::Blocked(
                "已有待审阅编辑，请在新一轮中克隆仓库".into(),
            ));
        }
        let (baseline, limits) = collect(source)?;
        // Prepare every file before touching the live candidate. Import failure
        // must not leave a partially copied repository masquerading as edits.
        let stage = tempfile::tempdir_in(self.directory.parent().unwrap())?;
        let staged = stage.path().join("files");
        fs::create_dir(&staged)?;
        for (path, content) in &baseline {
            let target = safe_path(&staged, path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(&target, content)?;
            fs::set_permissions(&target, fs::metadata(source.join(path))?.permissions())?;
        }
        let backup = stage.path().join("previous");
        fs::rename(&self.directory, &backup)?;
        if let Err(error) = fs::rename(&staged, &self.directory) {
            fs::rename(&backup, &self.directory)?;
            return Err(error.into());
        }
        self.baseline = baseline;
        self.limits = limits;
        Ok(())
    }

    pub fn source_stats(&self) -> (usize, u64) {
        (
            self.baseline.len(),
            self.baseline.values().map(|bytes| bytes.len() as u64).sum(),
        )
    }

    /// Side-effect attribution also supports binary generated files.
    pub fn changed_paths(&self) -> Result<Vec<String>> {
        let current = collect_candidate(self.root())?;
        let paths: BTreeSet<_> = self.baseline.keys().chain(current.keys()).collect();
        Ok(paths
            .into_iter()
            .filter(|path| self.baseline.get(*path) != current.get(*path))
            .cloned()
            .collect())
    }

    pub fn source_fingerprint(&self) -> Result<blake3::Hash> {
        let current = collect_candidate(self.root())?;
        let mut hash = blake3::Hasher::new();
        for (path, bytes) in current {
            hash.update(&(path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update(&(bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
        }
        Ok(hash.finalize())
    }

    pub fn changes(&self) -> Result<ChangeSet> {
        let current = collect_candidate(self.root())?;
        let paths: BTreeSet<_> = self.baseline.keys().chain(current.keys()).collect();
        let mut files = vec![];
        for path in paths {
            let before = self.baseline.get(path);
            let after = current.get(path);
            if before == after {
                continue;
            }
            let text = |value: Option<&Vec<u8>>| -> Result<Option<String>> {
                value
                    .map(|bytes| {
                        String::from_utf8(bytes.clone()).map_err(|_| {
                            XaimError::PatchConflict(format!("{path}: 二进制变更不支持审阅"))
                        })
                    })
                    .transpose()
            };
            files.push(FileChange {
                path: path.clone(),
                before: text(before)?,
                after: text(after)?,
            });
        }
        Ok(ChangeSet { files })
    }
}

impl ChangeSet {
    pub fn diff(&self) -> String {
        self.files
            .iter()
            .map(|file| {
                let mut diff = generate_diff(
                    file.before.as_deref().unwrap_or(""),
                    file.after.as_deref().unwrap_or(""),
                    &file.path,
                );
                if file.before.is_none() {
                    diff = diff.replacen(&format!("--- a/{}", file.path), "--- /dev/null", 1);
                }
                if file.after.is_none() {
                    diff = diff.replacen(&format!("+++ b/{}", file.path), "+++ /dev/null", 1);
                }
                diff
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn apply(&self, root: &Path, reverse: bool) -> Result<()> {
        for file in &self.files {
            check(root, &file.path, file.expected(reverse))?;
        }
        let mut written: Vec<&FileChange> = vec![];
        for file in &self.files {
            let outcome = check(root, &file.path, file.expected(reverse))
                .and_then(|_| write(root, &file.path, file.desired(reverse)));
            if let Err(error) = outcome {
                for previous in written.into_iter().rev() {
                    // Do not overwrite changes made by another process during recovery.
                    if check(root, &previous.path, previous.desired(reverse)).is_ok() {
                        write(root, &previous.path, previous.expected(reverse))?;
                    }
                }
                return Err(error);
            }
            written.push(file);
        }
        Ok(())
    }
}

fn check(root: &Path, path: &str, expected: &Option<String>) -> Result<()> {
    let target = safe_path(root, path)?;
    let current = match fs::read(&target) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if current.as_deref() != expected.as_deref().map(str::as_bytes) {
        return Err(XaimError::PatchConflict(format!(
            "{path}: 文件已在任务之外发生变化，拒绝覆盖或撤销"
        )));
    }
    Ok(())
}

fn write(root: &Path, path: &str, content: &Option<String>) -> Result<()> {
    let target = safe_path(root, path)?;
    if let Some(content) = content {
        fs::create_dir_all(target.parent().unwrap())?;
        let mut temporary = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
        temporary.write_all(content.as_bytes())?;
        if let Ok(metadata) = fs::metadata(&target) {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        temporary
            .persist(&target)
            .map_err(|e| XaimError::Io(e.error))?;
    } else if target.exists() {
        fs::remove_file(target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_workspace_rejects_overlapping_data_and_project_directories() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("data/workspaces");
        assert!(TaskWorkspace::managed_snapshot(root.path(), &nested, 1).is_err());
        assert!(fs::read_dir(&nested).unwrap().next().is_none());
        let project = root.path().join("data/project");
        fs::create_dir(&project).unwrap();
        assert!(TaskWorkspace::managed_snapshot(&project, &nested, 1).is_err());
    }

    #[test]
    fn managed_workspace_keeps_task_files_after_drop() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.txt"), "before").unwrap();
        let workspace =
            TaskWorkspace::managed_snapshot(root.path(), &data.path().join("workspaces"), 42)
                .unwrap();
        let path = workspace.root().to_path_buf();
        fs::write(path.join("source.txt"), "after").unwrap();
        drop(workspace);
        assert_eq!(
            fs::read_to_string(path.join("source.txt")).unwrap(),
            "after"
        );
        let manifest: crate::data::WorkspaceManifest = serde_json::from_slice(
            &fs::read(path.parent().unwrap().join("workspace.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.task_id, 42);
        assert_eq!(
            fs::read_to_string(root.path().join("source.txt")).unwrap(),
            "before"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_apply_and_undo_preserve_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("check.sh");
        fs::write(&target, "echo before\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let workspace = TaskWorkspace::snapshot(root.path()).unwrap();
        assert_eq!(
            fs::metadata(workspace.root().join("check.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        fs::write(workspace.root().join("check.sh"), "echo after\n").unwrap();
        let changes = workspace.changes().unwrap();
        changes.apply(root.path(), false).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        changes.apply(root.path(), true).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn snapshot_keeps_dirty_and_untracked_files_without_touching_git() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "dirty user content").unwrap();
        fs::write(root.path().join("untracked.txt"), "keep me").unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        let workspace = TaskWorkspace::snapshot(root.path()).unwrap();
        fs::write(workspace.root().join("a.txt"), "agent content").unwrap();
        let changes = workspace.changes().unwrap();
        assert_eq!(changes.files.len(), 1);
        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "dirty user content"
        );
        changes.apply(root.path(), false).unwrap();
        fs::write(root.path().join("untracked.txt"), "later unrelated edit").unwrap();
        changes.apply(root.path(), true).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "dirty user content"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("untracked.txt")).unwrap(),
            "later unrelated edit"
        );
    }

    #[test]
    fn oversized_files_are_skipped_with_a_report_instead_of_failing() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("keep.txt"), "hello").unwrap();
        let big = root.path().join("big.zip");
        fs::File::create(&big)
            .unwrap()
            .set_len(40 * 1024 * 1024)
            .unwrap();
        let workspace = TaskWorkspace::snapshot(root.path()).unwrap();
        assert!(workspace.root().join("keep.txt").is_file());
        assert!(!workspace.root().join("big.zip").exists());
        assert_eq!(workspace.limits().skipped_files, 1);
        assert_eq!(workspace.limits().paths, vec!["big.zip".to_string()]);
        assert!(workspace.changes().unwrap().files.is_empty());
    }

    #[test]
    fn oversized_candidate_is_never_reported_as_a_deleted_file() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("keep.txt"), "original").unwrap();
        let workspace = TaskWorkspace::snapshot(root.path()).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(workspace.root().join("keep.txt"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();

        assert!(workspace.changes().is_err());
        assert!(workspace.changed_paths().is_err());
        assert!(workspace.source_fingerprint().is_err());
        assert_eq!(fs::read(root.path().join("keep.txt")).unwrap(), b"original");
    }

    #[test]
    fn oversized_new_candidate_cannot_disappear_from_review() {
        let root = tempfile::tempdir().unwrap();
        let workspace = TaskWorkspace::snapshot(root.path()).unwrap();
        fs::File::create(workspace.root().join("generated.txt"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();

        assert!(workspace.changes().is_err());
        assert!(workspace.source_fingerprint().is_err());
    }

    #[test]
    fn changed_files_block_apply_and_rollback_before_any_writes() {
        let root = tempfile::tempdir().unwrap();
        for file in ["a", "b"] {
            fs::write(root.path().join(file), "old").unwrap();
        }
        let changes = ChangeSet {
            files: ["a", "b"]
                .into_iter()
                .map(|path| FileChange {
                    path: path.into(),
                    before: Some("old".into()),
                    after: Some("new".into()),
                })
                .collect(),
        };
        fs::write(root.path().join("b"), "user").unwrap();
        assert!(changes.apply(root.path(), false).is_err());
        assert_eq!(fs::read_to_string(root.path().join("a")).unwrap(), "old");
        fs::write(root.path().join("b"), "old").unwrap();
        changes.apply(root.path(), false).unwrap();
        fs::write(root.path().join("b"), "later user change").unwrap();
        assert!(changes.apply(root.path(), true).is_err());
        assert_eq!(fs::read_to_string(root.path().join("a")).unwrap(), "new");
    }

    #[test]
    fn paths_cannot_access_git_credentials_or_escape() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "../outside",
            ".git/config",
            ".env",
            ".npmrc",
            ".git-credentials",
            "a/../b",
            "C:/secret",
            "a\n.txt",
            "node_modules/a",
        ] {
            assert!(safe_path(root.path(), path).is_err(), "{path}");
        }
    }
}
