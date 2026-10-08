//! 补丁引擎：unified diff 的解析、生成、校验（dry-run）、应用与回滚。
//!
//! 职责：把模型产出的标准 unified diff 安全地落到工作区。
//! - [`parse_diff`] 把可能含多文件段落的补丁文本切成 [`PatchCandidate`]；
//! - [`validate_diff`] 全量 dry-run（内存中试应用），任何一段冲突都不落盘；
//! - [`apply_diff`] 两阶段提交：先全部试应用，再统一写盘；写盘失败自动把
//!   已写入的文件回滚为原内容，不留半应用的补丁；
//! - [`reverse_diff`] 生成反向补丁并应用，用于撤销已落地的补丁；
//! - [`generate_diff`] 由新旧内容生成标准 unified diff（similar）。
//!
//! 应用由 `diffy` 完成：按 hunk 头行号定位后向前后搜索上下文（GNU patch
//! 算法），行号轻微漂移仍可应用；上下文对不上则报 [`XaimError::PatchConflict`]。
//!
//! 安全：diff 内的目标路径必须落在项目根目录内（拒绝绝对路径与 `..` 越界，
//! 已存在目标经 canonicalize 拦截符号链接逃逸）。
//!
//! TODO: 文件级删除/重命名（当前删除补丁只清空内容）；二进制补丁。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use super::write::normalize;
use crate::errors::{Result, XaimError};

/// 一个待应用的补丁候选：目标文件 + 该文件的 unified diff 文本。
///
/// `score` 供上游（如 planner）对多候选排序使用，解析阶段不填（`None`）。
#[derive(Debug, Clone, PartialEq)]
pub struct PatchCandidate {
    pub file_path: String,
    pub diff: String,
    pub score: Option<f64>,
}

/// 从补丁文本解析出各文件的候选段。
///
/// 宽容解析：定位所有 `--- ` 紧跟 `+++ ` 的文件头对，每对切成一个候选；
/// 前导的 `diff --git`/`index` 行与正文两侧的说明文字一律忽略。
/// 找不到任何文件头对时返回空向量（由 apply/validate 报"未找到 diff 块"）。
pub fn parse_diff(text: &str) -> Result<Vec<PatchCandidate>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut minus_lines: Vec<usize> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.starts_with("--- ")
            && lines
                .get(idx + 1)
                .is_some_and(|next| next.starts_with("+++ "))
        {
            minus_lines.push(idx);
        }
    }

    let mut candidates = Vec::new();
    for (k, &minus) in minus_lines.iter().enumerate() {
        // 段终点：下一个文件头对，或之间出现的下一个 `diff --git `（下一节的前导）。
        let next_minus = minus_lines.get(k + 1).copied().unwrap_or(lines.len());
        let next_git = (minus + 1..lines.len()).find(|&i| lines[i].starts_with("diff --git "));
        let boundary_end = next_minus.min(next_git.unwrap_or(lines.len()));
        // 段尾裁剪：hunk 之后可能出现空行、说明文字、代码围栏等杂项，
        // 按 hunk 头声明的行数精确消费，hunk 结束处即段尾。
        let seg_end = boundary_end.min(hunk_content_end(&lines, minus));
        let diff = format!("{}\n", lines[minus..seg_end].join("\n"));
        let file_path = extract_target_path(lines[minus], lines[minus + 1])?;
        candidates.push(PatchCandidate {
            file_path,
            diff,
            score: None,
        });
    }
    Ok(candidates)
}

