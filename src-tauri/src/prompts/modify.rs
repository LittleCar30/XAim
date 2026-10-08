//! 代码修改提示词：任务 + 计划 + 上下文 -> unified diff。

use crate::search::context::Context;

/// 系统提示词（输出标准 unified diff，不带任何解释与围栏）。
pub const SYSTEM: &str = r#"你是 XAim，一个严谨的编程 Agent。
根据任务和上下文生成 unified diff。

规则：
1. 只修改必要文件
2. 输出标准 unified diff 格式
3. 不修改无关代码
4. 修改已有文件时必须基于上下文；任务要求新建文件时，允许创建计划中列出的项目相对路径
5. 保留原有代码风格
6. 每个修改引用文件路径和行号
7. 不输出解释，不输出 markdown 代码块标记
8. 新文件使用 --- /dev/null 和 +++ b/相对路径，hunk 从 @@ -0,0 +1,N @@ 开始；每行内容前加 +，N 必须等于新增行数
9. 不直接输出 HTML、SVG 或其他文件正文；即使项目为空，也必须输出新文件的 unified diff

新文件示例：
--- /dev/null
+++ b/index.html
@@ -0,0 +1,3 @@
+<!doctype html>
+<html lang="zh-CN">
+</html>

输出格式：
--- a/path/to/file
+++ b/path/to/file
@@ -10,3 +10,4 @@
 context
-old line
+new line
+new line 2
 context
"#;

/// 渲染用户消息：任务 + 计划（[`crate::agent::planner::format_plan`] 的产物）
/// + 上下文摘要（[`super::plan::format_context`]）。
pub fn build(task: &str, plan: &str, context: &Context) -> String {
    format!(
        "任务：{task}\n\n计划：\n{plan}\n\n上下文：{}\n\n请生成 unified diff：",
        super::plan::format_context(context)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::context::ContextFile;
    use crate::tools::patch::{apply_diff, parse_diff, validate_diff};

    /// 用 (路径, 片段) 列表构造最小 Context。
    fn context(files: &[(&str, &str)]) -> Context {
        Context {
            files: files
                .iter()
                .map(|(path, content)| ContextFile {
                    path: path.to_string(),
                    content: content.to_string(),
                    reason: String::new(),
                    tokens: 0,
                })
                .collect(),
            total_tokens: 0,
        }
    }

    #[test]
    fn system_prompt_states_diff_contract() {
        for rule in [
            "只修改必要文件",
            "标准 unified diff",
            "不修改无关代码",
            "允许创建计划中列出的项目相对路径",
            "保留原有代码风格",
            "文件路径和行号",
            "不输出解释",
            "不输出 markdown 代码块标记",
            "--- a/path/to/file",
            "@@",
        ] {
            assert!(SYSTEM.contains(rule), "SYSTEM 缺少规则/格式: {rule}");
        }
    }

    #[test]
    fn build_includes_task_plan_and_context() {
        let text = build(
            "修复登录空指针",
            "1. [modify] src/auth/login.ts — 添加空值检查",
            &context(&[("src/auth/login.ts", "export login()")]),
        );
        assert!(text.contains("任务：修复登录空指针"), "应包含任务: {text}");
        assert!(
            text.contains("[modify] src/auth/login.ts"),
            "应包含计划步骤: {text}"
        );
        assert!(
            text.contains("=== src/auth/login.ts ==="),
            "应包含上下文路径: {text}"
        );
        assert!(text.contains("export login()"), "应包含上下文片段: {text}");
        assert!(text.ends_with("请生成 unified diff："), "结尾应是生成指令");
    }

    #[test]
    fn build_output_has_stable_shape() {
        let text = build("任务A", "步骤B", &context(&[("a.ts", "code")]));
        assert_eq!(
            text,
            "任务：任务A\n\n计划：\n步骤B\n\n上下文：\n=== a.ts ===\ncode\n\n\n请生成 unified diff："
        );
    }

    #[test]
    fn empty_context_uses_placeholder() {
        let text = build("任意任务", "任意计划", &context(&[]));
        assert!(
            text.contains("（未检索到相关文件）"),
            "空上下文应有占位文案: {text}"
        );
    }

    /// 验收：SYSTEM 的输出格式示例本身必须是可解析且可干净应用到工作区的
    /// unified diff——提示词教模型输出的形态，就是补丁管线消费的形态。
    #[test]
    fn system_output_format_is_parseable_and_applicable() {
        let format_block = &SYSTEM[SYSTEM
            .find("--- a/")
            .expect("SYSTEM 应包含 diff 输出格式示例")..];

        let candidates = parse_diff(format_block).expect("输出格式示例应可解析");
        assert_eq!(candidates.len(), 1, "示例应解析出一个文件的候选");
        assert_eq!(candidates[0].file_path, "path/to/file");

        // 工作区按 @@ -10,3 定位摆放原始内容：第 10-12 行为 context/old line/context。
        let root = tempfile::tempdir().expect("tempdir");
        let target = root.path().join("path/to/file");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut lines: Vec<String> = (1..=9).map(|i| format!("填充第 {i} 行")).collect();
        lines.extend([
            "context".to_string(),
            "old line".to_string(),
            "context".to_string(),
        ]);
        std::fs::write(&target, lines.join("\n") + "\n").unwrap();

        validate_diff(format_block, root.path()).expect("示例 diff 应能对齐工作区");
        apply_diff(format_block, root.path()).expect("示例 diff 应能干净应用");
        let patched = std::fs::read_to_string(&target).unwrap();
        assert!(patched.contains("new line"), "old line 应被替换: {patched}");
        assert!(
            patched.contains("new line 2"),
            "两行新增都应写入: {patched}"
        );
        assert!(!patched.contains("old line"), "old line 应被移除");
        assert_eq!(patched.lines().count(), 13, "10,3 -> 10,4 应净增一行");
    }
}
