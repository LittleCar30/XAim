//! Git 操作封装：为任务提供"回滚点 - 执行 - 提交/回滚"的工作区保护。
//!
//! 策略：**回滚点 = WIP 提交**。任务开始前 [`create_rollback_point`] 把
//! 工作区全部改动（含未跟踪文件，遵守 .gitignore）提交为带 task id 的
//! 快照提交并返回其 id；任务失败时 [`rollback`] 对该 id 做 `reset --hard`
//! 并清掉任务产出的未跟踪文件，工作区即回到任务前一刻——用户的未提交
//! 修改内容原样保留（它们就在快照提交里）。选 WIP 提交而非 stash：
//! 回滚要求 reset --hard 到具体提交，stash 在任务改动之后 pop 极易冲突，
//! 且 stash 提交会打乱分支历史。
//!
//! 无改动时回滚点直接取当前 HEAD（不产生空提交）；非 Git 仓库在创建
//! 回滚点/提交时自动 init，其余操作报 [`XaimError::Config`]。
//! 提交身份缺 user.name/user.email 时回退到 "XAim <xaim@localhost>"。

use std::path::Path;

use git2::{
    DiffFormat, DiffOptions, IndexAddOption, Repository, ResetType, Signature, StatusOptions,
};

use crate::errors::{Result, XaimError};

/// 工作区是否处于 Git 仓库内（含从子目录向上发现）。
pub fn is_git_repo(root: &Path) -> bool {
    Repository::discover(root).is_ok()
}

/// 当前分支名；detached HEAD 时返回短 commit id；
/// 尚无提交的新仓库返回 HEAD 指向的分支名（如 `master`）。
pub fn current_branch(root: &Path) -> Result<String> {
    let repo = open_repo(root)?;
    if repo.head_detached().unwrap_or(false) {
        let head = repo.head()?;
        let id = head.peel_to_commit()?.id().to_string();
        return Ok(id[..7].to_string());
    }
    let branch = match repo.head() {
        Ok(head) => head.shorthand().unwrap_or("HEAD").to_string(),
        Err(_) => {
            // 未出生 HEAD：符号引用仍指向分支名。
            let head = repo.find_reference("HEAD")?;
            head.symbolic_target()
                .unwrap_or("refs/heads/main")
                .trim_start_matches("refs/heads/")
                .to_string()
        }
    };
    Ok(branch)
}

/// 创建任务回滚点：把当前工作区（含未提交修改与未跟踪文件）快照为一个
/// 带 task id 的提交，返回提交 id；工作区干净时返回当前 HEAD id。
pub fn create_rollback_point(root: &Path, task_id: i64) -> Result<String> {
    let repo = ensure_repo(root)?;
    commit_all(&repo, &format!("XAim 回滚点 (task #{task_id})"))
}

/// 回滚到指定提交：`reset --hard` 后清除相对该提交的未跟踪文件（任务产物），
/// 工作区恢复到回滚点创建时的状态且保持干净。
///
/// 注意 `reset --hard` 会把当前分支移回该提交（任务期间的提交经 reflog 可找回）。
pub fn rollback(root: &Path, commit_id: &str) -> Result<()> {
    let repo = open_repo(root)?;
    let commit = repo
        .revparse_single(commit_id)?
        .peel_to_commit()
        .map_err(|_| XaimError::Config(format!("回滚点 {commit_id} 不是提交")))?;
    repo.reset(commit.as_object(), ResetType::Hard, None)?;
    remove_untracked(&repo)?;
    Ok(())
}

/// 回滚任务的补丁提交：找到提交消息以 `XAim task #{task_id}:` 开头的提交
/// （编排器确认阶段的提交格式），`reset --hard` 到其父提交并清理任务产出
/// 的未跟踪文件，工作区即回到补丁应用前的状态。
///
/// 找不到任务提交（补丁应用后尚未提交，或已被手工处理）或任务提交是
/// 仓库首个提交（无父提交可回）时返回 [`XaimError::Config`]。
pub fn rollback_task_commit(root: &Path, task_id: i64) -> Result<()> {
    let repo = open_repo(root)?;
    let marker = format!("XAim task #{task_id}:");
    let mut walk = repo.revwalk()?;
    walk.push_head()
        .map_err(|_| XaimError::Config("仓库没有任何提交，无法回滚".into()))?;
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        if commit
            .summary()
            .is_some_and(|summary| summary.starts_with(&marker))
        {
            return if commit.parent_count() > 0 {
                rollback(root, &commit.parent(0)?.id().to_string())
            } else {
                Err(XaimError::Config(format!(
                    "任务 #{task_id} 的提交是仓库首个提交，没有可回滚的父提交"
                )))
            };
        }
    }
    Err(XaimError::Config(format!(
        "未找到任务 #{task_id} 的补丁提交"
    )))
}

