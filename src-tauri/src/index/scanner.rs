//! 项目文件扫描：递归遍历项目目录，生成前端文件树（[`FileNode`]）。
//!
//! 行为要点：
//! - 通过 `ignore` crate 读取项目内各级 `.gitignore`（不要求项目在 git 仓库内，
//!   见 [`scan_project`] 中的 `require_git(false)`）；
//! - 在 .gitignore 之上叠加内置默认忽略表 [`DEFAULT_IGNORES`] 与调用方传入的
//!   附加忽略模式（gitignore 语法，如 `"logs"`、`"*.log"`、`"!keep.log"`）；
//! - 跳过敏感文件（`.env`/`.env.*`/`*.key`/`*.pem`）、超过大小上限的文件
//!   （默认 1 MiB，可用 [`scan_with_limit`] 调整）以及二进制文件；
//! - 输出按名称排序、目录优先；节点 `path` 为相对项目根、以 `/` 分隔的路径，
//!   根节点 `path` 为 `"."`。
//!
//! 同步入口为 [`scan_project`]；UI 层应使用 [`scan_project_async`]，
//! 它把遍历放到 `tokio::task::spawn_blocking` 的阻塞线程池，避免卡住界面。

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{DirEntry, WalkBuilder};

use crate::errors::{Result, XaimError};
use crate::types::FileNode;

/// 内置默认忽略的目录/文件名，任意层级生效。
pub const DEFAULT_IGNORES: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "target",
    ".next",
    ".cache",
];

/// 默认单文件大小上限：1 MiB。超过该大小的文件不进入文件树。
pub const DEFAULT_MAX_FILE_SIZE: u64 = 1024 * 1024;

/// 二进制嗅探读取的字节数：头部出现 NUL 字节即判定为二进制。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 同步扫描 `root`，返回以它为根的文件树。`ignore` 为附加忽略模式。
pub fn scan_project(root: &Path, ignore: &[String]) -> Result<FileNode> {
    scan_with_limit(root, ignore, DEFAULT_MAX_FILE_SIZE)
}

/// [`scan_project`] 的可配置版本：`max_file_size` 为单文件大小上限（字节）。
pub fn scan_with_limit(root: &Path, ignore: &[String], max_file_size: u64) -> Result<FileNode> {
    if !root.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("项目根目录不存在或不是目录: {}", root.display()),
        )
        .into());
    }

    let defaults = build_matcher(root, DEFAULT_IGNORES)?;
    let user_patterns: Vec<&str> = ignore.iter().map(String::as_str).collect();
    let user = build_matcher(root, &user_patterns)?;

    let root_owned = root.to_path_buf();
    let walker = WalkBuilder::new(root)
        .hidden(false) // 保留 .github/.gitignore 等隐藏项，.git 由默认忽略表过滤
        .require_git(false) // 非 git 仓库也遵守 .gitignore
        .git_global(false) // 不读用户全局 gitignore，结果只取决于项目自身
        .parents(false) // 只应用项目内的 .gitignore，不外溢到父目录
        .filter_entry(move |entry| {
            if entry.path() == root_owned.as_path() {
                return true; // 根节点始终保留，否则整棵树为空
            }
            let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
            let rel = entry
                .path()
                .strip_prefix(&root_owned)
                .unwrap_or(entry.path());
            !defaults.matched(rel, is_dir).is_ignore() && !user.matched(rel, is_dir).is_ignore()
        })
        .build();

    build_tree(root, walker, max_file_size)
}

/// 异步扫描：把同步遍历放到阻塞线程池，避免占用 tokio worker / 卡住 UI。
pub async fn scan_project_async(root: PathBuf, ignore: Vec<String>) -> Result<FileNode> {
    tokio::task::spawn_blocking(move || scan_project(&root, &ignore))
        .await
        .map_err(|err| XaimError::Unknown(format!("扫描线程异常退出: {err}")))?
}

