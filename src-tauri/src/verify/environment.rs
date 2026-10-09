//! Disposable verification source tree. Dependencies never enter tool-visible
//! snapshots or the proposed patch, and checks cannot rewrite candidate files.
use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::workspace::TaskWorkspace;
use crate::errors::{Result, XaimError};

pub struct VerificationEnvironment {
    workspace: TaskWorkspace,
    node_status: Option<String>,
    python_status: Option<String>,
    python: Option<PathBuf>,
}

fn same_manifests(candidate: &Path, original: &Path, names: &[&str]) -> bool {
    names.iter().all(|name| {
        match (
            fs::read(candidate.join(name)),
            fs::read(original.join(name)),
        ) {
            (Ok(a), Ok(b)) => a == b,
            (Err(a), Err(b)) => {
                a.kind() == std::io::ErrorKind::NotFound && b.kind() == std::io::ErrorKind::NotFound
            }
            _ => false,
        }
    })
}

// Bounded physical copying also handles package symlinks by reading their
// targets. A traversal/size limit prevents cyclic or oversized dependency trees.
fn copy_dependencies(
    source: &Path,
    target: &Path,
    bytes: &mut u64,
    entries: &mut usize,
    trusted_root: &Path,
    depth: usize,
) -> Result<()> {
    if depth > 128 {
        return Err(XaimError::Verify("依赖路径层级过深，无法验证".into()));
    }
    if !fs::canonicalize(source)?.starts_with(trusted_root) {
        return Err(XaimError::Verify("依赖链接指向项目外，无法安全复用".into()));
    }
    *entries += 1;
    if *entries > 50_000 {
        return Err(XaimError::Verify("依赖副本超过 50000 项，无法验证".into()));
    }
    let metadata = fs::metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_dependencies(
                &entry.path(),
                &target.join(entry.file_name()),
                bytes,
                entries,
                trusted_root,
                depth + 1,
            )?;
        }
    } else if metadata.is_file() {
        *bytes += metadata.len();
        if *bytes > 256 * 1024 * 1024 {
            return Err(XaimError::Verify("依赖副本超过 256 MB，无法验证".into()));
        }
        fs::copy(source, target)?;
    }
    Ok(())
}

impl VerificationEnvironment {
    pub fn prepare(candidate: &Path, original: Option<&Path>) -> Result<Self> {
        let workspace = TaskWorkspace::snapshot(candidate)?;
        let mut node_status = None;
        let mut python_status = None;
        let mut python = None;
        if let Some(original) = original {
            if candidate.join("package.json").is_file() {
                if !same_manifests(
                    candidate,
                    original,
                    &[
                        "package.json",
                        "package-lock.json",
                        "pnpm-lock.yaml",
                        "yarn.lock",
                        "pnpm-workspace.yaml",
                    ],
                ) {
                    node_status = Some("依赖清单已变化，需准备匹配的新依赖环境".into());
                } else if original.join("node_modules").is_dir() {
                    if let Err(error) = copy_dependencies(
                        &original.join("node_modules"),
                        &workspace.root().join("node_modules"),
                        &mut 0,
                        &mut 0,
                        &fs::canonicalize(original)?,
                        0,
                    ) {
                        node_status = Some(format!("依赖环境准备失败：{error}"));
                    }
                } else {
                    let needs_dependencies = fs::read(candidate.join("package.json"))
                        .ok()
                        .and_then(|s| serde_json::from_slice::<serde_json::Value>(&s).ok())
                        .is_some_and(|m| {
                            ["dependencies", "devDependencies"]
                                .iter()
                                .any(|key| m[key].as_object().is_some_and(|deps| !deps.is_empty()))
                        });
                    if needs_dependencies {
                        node_status = Some("项目未安装 node_modules，无法运行 Node 检查".into());
                    }
                }
            }
            if same_manifests(
                candidate,
                original,
                &[
                    "requirements.txt",
                    "pyproject.toml",
                    "poetry.lock",
                    "uv.lock",
                ],
            ) {
                for dir in [".venv", "venv"] {
                    let executable = original.join(dir).join(if cfg!(windows) {
                        "Scripts/python.exe"
                    } else {
                        "bin/python"
                    });
                    if executable.is_file() {
                        python = Some(executable);
                        break;
                    }
                }
            } else if candidate.join("requirements.txt").is_file()
                || candidate.join("pyproject.toml").is_file()
            {
                python_status = Some("Python 依赖清单已变化，需准备匹配的新依赖环境".into());
            }
        }
        Ok(Self {
            workspace,
            node_status,
            python_status,
            python,
        })
    }

    pub fn unavailable(&self, command: &str) -> Option<&str> {
        if ["node ", "npm ", "pnpm ", "yarn "]
            .iter()
            .any(|p| command.starts_with(p))
        {
            self.node_status.as_deref()
        } else if command.starts_with("python ") {
            self.python_status.as_deref()
        } else {
            None
        }
    }

    pub fn root(&self) -> &Path {
        self.workspace.root()
    }

    pub fn command(&self, command: &str) -> String {
        match (command.strip_prefix("python "), &self.python) {
            (Some(args), Some(python)) => format!("\"{}\" {args}", python.display()),
            _ => command.into(),
        }
    }

    pub fn side_effects(&self) -> Result<Vec<String>> {
        self.workspace.changed_paths()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependencies_are_copied_only_for_matching_manifests_and_checks_are_isolated() {
        let original = tempfile::tempdir().unwrap();
        fs::write(original.path().join("package.json"), "{}").unwrap();
        fs::write(original.path().join("Cargo.lock"), "baseline").unwrap();
        fs::create_dir(original.path().join("node_modules")).unwrap();
        fs::write(original.path().join("node_modules/dependency"), "installed").unwrap();
        let candidate = TaskWorkspace::snapshot(original.path()).unwrap();
        let env =
            VerificationEnvironment::prepare(candidate.root(), Some(original.path())).unwrap();
        assert!(env.unavailable("npm test").is_none());
        assert!(env.root().join("node_modules/dependency").is_file());
        fs::write(env.root().join("Cargo.lock"), "check side effect").unwrap();
        assert_eq!(env.side_effects().unwrap().len(), 1);
        fs::write(env.root().join("generated.bin"), [0xff, 0xfe]).unwrap();
        assert_eq!(env.side_effects().unwrap().len(), 2);
        assert!(candidate.changes().unwrap().files.is_empty());
        fs::write(candidate.root().join("package.json"), "{\"changed\":true}").unwrap();
        let changed =
            VerificationEnvironment::prepare(candidate.root(), Some(original.path())).unwrap();
        assert!(changed.unavailable("npm test").is_some());
        assert!(!changed.root().join("node_modules").exists());
    }
}