/// 工作区相对最近提交的差异（unified diff，含未跟踪文件内容）。
/// 无改动返回空串；无提交的新仓库相对空树计算。
pub fn diff(root: &Path) -> Result<String> {
    let repo = open_repo(root)?;
    let head_tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(_) => None,
    };
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);
    opts.show_untracked_content(true);
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            '+' | '-' | ' ' => out.push(line.origin()),
            _ => {}
        }
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(out)
}

/// 提交当前工作区全部改动（git add -A + commit），返回提交 id。
/// 无改动时返回当前 HEAD id（不产生空提交）。
pub fn commit(root: &Path, msg: &str) -> Result<String> {
    let repo = ensure_repo(root)?;
    commit_all(&repo, msg)
}

/// 工作区状态清单，形如 `M src/a.rs`、`A new.txt`、`D old.txt`（按路径排序）。
/// 标记：A 新增 / D 删除 / M 修改 / R 重命名 / T 类型变更（暂存与工作区合并展示）。
pub fn status(root: &Path) -> Result<Vec<String>> {
    let repo = open_repo(root)?;
    let statuses = repo.statuses(Some(&mut status_options()))?;
    let mut entries = Vec::with_capacity(statuses.len());
    for entry in statuses.iter() {
        let s = entry.status();
        let mark = if s.is_index_new() || s.is_wt_new() {
            "A"
        } else if s.is_index_deleted() || s.is_wt_deleted() {
            "D"
        } else if s.is_index_renamed() || s.is_wt_renamed() {
            "R"
        } else if s.is_index_typechange() || s.is_wt_typechange() {
            "T"
        } else {
            "M"
        };
        entries.push(format!("{mark} {}", entry.path().unwrap_or_default()));
    }
    entries.sort();
    Ok(entries)
}

/// 打开仓库；目录不是 Git 仓库时报错（只读类操作不自动 init）。
fn open_repo(root: &Path) -> Result<Repository> {
    Repository::discover(root)
        .map_err(|e| XaimError::Config(format!("不是 Git 仓库或无法定位仓库: {e}")))
}

/// 打开仓库；不存在时自动 init（创建回滚点/提交需要保证仓库可用）。
/// 自动 init 时本地关闭 `core.autocrlf`：XAim 对源码做字节精确的快照与
/// 恢复（Windows 上系统配置常为 true，会把检出的 LF 改写成 CRLF）。
/// 用户已有仓库尊重其现有配置。
fn ensure_repo(root: &Path) -> Result<Repository> {
    match Repository::discover(root) {
        Ok(repo) => Ok(repo),
        Err(_) => {
            std::fs::create_dir_all(root)?;
            let repo = Repository::init(root)?;
            let mut config = repo.config()?;
            config.set_bool("core.autocrlf", false)?;
            Ok(repo)
        }
    }
}

/// 提交身份：仓库配置优先，缺省回退到 XAim 内置身份（测试沙箱无 git 配置）。
fn signature(repo: &Repository) -> Result<Signature<'static>> {
    match repo.signature() {
        Ok(sig) => Ok(sig),
        Err(_) => Ok(Signature::now("XAim", "xaim@localhost")?),
    }
}

/// 状态扫描选项：含未跟踪文件并展开目录，不含被 .gitignore 忽略的文件。
fn status_options() -> StatusOptions {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);
    opts
}

