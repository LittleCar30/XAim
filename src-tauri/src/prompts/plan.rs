//! 计划生成提示词：任务目标 + 代码上下文 -> 执行计划（JSON）。

use crate::search::context::Context;

/// 系统提示词。输出契约与 [`crate::agent::planner::Plan`]/
/// [`crate::agent::planner::STEP_ACTIONS`] 保持一致。
pub const SYSTEM: &str = r#"你是 XAim 的任务规划器。
根据任务和代码上下文，生成执行计划。

输出格式：
{
  "steps": [
    {"action": "read", "file": "src/auth.ts", "reason": "查看登录逻辑"},
    {"action": "modify", "file": "src/auth.ts", "reason": "添加空值检查"},
    {"action": "test", "file": "pnpm test", "reason": "验证修改"}
  ],
  "files_to_modify": ["src/auth.ts"],
  "risks": ["可能影响其他调用方"]
}

规则：
1. 只输出 JSON，不输出其他内容
2. 步骤要具体，action 只能是 read / modify / test，test 步骤的 file 填要运行的验证命令
3. files_to_modify 必须列出所有要修改的文件
4. 标注风险
5. 修改已有文件时不得编造其内容；用户要求新建文件或项目时，允许规划新路径，并在 modify 步骤及 files_to_modify 中列出
6. 空项目的新建文件任务无需读取不存在的文件，也不要假设存在 package.json 或测试工具
"#;

/// 渲染用户消息：任务 + 上下文摘要（[`format_context`]），并重申只输出 JSON。
pub fn build(task: &str, context: &Context) -> String {
    format!(
        "任务：{task}\n\n代码上下文：{}\n\n请生成执行计划。只输出 JSON。",
        format_context(context)
    )
}

/// 把上下文渲染成提示词摘要：逐文件 `=== 路径 ===` + 片段内容；没有检索到
/// 文件时返回占位文案。计划与代码修改两组提示词共用。
pub fn format_context(context: &Context) -> String {
    if context.files.is_empty() {
        return "（未检索到相关文件）".to_string();
    }
    let mut digest = String::new();
    for file in &context.files {
        digest.push_str(&format!("\n=== {} ===\n{}\n", file.path, file.content));
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::context::ContextFile;

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
    fn system_prompt_states_plan_contract() {
        for rule in [
            "只输出 JSON",
            "步骤要具体",
            "files_to_modify 必须列出所有要修改的文件",
            "标注风险",
            "允许规划新路径",
            "\"read\"",
            "\"modify\"",
            "\"test\"",
            "steps",
            "risks",
        ] {
            assert!(SYSTEM.contains(rule), "SYSTEM 缺少规则/字段: {rule}");
        }
    }

    #[test]
    fn build_includes_task_and_context() {
        let text = build(
            "修复登录空指针",
            &context(&[("src/auth/login.ts", "export login()")]),
        );
        assert!(text.contains("任务：修复登录空指针"), "应包含任务: {text}");
        assert!(
            text.contains("=== src/auth/login.ts ==="),
            "应包含文件头: {text}"
        );
        assert!(text.contains("export login()"), "应包含上下文片段: {text}");
        assert!(text.contains("请生成执行计划"), "应包含生成指令");
        assert!(text.ends_with("只输出 JSON。"), "结尾应重申只输出 JSON");
    }

    #[test]
    fn build_lists_multiple_files_in_order() {
        let text = build(
            "拆分 UserService",
            &context(&[
                ("src/user.ts", "class User"),
                ("src/service.ts", "export svc"),
            ]),
        );
        let user = text.find("src/user.ts").expect("应包含第一个文件");
        let service = text.find("src/service.ts").expect("应包含第二个文件");
        assert!(user < service, "上下文文件应按传入顺序排列: {text}");
    }

    #[test]
    fn build_output_has_stable_shape() {
        // 输出形态固定，便于依赖该格式的回归排查与小模型模仿。
        let text = build("任务A", &context(&[("a.ts", "code")]));
        assert_eq!(
            text,
            "任务：任务A\n\n代码上下文：\n=== a.ts ===\ncode\n\n\n请生成执行计划。只输出 JSON。"
        );
    }

    #[test]
    fn empty_context_uses_placeholder() {
        let text = build("任意任务", &context(&[]));
        assert!(
            text.contains("（未检索到相关文件）"),
            "空上下文应有占位文案: {text}"
        );
    }
}