/// 从 `start` 行起跳过文件头（`---`/`+++`）定位 hunk，按每个 hunk 头声明的
/// 旧行数与新行数精确消费 hunk 体，返回最后一个 hunk 行之后的下标。
/// 头部解析失败时返回 `start`（让 diffy 给出更具体的错误）。
fn hunk_content_end(lines: &[&str], start: usize) -> usize {
    let mut i = start;
    while i < lines.len() && !lines[i].starts_with("@@") {
        i += 1;
    }
    while i < lines.len() && lines[i].starts_with("@@") {
        let Some((mut old_left, mut new_left)) = hunk_line_counts(lines[i]) else {
            return i;
        };
        i += 1;
        while i < lines.len() && (old_left > 0 || new_left > 0) {
            let line = lines[i];
            if line.starts_with('\\') {
                // `\ No newline at end of file` 标记不占行数。
            } else if line.starts_with('-') {
                old_left = old_left.saturating_sub(1);
            } else if line.starts_with('+') {
                new_left = new_left.saturating_sub(1);
            } else {
                // 上下文行（含空行上下文）。
                old_left = old_left.saturating_sub(1);
                new_left = new_left.saturating_sub(1);
            }
            i += 1;
        }
    }
    i
}

/// 从 hunk 头提取 (旧行数, 新行数)；缺省计数按 1 处理（如 `@@ -5 +5 @@`）。
fn hunk_line_counts(header: &str) -> Option<(u32, u32)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))?").unwrap());
    let caps = re.captures(header)?;
    Some((
        caps.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(1)),
        caps.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1)),
    ))
}

/// 全量 dry-run：所有候选在内存中试应用一遍，不写任何文件。
/// 任一段解析失败、目标缺失或上下文冲突即返回 [`XaimError::PatchConflict`]。
pub fn validate_diff(diff: &str, root: &Path) -> Result<()> {
    plan_apply(diff, root).map(|_| ())
}

/// 应用补丁到工作区（两阶段提交）。
///
/// 先对所有文件段做内存试应用（dry-run），全部通过后统一写盘；写盘阶段
/// 失败时自动把已写入的文件回滚为原内容，保证不留下半应用的补丁。
pub fn apply_diff(diff: &str, root: &Path) -> Result<()> {
    let planned = plan_apply(diff, root)?;
    let mut written: Vec<&PlannedChange> = Vec::new();
    for change in &planned {
        if let Err(e) = std::fs::write(&change.target, &change.patched) {
            let rollback = rollback(&written);
            return Err(XaimError::Unknown(format!(
                "写入 {} 失败: {e}；{rollback}",
                change.target.display()
            )));
        }
        written.push(change);
    }
    Ok(())
}

/// 应用反向补丁，撤销一次已落地的补丁（工作区当前应处于补丁应用后的状态）。
/// 与 [`apply_diff`] 相同的两阶段提交与回滚保证。
pub fn reverse_diff(diff: &str, root: &Path) -> Result<()> {
    let candidates = parse_diff(diff)?;
    if candidates.is_empty() {
        return Err(XaimError::PatchConflict("补丁中未找到 diff 块".into()));
    }
    let mut reversed = String::new();
    for cand in &candidates {
        reversed.push_str(&reverse_patch_text(&cand.diff)?);
        reversed.push('\n');
    }
    apply_diff(&reversed, root)
}