/// 用 gitignore 语法的模式列表构建匹配器，模式相对 `root` 生效。
fn build_matcher(root: &Path, patterns: &[&str]) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for pat in patterns {
        builder
            .add_line(None, pat)
            .map_err(|err| XaimError::Config(format!("忽略模式 `{pat}` 非法: {err}")))?;
    }
    builder
        .build()
        .map_err(|err| XaimError::Config(format!("忽略模式构建失败: {err}")))
}

/// 把 DFS 先序遍历的条目流折叠成一棵树：栈中保存各层尚未收口的目录节点，
/// 遇到更浅的条目时把更深的目录帧弹回各自的父节点。
fn build_tree<I>(root: &Path, entries: I, max_file_size: u64) -> Result<FileNode>
where
    I: Iterator<Item = std::result::Result<DirEntry, ignore::Error>>,
{
    let mut stack: Vec<(usize, FileNode)> = Vec::new();
    let mut root_node: Option<FileNode> = None;

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::debug!(%err, "扫描跳过不可读条目");
                continue;
            }
        };
        let depth = entry.depth();

        // 收口所有深度不小于当前条目的目录帧，挂回各自的父节点。
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            let (_, node) = stack.pop().expect("栈非空才可弹出");
            attach_child(&mut stack, &mut root_node, node);
        }

        if depth == 0 {
            stack.push((
                0,
                FileNode {
                    name: root_display_name(root),
                    path: ".".to_string(),
                    is_dir: true,
                    size: None,
                    children: Vec::new(),
                },
            ));
            continue;
        }

        let Some(node) = entry_node(&entry, root, max_file_size) else {
            continue;
        };
        if node.is_dir {
            stack.push((depth, node));
        } else {
            attach_child(&mut stack, &mut root_node, node);
        }
    }

    // 遍历结束，把仍未收口的目录逐层挂回，最后剩下的就是根节点。
    while let Some((_, node)) = stack.pop() {
        attach_child(&mut stack, &mut root_node, node);
    }

    let mut tree = root_node
        .ok_or_else(|| XaimError::Unknown(format!("扫描未产出根节点: {}", root.display())))?;
    sort_tree(&mut tree);
    Ok(tree)
}

/// 把节点挂到栈顶目录的 children 下；栈为空说明它就是根节点。
fn attach_child(stack: &mut [(usize, FileNode)], root_slot: &mut Option<FileNode>, node: FileNode) {
    match stack.last_mut() {
        Some((_, parent)) => parent.children.push(node),
        None => *root_slot = Some(node),
    }
}

/// 为单个条目构造树节点；返回 `None` 表示按规则跳过
/// （敏感文件、超限文件、二进制文件或元数据不可读）。
fn entry_node(entry: &DirEntry, root: &Path, max_file_size: u64) -> Option<FileNode> {
    let name = entry.file_name().to_string_lossy().into_owned();
    let path = rel_path(root, entry.path());
    if is_sensitive(&name) {
        return None;
    }
    if entry.file_type().is_some_and(|ft| ft.is_dir()) {
        return Some(FileNode {
            name,
            path,
            is_dir: true,
            size: None,
            children: Vec::new(),
        });
    }

    let meta = entry.metadata().ok()?;
    if meta.len() > max_file_size || looks_binary(entry.path()) {
        return None;
    }
    Some(FileNode {
        name,
        path,
        is_dir: false,
        size: Some(meta.len()),
        children: Vec::new(),
    })
}

/// 敏感文件判定：`.env`/`.env.*`/`*.key`/`*.pem`（不区分大小写）。
/// 出于安全考虑，规则同样作用于同名目录。
pub(crate) fn is_sensitive(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
}

/// NUL 字节嗅探判定二进制：读文件头部，出现 `\0` 即视为二进制；
/// 打不开或读不出的文件同样按二进制跳过。
fn looks_binary(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return true,
    };
    let mut buf = [0u8; BINARY_SNIFF_BYTES];
    match file.read(&mut buf) {
        Ok(n) => buf[..n].contains(&0),
        Err(_) => true,
    }
}