/// git add -A：把全部改动（新增/修改/删除，遵守 .gitignore）写入暂存区。
fn stage_all(repo: &Repository) -> Result<()> {
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

/// 暂存后提交；暂存区与 HEAD 一致时返回 HEAD id（不产生空提交），
/// 无 HEAD 的新仓库产生首个提交。
fn commit_all(repo: &Repository, msg: &str) -> Result<String> {
    stage_all(repo)?;
    if let Ok(head) = repo.head() {
        let head_tree = head.peel_to_tree()?;
        let index = repo.index()?;
        let staged = repo.diff_tree_to_index(Some(&head_tree), Some(&index), None)?;
        if staged.deltas().count() == 0 {
            return Ok(head.peel_to_commit()?.id().to_string());
        }
    }

    let signature = signature(repo)?;
    let mut index = repo.index()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let parents = match repo.head() {
        Ok(head) => vec![head.peel_to_commit()?],
        Err(_) => Vec::new(),
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        msg,
        &tree,
        &parent_refs,
    )?;
    Ok(oid.to_string())
}

/// 清除相对当前 HEAD 的未跟踪文件（任务产物）；.gitignore 忽略的不动。
fn remove_untracked(repo: &Repository) -> Result<()> {
    let statuses = repo.statuses(Some(&mut status_options()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| XaimError::Config("仓库没有工作目录（bare）".into()))?;
    for entry in statuses.iter() {
        if !entry.status().is_wt_new() {
            continue;
        }
        let path = workdir.join(entry.path().unwrap_or_default());
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    fn head_commit_message(root: &Path) -> String {
        let repo = Repository::discover(root).unwrap();
        let message = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .to_string();
        message
    }

    fn head_id(root: &Path) -> String {
        let repo = Repository::discover(root).unwrap();
        let id = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        id
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn is_git_repo_detects_existing_repos_only() {
        let dir = tempdir().unwrap();
        assert!(!is_git_repo(dir.path()));
        Repository::init(dir.path()).unwrap();
        assert!(is_git_repo(dir.path()));
    }

    #[test]
    fn rollback_point_auto_inits_repo_for_plain_dirs() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn a() {}\n");

        let id = create_rollback_point(dir.path(), 1).unwrap();

        assert_eq!(id.len(), 40, "应返回完整 commit id");
        assert!(is_git_repo(dir.path()), "非仓库目录应自动 init");
        assert!(
            head_commit_message(dir.path()).contains("task #1"),
            "回滚点提交应携带 task id"
        );
        assert!(
            status(dir.path()).unwrap().is_empty(),
            "创建回滚点后工作区应干净"
        );
    }

    #[test]
    fn current_branch_reports_branch_and_survives_unborn_head() {
        let dir = tempdir().unwrap();
        Repository::init(dir.path()).unwrap();

        // 未出生 HEAD（尚无提交）也应返回分支名而非报错。
        let unborn = current_branch(dir.path()).unwrap();
        assert!(unborn == "master" || unborn == "main", "{unborn}");

        create_rollback_point(dir.path(), 1).unwrap();
        let branch = current_branch(dir.path()).unwrap();
        assert!(branch == "master" || branch == "main", "{branch}");
    }

    #[test]
    fn rollback_point_captures_uncommitted_and_untracked_state() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        create_rollback_point(dir.path(), 1).unwrap();

        // 用户的未提交修改 + 未跟踪文件。
        write_file(dir.path(), "a.txt", "user edit\n");
        write_file(dir.path(), "notes.txt", "user draft\n");

        let prev_head = head_id(dir.path());
        let rollback_id = create_rollback_point(dir.path(), 2).unwrap();

        assert_ne!(rollback_id, prev_head, "有改动时应产生新提交");
        assert!(head_commit_message(dir.path()).contains("task #2"));
        assert!(
            status(dir.path()).unwrap().is_empty(),
            "未提交内容应被快照进回滚点，而非留在工作区"
        );
    }

    #[test]
    fn rollback_restores_point_state_and_cleans_worktree() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        create_rollback_point(dir.path(), 1).unwrap();
        write_file(dir.path(), "a.txt", "user edit\n");
        write_file(dir.path(), "notes.txt", "user draft\n");
        let rollback_id = create_rollback_point(dir.path(), 2).unwrap();

        // 模拟任务改动：改已有文件 + 产出新文件。
        write_file(dir.path(), "a.txt", "task changed\n");
        write_file(dir.path(), "task_artifact.log", "generated\n");
        assert!(!status(dir.path()).unwrap().is_empty());

        rollback(dir.path(), &rollback_id).unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "user edit\n",
            "应恢复到回滚点状态：用户的未提交修改原样保留"
        );
        assert!(
            dir.path().join("notes.txt").exists(),
            "回滚点内的文件应保留"
        );
        assert!(
            !dir.path().join("task_artifact.log").exists(),
            "任务产出的未跟踪文件应被清除"
        );
        assert!(status(dir.path()).unwrap().is_empty(), "回滚后工作区应干净");
    }

    #[test]
    fn rollback_after_commit_moves_branch_back() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        create_rollback_point(dir.path(), 1).unwrap();
        let rollback_id = create_rollback_point(dir.path(), 2).unwrap();

        write_file(dir.path(), "a.txt", "task changed\n");
        let task_commit = commit(dir.path(), "task done").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "task changed\n"
        );

        rollback(dir.path(), &rollback_id).unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "base\n"
        );
        assert_ne!(
            head_id(dir.path()),
            task_commit,
            "reset --hard 应把分支移回回滚点"
        );
        assert!(status(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn rollback_task_commit_resets_to_task_commit_parent() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        let init = commit(dir.path(), "init").unwrap();

        // 模拟任务确认阶段：改动后以任务格式提交，并产生一个未跟踪产物。
        write_file(dir.path(), "a.txt", "task changed\n");
        write_file(dir.path(), "task_artifact.log", "generated\n");
        commit(dir.path(), "XAim task #7: src/lib.rs").unwrap();

        rollback_task_commit(dir.path(), 7).unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "base\n",
            "应回到任务提交之前的内容"
        );
        assert!(
            !dir.path().join("task_artifact.log").exists(),
            "任务产出的未跟踪文件应被清除"
        );
        assert_eq!(head_id(dir.path()), init, "分支应移回任务提交的父提交");
        assert!(status(dir.path()).unwrap().is_empty(), "回滚后工作区应干净");
    }

    #[test]
    fn rollback_task_commit_reports_missing_or_unrollbackable() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");

        // 无任何提交的仓库。
        let err = rollback_task_commit(dir.path(), 7).unwrap_err();
        assert!(matches!(err, XaimError::Config(_)), "{err:?}");

        // 有提交但没有任务 #7 的补丁提交。
        commit(dir.path(), "init").unwrap();
        let err = rollback_task_commit(dir.path(), 7).unwrap_err();
        assert!(err.to_string().contains("未找到任务 #7"), "{err}");

        // 任务提交是仓库首个提交：没有父提交可回（独立目录，保证首提交形态）。
        let first_only = tempdir().unwrap();
        write_file(first_only.path(), "a.txt", "changed\n");
        commit(first_only.path(), "XAim task #7: a.txt").unwrap();
        let err = rollback_task_commit(first_only.path(), 7).unwrap_err();
        assert!(err.to_string().contains("首个提交"), "{err}");
    }

    #[test]
    fn commit_stages_changes_and_reports_clean_after() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        create_rollback_point(dir.path(), 1).unwrap();

        write_file(dir.path(), "a.txt", "changed\n");
        write_file(dir.path(), "b.txt", "new\n");
        let id = commit(dir.path(), "task done").unwrap();

        assert_eq!(id.len(), 40);
        assert_eq!(head_commit_message(dir.path()), "task done");
        assert!(status(dir.path()).unwrap().is_empty(), "提交后工作区应干净");
    }

    #[test]
    fn commit_without_changes_returns_current_head() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        let first = commit(dir.path(), "init").unwrap();

        let again = commit(dir.path(), "no changes").unwrap();

        assert_eq!(first, again, "无改动不应产生空提交");
        assert_eq!(head_commit_message(dir.path()), "init");
    }

    #[test]
    fn diff_reports_changes_then_becomes_empty_after_commit() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.txt", "base\n");
        create_rollback_point(dir.path(), 1).unwrap();

        assert!(diff(dir.path()).unwrap().is_empty(), "无改动时 diff 应为空");

        write_file(dir.path(), "a.txt", "changed\n");
        write_file(dir.path(), "new.txt", "added\n");
        let text = diff(dir.path()).unwrap();
        assert!(
            text.contains("diff --git"),
            "应输出标准 unified diff: {text}"
        );
        assert!(text.contains("+changed\n"), "{text}");
        assert!(
            text.contains("new.txt") && text.contains("+added"),
            "未跟踪文件应含内容: {text}"
        );

        commit(dir.path(), "done").unwrap();
        assert!(diff(dir.path()).unwrap().is_empty(), "提交后 diff 应为空");
    }

    #[test]
    fn read_only_operations_fail_on_non_repo_dirs() {
        let dir = tempdir().unwrap();
        assert!(matches!(status(dir.path()), Err(_)));
        assert!(matches!(diff(dir.path()), Err(_)));
        assert!(matches!(current_branch(dir.path()), Err(_)));
        assert!(matches!(rollback(dir.path(), "HEAD"), Err(_)));
        // create_rollback_point / commit 会自动 init，不在此列。
    }
}
