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

pub struct TaskWorkspace {
    directory: tempfile::TempDir,
    baseline: BTreeMap<String, Vec<u8>>,
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

fn collect(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
        size: &mut usize,
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
                walk(root, &entry.path(), files, size)?;
            } else if metadata.is_file() {
                if metadata.len() > MAX_FILE_BYTES || files.len() >= MAX_FILES {
                    return Err(XaimError::Config("项目快照超过文件大小或数量上限".into()));
                }
                let content = fs::read(entry.path())?;
                *size += content.len();
                if *size > MAX_SNAPSHOT_BYTES {
                    return Err(XaimError::Config("项目快照超过 256 MB 上限".into()));
                }
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(path, content);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files, &mut 0)?;
    Ok(files)
}

impl TaskWorkspace {
    pub fn snapshot(root: &Path) -> Result<Self> {
        let baseline = collect(root)?;
        let directory = tempfile::Builder::new().prefix("xaim-task-").tempdir()?;
        for (path, content) in &baseline {
            let target = safe_path(directory.path(), path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(&target, content)?;
            fs::set_permissions(&target, fs::metadata(root.join(path))?.permissions())?;
        }
        Ok(Self {
            directory,
            baseline,
        })
    }

    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    pub fn changes(&self) -> Result<ChangeSet> {
        let current = collect(self.root())?;
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
