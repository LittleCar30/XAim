//! 修复提示词：验证失败诊断 + 相关代码 + 原任务 -> 修复 diff。

/// 系统提示词。输出格式示例是 [`crate::tools::patch`] 可直接解析的最小合法
/// unified diff（hunk 头行数与正文严格一致），避免模型模仿出非法形态。
pub const SYSTEM: &str = r#"你是 XAim，修复代码错误。
根据错误信息和当前代码生成修复 diff。

规则：
1. 只修错误，不重构
2. 输出 unified diff
3. 不引入新错误
4. 不输出解释

输出格式：
--- a/path
+++ b/path
@@ -1,3 +1,4 @@
 context
-old line
+new line
+new line 2
 context
"#;

/// 渲染修复用户消息：原任务 + 诊断错误 + 相关代码，并重申只输出 diff 补丁。
/// 诊断文本由 `repair::loop::render_diagnostics` 生成，相关代码由其
/// `collect_code_context` 收集（出错文件片段）。
pub fn build(errors: &str, code: &str, task: &str) -> String {
    format!(
        "任务：{task}\n\n验证未通过，以下是解析出的诊断错误：\n{errors}\n\n相关代码：\n{code}\n\n\
         请修复以上错误。只输出一个标准 unified diff 补丁：包含 \
         `--- a/<相对路径>`、`+++ b/<相对路径>` 与 `@@` hunk 头，路径相对项目根目录，\
         可整体包裹在 ```diff 代码围栏中；不要输出补丁之外的任何解释文字。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::patch::parse_diff;

    #[test]
    fn system_prompt_states_repair_contract() {
        for rule in [
            "只修错误，不重构",
            "输出 unified diff",
            "不引入新错误",
            "不输出解释",
            "--- a/path",
            "+++ b/path",
            "@@",
        ] {
            assert!(SYSTEM.contains(rule), "SYSTEM 缺少规则/格式: {rule}");
        }
    }

    /// 输出格式示例必须本身可解析：提示词教模型输出的形态就是补丁管线
    /// 消费的形态（与 prompts::modify 的验收口径一致）。
    #[test]
    fn system_output_format_is_parseable() {
        let format_block = &SYSTEM[SYSTEM
            .find("--- a/")
            .expect("SYSTEM 应包含 diff 输出格式示例")..];
        let candidates = parse_diff(format_block).expect("输出格式示例应可解析");
        assert_eq!(candidates.len(), 1, "示例应解析出一个文件的候选");
        assert_eq!(candidates[0].file_path, "path");
    }

    #[test]
    fn build_includes_task_errors_and_code() {
        let text = build(
            "- [error] src/lib.rs:1:20: 表达式未完成",
            "\n=== src/lib.rs ===\npub fn add(a: i32, b: i32) -> i32 { a + }\n",
            "实现加法函数",
        );
        assert!(text.contains("任务：实现加法函数"), "应包含原任务: {text}");
        assert!(
            text.contains("src/lib.rs:1:20: 表达式未完成"),
            "应包含诊断错误"
        );
        assert!(text.contains("=== src/lib.rs ==="), "应包含相关代码路径");
        assert!(text.contains("pub fn add"), "应包含相关代码片段");
        assert!(text.contains("请修复以上错误"), "应包含修复指令");
        assert!(text.contains("```diff"), "应说明允许 diff 代码围栏");
        assert!(
            text.ends_with("不要输出补丁之外的任何解释文字。"),
            "结尾应重申只输出补丁"
        );
    }

    #[test]
    fn build_output_has_stable_shape() {
        let text = build("错误E", "代码C", "任务T");
        assert_eq!(
            text,
            "任务：任务T\n\n验证未通过，以下是解析出的诊断错误：\n错误E\n\n相关代码：\n代码C\n\n\
             请修复以上错误。只输出一个标准 unified diff 补丁：包含 \
             `--- a/<相对路径>`、`+++ b/<相对路径>` 与 `@@` hunk 头，路径相对项目根目录，\
             可整体包裹在 ```diff 代码围栏中；不要输出补丁之外的任何解释文字。"
        );
    }
}