/// 相对项目根的路径，统一以 `/` 分隔，供前端跨平台使用。
pub(crate) fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 根节点显示名；根路径没有末段（如盘符 `D:\`）时退回完整路径。
fn root_display_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// 就地排序：每层 children 目录优先，再按名称不区分大小写比较。
fn sort_tree(node: &mut FileNode) {
    node.children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for child in &mut node.children {
        sort_tree(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    /// 在 `root` 下创建相对路径 `rel` 的文件（自动补建父目录）。
    fn write_file(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 按名称查找直接子节点。
    fn child<'a>(node: &'a FileNode, name: &str) -> Option<&'a FileNode> {
        node.children.iter().find(|c| c.name == name)
    }

    fn count_files(node: &FileNode) -> usize {
        node.children
            .iter()
            .map(|c| if c.is_dir { count_files(c) } else { 1 })
            .sum()
    }

    #[test]
    fn size_limit_is_inclusive_and_zero_keeps_empty_files() {
        let tmp = tempdir().unwrap();
        write_file(tmp.path(), "empty.txt", b"");
        write_file(tmp.path(), "exact.txt", b"1234");
        write_file(tmp.path(), "over.txt", b"12345");
        let tree = scan_with_limit(tmp.path(), &[], 4).unwrap();
        assert_eq!(child(&tree, "exact.txt").unwrap().size, Some(4));
        assert!(child(&tree, "over.txt").is_none());
        let tree = scan_with_limit(tmp.path(), &[], 0).unwrap();
        assert_eq!(count_files(&tree), 1);
        assert_eq!(child(&tree, "empty.txt").unwrap().size, Some(0));
    }

    #[test]
    fn builds_sorted_tree_dirs_first() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "apple.txt", b"a");
        write_file(root, "zebra.txt", b"zzzz");
        write_file(root, "src/deep/mod.rs", b"fn main() {}");
        write_file(root, "docs/readme.md", b"# doc");
        fs::create_dir_all(root.join("empty_dir")).unwrap();

        let tree = scan_project(root, &[]).unwrap();

        assert_eq!(tree.name, root.file_name().unwrap().to_string_lossy());
        assert_eq!(tree.path, ".");
        assert!(tree.is_dir);
        assert_eq!(tree.size, None);

        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["docs", "empty_dir", "src", "apple.txt", "zebra.txt"]
        );

        let src = child(&tree, "src").unwrap();
        assert!(src.is_dir);
        let deep = child(src, "deep").unwrap();
        let mod_rs = child(deep, "mod.rs").unwrap();
        assert_eq!(mod_rs.path, "src/deep/mod.rs");
        assert_eq!(mod_rs.size, Some(12));
        assert!(mod_rs.children.is_empty());

        assert_eq!(child(&tree, "zebra.txt").unwrap().size, Some(4));
    }

    #[test]
    fn respects_gitignore_even_without_git_repo() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, ".gitignore", b"generated/\n*.tmp\n!keep.tmp\n");
        write_file(root, "generated/out.txt", b"x");
        write_file(root, "junk.tmp", b"x");
        write_file(root, "keep.tmp", b"x");
        write_file(root, "real.txt", b"x");

        let tree = scan_project(root, &[]).unwrap();

        assert!(child(&tree, "generated").is_none());
        assert!(child(&tree, "junk.tmp").is_none());
        assert!(
            child(&tree, "keep.tmp").is_some(),
            "白名单模式 !keep.tmp 应生效"
        );
        assert!(child(&tree, "real.txt").is_some());
        assert!(
            child(&tree, ".gitignore").is_some(),
            "隐藏文件应保留（.git 除外）"
        );
    }

    #[test]
    fn applies_default_ignores_at_any_depth() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for rel in [
            "node_modules/pkg/index.js",
            "src/node_modules/left-pad/index.js",
            ".git/HEAD",
            "target/debug/app.exe",
            "dist/bundle.js",
            "build/out.o",
            ".next/cache.js",
            ".cache/data",
            "src/main.ts",
        ] {
            write_file(root, rel, b"x");
        }

        let tree = scan_project(root, &[]).unwrap();

        for absent in [
            "node_modules",
            ".git",
            "target",
            "dist",
            "build",
            ".next",
            ".cache",
        ] {
            assert!(child(&tree, absent).is_none(), "{absent} 应被默认忽略");
        }
        let src = child(&tree, "src").unwrap();
        assert!(
            child(src, "node_modules").is_none(),
            "嵌套的默认忽略目录同样生效"
        );
        assert_eq!(child(src, "main.ts").unwrap().path, "src/main.ts");
    }

    #[test]
    fn skips_sensitive_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for rel in [".env", ".env.local", "server.key", "cert.pem", "API.KEY"] {
            write_file(root, rel, b"secret");
        }
        write_file(root, "env.txt", b"ok");
        write_file(root, "keynotes.md", b"ok");

        let tree = scan_project(root, &[]).unwrap();
        for absent in [".env", ".env.local", "server.key", "cert.pem", "API.KEY"] {
            assert!(child(&tree, absent).is_none(), "{absent} 不应出现在文件树");
        }
        assert!(child(&tree, "env.txt").is_some());
        assert!(child(&tree, "keynotes.md").is_some());
    }

    #[test]
    fn skips_binary_files_by_content_not_name() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "blob.bin", &[0x00, b'a', b'b']);
        write_file(root, "text.bin", b"plain text");
        write_file(root, "main.rs", b"fn main() {}\n");

        let tree = scan_project(root, &[]).unwrap();
        assert!(
            child(&tree, "blob.bin").is_none(),
            "含 NUL 字节的文件应视为二进制"
        );
        assert!(
            child(&tree, "text.bin").is_some(),
            "文本内容不因扩展名被误判"
        );
        assert!(child(&tree, "main.rs").is_some());
    }

    #[test]
    fn skips_files_over_size_limit_but_limit_is_configurable() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let big = "a".repeat(1024 * 1024 + 1);
        write_file(root, "big.txt", big.as_bytes());
        write_file(root, "small.txt", b"ok");

        let tree = scan_project(root, &[]).unwrap();
        assert!(
            child(&tree, "big.txt").is_none(),
            "超过默认 1MiB 的文件应被跳过"
        );
        assert!(child(&tree, "small.txt").is_some());

        let tree = scan_with_limit(root, &[], 2 * 1024 * 1024).unwrap();
        assert_eq!(child(&tree, "big.txt").unwrap().size, Some(1024 * 1024 + 1));
    }

    #[test]
    fn honors_extra_ignore_patterns() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "logs/app.log", b"x");
        write_file(root, "logs/notes.txt", b"x");
        write_file(root, "app.log", b"x");
        write_file(root, "src/main.rs", b"x");

        let tree = scan_project(root, &["logs".to_string(), "*.log".to_string()]).unwrap();
        assert!(child(&tree, "logs").is_none());
        assert!(child(&tree, "app.log").is_none());
        assert!(child(&tree, "src").is_some());
    }

    #[test]
    fn rejects_missing_root() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = scan_project(&missing, &[]).unwrap_err();
        assert!(matches!(err, XaimError::Io(_)));
    }

    #[test]
    fn rejects_illegal_ignore_pattern() {
        let tmp = tempdir().unwrap();
        let err = scan_project(tmp.path(), &["{a".to_string()]).unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "非法模式应返回 Config 错误"
        );
    }

    #[tokio::test]
    async fn async_scan_matches_sync_result() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "src/main.rs", b"fn main() {}");
        write_file(root, "readme.md", b"# hi");

        let sync = scan_project(root, &[]).unwrap();
        let asynced = scan_project_async(root.to_path_buf(), Vec::new())
            .await
            .unwrap();
        assert_eq!(asynced, sync);
    }

    #[test]
    fn medium_project_scans_quickly() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for d in 0..40u32 {
            for f in 0..25u32 {
                write_file(
                    root,
                    &format!("mod_{d:02}/file_{f:02}.txt"),
                    b"hello world\n",
                );
            }
        }

        let start = Instant::now();
        let tree = scan_project(root, &[]).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(3),
            "中型项目扫描耗时 {elapsed:?}，应 < 3s"
        );
        assert_eq!(count_files(&tree), 40 * 25);
    }
}
