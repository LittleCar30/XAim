//! diff 工具。TODO: 基于 similar / imara-diff 生成与应用。

/// 统一 diff 的一个 hunk。TODO: 补充上下文行模型。
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<String>,
}

/// TODO: 生成两段文本的 unified diff。
pub fn unified(_old: &str, _new: &str) -> String {
    todo!("utils::diff::unified")
}