/// 由新旧内容生成标准 unified diff（`a/{path}` / `b/{path}` 头，3 行上下文）。
/// 内容相同返回空串。
pub fn generate_diff(old: &str, new: &str, path: &str) -> String {
    if old == new {
        return String::new();
    }
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// 一段补丁的计划结果：目标路径、应用前的原内容与试应用后的新内容。
struct PlannedChange {
    target: PathBuf,
    original: String,
    patched: String,
}

/// 两阶段的公共执行体：解析 → 逐段定位目标并读取原内容 → 内存试应用。
/// 任一步失败即报错，保证不产生任何落盘副作用。
fn plan_apply(diff: &str, root: &Path) -> Result<Vec<PlannedChange>> {
    let candidates = parse_diff(diff)?;
    if candidates.is_empty() {
        return Err(XaimError::PatchConflict("补丁中未找到 diff 块".into()));
    }
    let mut planned = Vec::with_capacity(candidates.len());
    for cand in &candidates {
        let target = resolve_in_root(root, &cand.file_path)?;
        let original = read_current(&target, cand)?;
        let patched = apply_patch_text(&original, &cand.diff, &cand.file_path)?;
        planned.push(PlannedChange {
            target,
            original,
            patched,
        });
    }
    Ok(planned)
}

/// 把已写入的文件依次恢复为原内容，返回结果描述（供错误信息拼接）。
fn rollback(written: &[&PlannedChange]) -> String {
    if written.is_empty() {
        return "无需回滚".into();
    }
    let mut failures = Vec::new();
    for change in written.iter().rev() {
        if let Err(e) = std::fs::write(&change.target, &change.original) {
            failures.push(format!("{} 回滚失败: {e}", change.target.display()));
        }
    }
    if failures.is_empty() {
        format!("已回滚 {} 个文件", written.len())
    } else {
        format!("回滚失败: {}", failures.join("; "))
    }
}

/// 解析 diff 内的目标路径并做项目内校验：拒绝绝对路径；
/// 已存在文件经 canonicalize 检查（顺带拦截符号链接逃逸），
/// 尚不存在的新文件退回词法前缀比对。
pub(crate) fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf> {
    if Path::new(rel).is_absolute() {
        return Err(XaimError::Security(format!(
            "diff 路径不允许是绝对路径: {rel}"
        )));
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let candidate = normalize(&root.join(rel));
    match std::fs::canonicalize(&candidate) {
        Ok(real) => {
            if !real.starts_with(&canonical_root) {
                return Err(XaimError::Security(format!("diff 路径越出项目目录: {rel}")));
            }
            Ok(real)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !candidate.starts_with(normalize(root)) {
                return Err(XaimError::Security(format!("diff 路径越出项目目录: {rel}")));
            }
            Ok(candidate)
        }
        Err(e) => Err(e.into()),
    }
}

/// 读取目标文件当前内容；`--- /dev/null` 的新文件补丁允许目标不存在（视为空）。
fn read_current(target: &Path, cand: &PatchCandidate) -> Result<String> {
    match std::fs::read_to_string(target) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(XaimError::PatchConflict(
            format!("{}: 目标文件不是 UTF-8 文本", cand.file_path),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if is_new_file_patch(&cand.diff) {
                Ok(String::new())
            } else {
                Err(XaimError::PatchConflict(format!(
                    "{}: 目标文件不存在",
                    cand.file_path
                )))
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn is_new_file_patch(diff_text: &str) -> bool {
    diff_text
        .lines()
        .any(|line| line.starts_with("--- /dev/null"))
}

/// 用 diffy 在内存中把补丁应用到 `original`，返回新内容（不落盘）。
fn apply_patch_text(original: &str, diff_text: &str, rel: &str) -> Result<String> {
    let patch = diffy::Patch::from_str(diff_text)
        .map_err(|e| XaimError::PatchConflict(format!("{rel}: diff 解析失败: {e}")))?;
    diffy::apply(original, &patch)
        .map_err(|e| XaimError::PatchConflict(format!("{rel}: 与工作区不匹配，无法干净应用: {e}")))
}

/// 从 `--- `/`+++ ` 行对提取目标文件路径：剥离时间戳（tab 之后）与 `a/`、
/// `b/` 前缀；`/dev/null` 一侧为 None（新建/删除），回退取另一侧。
fn extract_target_path(minus_line: &str, plus_line: &str) -> Result<String> {
    match header_path(plus_line).or_else(|| header_path(minus_line)) {
        Some(path) => Ok(path),
        None => Err(XaimError::Unknown(format!(
            "无法从 diff 中解析目标文件路径: {minus_line} / {plus_line}"
        ))),
    }
}

fn header_path(line: &str) -> Option<String> {
    let raw = line
        .strip_prefix("--- ")
        .or_else(|| line.strip_prefix("+++ "))?;
    let path = raw.split('\t').next()?.trim();
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    let path = path
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path);
    let path = path
        .strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path);
    Some(path.to_string())
}

/// 原地交换补丁的新旧两侧：`+`/`-` 换角色、上下文原样、头两个文件名互换
/// （保持 `---` 在前），hunk 头两组行号互换；`\ No newline` 标记跟随其行，
/// 换角色后语义仍成立。
fn reverse_patch_text(diff_text: &str) -> Result<String> {
    let mut out = String::new();
    let mut pending_minus: Option<String> = None;
    for line in diff_text.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            pending_minus = Some(format!("+++ {rest}"));
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let swapped_minus = pending_minus
                .take()
                .ok_or_else(|| XaimError::Unknown("diff 缺少 --- 头，无法反转".into()))?;
            out.push_str(&format!("--- {rest}\n"));
            out.push_str(&swapped_minus);
            out.push('\n');
        } else if line.starts_with("@@") {
            out.push_str(&reverse_hunk_header(line)?);
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix('-') {
            out.push('+');
            out.push_str(rest);
            out.push('\n');
        } else if let Some(rest) = line.strip_prefix('+') {
            out.push('-');
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// 交换 hunk 头两组行号：反转后的旧侧 = 原新侧，新侧 = 原旧侧
/// （缺省计数按 1 处理，输出补全为显式计数）。
fn reverse_hunk_header(line: &str) -> Result<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$").unwrap());
    let caps = re
        .captures(line)
        .ok_or_else(|| XaimError::Unknown(format!("无法解析 hunk 头: {line}")))?;
    let old_start: u32 = caps[1].parse().unwrap_or(0);
    let old_count: u32 = caps.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let new_start: u32 = caps[3].parse().unwrap_or(0);
    let new_count: u32 = caps.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let tail = caps.get(5).map_or("", |m| m.as_str());
    Ok(format!(
        "@@ -{new_start},{new_count} +{old_start},{old_count} @@{tail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    const OLD: &str = "fn main() {\n    println!(\"hello\");\n}\n";
    const NEW: &str = "fn main() {\n    println!(\"hello\");\n    println!(\"world\");\n}\n";

    /// 在 `root` 下创建相对路径 `rel` 的文件（自动补建父目录）。
    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn generate_produces_standard_unified_diff() {
        let diff = generate_diff(OLD, NEW, "src/main.rs");
        assert!(
            diff.starts_with("--- a/src/main.rs\n+++ b/src/main.rs\n"),
            "{diff}"
        );
        assert!(diff.contains("@@ -1,3 +1,4 @@"), "{diff}");
        assert!(diff.contains("+    println!(\"world\");"), "{diff}");
        assert_eq!(generate_diff(OLD, OLD, "x.rs"), "", "内容相同应返回空串");
    }

    #[test]
    fn parse_extracts_path_from_generated_diff() {
        let cands = parse_diff(&generate_diff(OLD, NEW, "src/main.rs")).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].file_path, "src/main.rs");
        assert_eq!(cands[0].score, None);
        // 段落应被 diffy 原样解析（无残留空行或前导行）。
        assert!(diffy::Patch::from_str(&cands[0].diff).is_ok());
    }

    #[test]
    fn parse_splits_git_style_multi_file_diff_with_prose() {
        let d1 = generate_diff(OLD, NEW, "a_one.rs");
        let d2 = generate_diff(NEW, OLD, "b_two.rs");
        let combined = format!(
            "下面是修改说明。\n```diff\ndiff --git a/a_one.rs b/a_one.rs\nindex 111111..222222 100644\n{}diff --git a/b_two.rs b/b_two.rs\nindex 333333..444444 100644\n{}\n```\n以上。",
            d1, d2
        );

        let cands = parse_diff(&combined).unwrap();
        assert_eq!(cands.len(), 2, "{combined}");
        assert_eq!(cands[0].file_path, "a_one.rs");
        assert_eq!(cands[1].file_path, "b_two.rs");
        // 每段独立可解析：不应混入下一节的 diff --git 前导或尾部空行。
        assert!(
            diffy::Patch::from_str(&cands[0].diff).is_ok(),
            "{}",
            cands[0].diff
        );
        assert!(
            diffy::Patch::from_str(&cands[1].diff).is_ok(),
            "{}",
            cands[1].diff
        );
    }

    #[test]
    fn parse_handles_dev_null_and_timestamps() {
        let new_file =
            "--- /dev/null\n+++ b/new.rs\t2026-01-01 00:00:00\n@@ -0,0 +1,2 @@\n+alpha\n+beta\n";
        let cands = parse_diff(new_file).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(
            cands[0].file_path, "new.rs",
            "时间戳应剥离、/dev/null 回退到 +++ 侧"
        );

        let deletion = "--- a/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-alpha\n-beta\n";
        let cands = parse_diff(deletion).unwrap();
        assert_eq!(cands[0].file_path, "old.rs", "删除补丁回退到 --- 侧取路径");
    }

    #[test]
    fn parse_ignores_non_diff_text() {
        assert!(parse_diff("这只是说明文字，没有任何 diff 块。")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn validate_passes_on_matching_workspace_and_rejects_conflict() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/main.rs", OLD);
        let diff = generate_diff(OLD, NEW, "src/main.rs");

        assert!(
            validate_diff(&diff, dir.path()).is_ok(),
            "匹配的工作区应通过 dry-run"
        );

        write_file(dir.path(), "src/main.rs", "已经完全不同的内容\n");
        let err = validate_diff(&diff, dir.path()).unwrap_err();
        assert!(
            matches!(err, XaimError::PatchConflict(_)),
            "上下文冲突应报 PatchConflict"
        );

        let empty_dir = tempdir().unwrap();
        let err = validate_diff(&diff, empty_dir.path()).unwrap_err();
        assert!(
            matches!(err, XaimError::PatchConflict(_)),
            "目标缺失应报 PatchConflict"
        );

        let err = validate_diff("没有任何 diff 块", dir.path()).unwrap_err();
        assert!(matches!(err, XaimError::PatchConflict(_)), "空补丁应报错");
    }

    #[test]
    fn validate_rejects_path_escape_and_absolute_paths() {
        let dir = tempdir().unwrap();
        let traversal = "--- a/../evil.txt\n+++ b/../evil.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let err = validate_diff(traversal, dir.path()).unwrap_err();
        assert!(matches!(err, XaimError::Security(_)), "父目录穿越应被拒绝");

        let outside = tempdir().unwrap();
        let abs = outside.path().join("evil.txt").display().to_string();
        let absolute = format!("--- {abs}\n+++ {abs}\n@@ -1 +1 @@\n-x\n+y\n");
        let err = validate_diff(&absolute, dir.path()).unwrap_err();
        assert!(matches!(err, XaimError::Security(_)), "绝对路径应被拒绝");
    }

    #[test]
    fn apply_writes_workspace() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/main.rs", OLD);

        apply_diff(&generate_diff(OLD, NEW, "src/main.rs"), dir.path()).unwrap();

        let written = fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
        assert_eq!(written, NEW);
    }

    #[test]
    fn apply_supports_multiple_files() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.rs", OLD);
        write_file(dir.path(), "src/b.rs", OLD);
        let combined = format!(
            "{}\n{}",
            generate_diff(OLD, NEW, "a.rs"),
            generate_diff(OLD, NEW, "src/b.rs")
        );

        apply_diff(&combined, dir.path()).unwrap();

        assert_eq!(fs::read_to_string(dir.path().join("a.rs")).unwrap(), NEW);
        assert_eq!(
            fs::read_to_string(dir.path().join("src/b.rs")).unwrap(),
            NEW
        );
    }

    #[test]
    fn apply_is_atomic_when_any_candidate_conflicts() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.rs", OLD);
        write_file(dir.path(), "b.rs", OLD);

        // b.rs 的补丁以 NEW 为基准，而工作区仍是 OLD：计划阶段即冲突。
        let combined = format!(
            "{}\n{}",
            generate_diff(OLD, NEW, "a.rs"),
            generate_diff(NEW, OLD, "b.rs")
        );
        let err = apply_diff(&combined, dir.path()).unwrap_err();
        assert!(matches!(err, XaimError::PatchConflict(_)));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            OLD,
            "任一段冲突时整个补丁都不应落盘"
        );
    }

    #[test]
    fn apply_rolls_back_completed_writes_on_failure() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "a.rs", OLD);
        write_file(dir.path(), "b.rs", OLD);

        // 把 b.rs 设为只读，制造写盘阶段失败（计划阶段的读取不受影响）。
        let b = dir.path().join("b.rs");
        let mut perms = fs::metadata(&b).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&b, perms).unwrap();

        let combined = format!(
            "{}\n{}",
            generate_diff(OLD, NEW, "a.rs"),
            generate_diff(OLD, NEW, "b.rs")
        );
        let err = apply_diff(&combined, dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, XaimError::Unknown(_)), "{msg}");
        assert!(msg.contains("回滚"), "错误信息应说明回滚结果: {msg}");
        assert_eq!(
            fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            OLD,
            "已写入的 a.rs 应被自动回滚"
        );

        // 恢复可写，避免 TempDir 清理失败。
        let mut perms = fs::metadata(&b).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(&b, perms).unwrap();
    }

    #[test]
    fn reverse_restores_original_content() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/main.rs", OLD);
        let diff = generate_diff(OLD, NEW, "src/main.rs");

        apply_diff(&diff, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            NEW
        );

        reverse_diff(&diff, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            OLD,
            "反向应用应恢复原内容"
        );

        // reverse_diff 不是开关：对已恢复的原内容再打同一反向补丁必然冲突。
        let err = reverse_diff(&diff, dir.path()).unwrap_err();
        assert!(matches!(err, XaimError::PatchConflict(_)));

        // 重新前向应用则回到 NEW。
        apply_diff(&diff, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            NEW
        );
    }

    #[test]
    fn reverse_supports_created_files() {
        let dir = tempdir().unwrap();
        let create = "--- /dev/null\n+++ b/created.rs\n@@ -0,0 +1,2 @@\n+alpha\n+beta\n";

        apply_diff(create, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("created.rs")).unwrap(),
            "alpha\nbeta\n"
        );

        reverse_diff(create, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("created.rs")).unwrap(),
            "",
            "反向后内容应清空（文件级删除为 TODO）"
        );
    }

    #[test]
    fn reverse_patch_text_swaps_sides_and_headers() {
        let diff = "--- a/x.rs\n+++ b/x.rs\n@@ -1,4 +1,3 @@\n ctx\n-del\n-del2\n+add\n keep\n";
        let reversed = reverse_patch_text(diff).unwrap();

        assert!(
            reversed.starts_with("--- b/x.rs\n+++ a/x.rs\n"),
            "{reversed}"
        );
        assert!(
            reversed.contains("@@ -1,3 +1,4 @@"),
            "行号应互换: {reversed}"
        );
        assert!(
            reversed.contains("+del\n") && reversed.contains("+del2\n"),
            "{reversed}"
        );
        assert!(reversed.contains("-add\n"), "{reversed}");
        assert!(
            reversed.contains(" ctx\n") && reversed.contains(" keep\n"),
            "上下文应原样保留"
        );
    }

    #[test]
    fn reverse_patch_text_keeps_no_newline_marker_attached() {
        let diff =
            "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n-a\n\\ No newline at end of file\n+b\n+a2\n";
        let reversed = reverse_patch_text(diff).unwrap();
        assert!(
            reversed.contains("+a\n\\ No newline at end of file\n"),
            "标记应跟随换角色后的行: {reversed}"
        );
    }
}
