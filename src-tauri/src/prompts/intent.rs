//! 意图识别提示词：用户自然语言请求 + 项目语言 -> 结构化意图 JSON。

/// 系统提示词。约束输出格式与行为规则，与 [`crate::agent::planner::ACTIONS`] 的
/// 动作枚举、[`crate::agent::planner::Intent`] 的字段保持一致。
pub const SYSTEM: &str = r#"你是 XAim 的意图解析器。
把用户任务转成 JSON。

输出格式：
{
  "action": "fix_bug | add_test | refactor | review | explain | upgrade | cross_file",
  "target": "文件或模块名",
  "description": "一句话描述",
  "constraints": ["约束1"],
  "verify": ["type", "lint", "test"]
}

规则：
1. 只输出 JSON，不输出其他内容
2. 不编造不存在的文件
3. 不确定的字段留空
4. action 必须是给定枚举之一
"#;

/// Few-shot 示例：`(输入, 期望输出 JSON)` 对。固定输出形态，提升 Fast 档
/// 小模型的格式稳定性；输出 JSON 的键与 [`crate::agent::planner::Intent`] 一致。
pub const FEW_SHOT: &[(&str, &str)] = &[
    (
        "修复登录空指针",
        r#"{"action":"fix_bug","target":"login","description":"修复登录接口空指针","constraints":[],"verify":["type","test"]}"#,
    ),
    (
        "给 auth 模块补单元测试",
        r#"{"action":"add_test","target":"auth","description":"为 auth 模块补单元测试","constraints":["覆盖主要分支"],"verify":["test"]}"#,
    ),
    (
        "把所有 fetch 改成 axios",
        r#"{"action":"cross_file","target":"*","description":"全局替换 fetch 为 axios","constraints":["保持 API 兼容"],"verify":["type","test"]}"#,
    ),
];

/// 渲染用户消息主体：项目语言 + 任务。few-shot 示例由 [`render`] 拼接。
pub fn build(task: &str, lang: &str) -> String {
    format!("项目语言：{}\n任务：{}", lang, task)
}

/// 渲染完整用户消息：few-shot 示例 + [`build`] 主体，并重申只输出 JSON。
pub fn render(task: &str, lang: &str) -> String {
    let mut examples = String::new();
    for (input, output) in FEW_SHOT {
        examples.push_str("输入：");
        examples.push_str(input);
        examples.push_str("\n输出：");
        examples.push_str(output);
        examples.push_str("\n\n");
    }
    format!("{examples}{}\n\n只输出 JSON。", build(task, lang))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::planner::{Intent, ACTIONS};
    use serde_json::Value;

    /// 意图 JSON 必须包含且仅包含的五个键，与 SYSTEM 输出格式一致。
    const INTENT_KEYS: [&str; 5] = ["action", "target", "description", "constraints", "verify"];
    /// verify 允许的取值。
    const VERIFY_KINDS: [&str; 3] = ["type", "lint", "test"];

    /// 20 个样例：`(任务, 项目语言, 期望意图 JSON)`。前三个与 [`FEW_SHOT`] 一致，
    /// 其余覆盖全部 7 种 action、空 target（规则 3"不确定的字段留空"）、
    /// 多约束、未知语言等形态。
    const SAMPLES: [(&str, &str, &str); 20] = [
        (
            "修复登录空指针",
            "rust",
            r#"{"action":"fix_bug","target":"login","description":"修复登录接口空指针","constraints":[],"verify":["type","test"]}"#,
        ),
        (
            "给 auth 模块补单元测试",
            "typescript",
            r#"{"action":"add_test","target":"auth","description":"为 auth 模块补单元测试","constraints":["覆盖主要分支"],"verify":["test"]}"#,
        ),
        (
            "把所有 fetch 改成 axios",
            "javascript",
            r#"{"action":"cross_file","target":"*","description":"全局替换 fetch 为 axios","constraints":["保持 API 兼容"],"verify":["type","test"]}"#,
        ),
        (
            "导出 PDF 时崩溃",
            "typescript",
            r#"{"action":"fix_bug","target":"export/pdf","description":"修复导出 PDF 时的崩溃","constraints":["不改变导出接口"],"verify":["type","test"]}"#,
        ),
        (
            "拆分过大的 UserService",
            "typescript",
            r#"{"action":"refactor","target":"services/user.ts","description":"把 UserService 拆成更小的服务","constraints":["保持公开 API 不变"],"verify":["type","lint","test"]}"#,
        ),
        (
            "审查最近一次提交的改动",
            "go",
            r#"{"action":"review","target":"HEAD~1","description":"审查最近一次提交","constraints":[],"verify":["lint"]}"#,
        ),
        (
            "解释这段正则在做什么",
            "python",
            r#"{"action":"explain","target":"utils/validate.py","description":"解释 validate.py 中的正则逻辑","constraints":[],"verify":[]}"#,
        ),
        (
            "把 React 升级到 19",
            "typescript",
            r#"{"action":"upgrade","target":"package.json","description":"升级 React 到 19","constraints":["逐个修复破坏性变更"],"verify":["type","test"]}"#,
        ),
        (
            "所有日期格式化统一用 dayjs",
            "typescript",
            r#"{"action":"cross_file","target":"*","description":"全局替换 moment 为 dayjs","constraints":["保持格式化输出一致"],"verify":["type","test"]}"#,
        ),
        (
            "让构建快一点",
            "未知",
            r#"{"action":"refactor","target":"","description":"优化构建流程","constraints":[],"verify":["test"]}"#,
        ),
        (
            "给 parser 补边界用例",
            "python",
            r#"{"action":"add_test","target":"parser","description":"为 parser 补充边界条件测试","constraints":["覆盖空输入与超长输入"],"verify":["test"]}"#,
        ),
        (
            "消除 any 类型告警",
            "typescript",
            r#"{"action":"fix_bug","target":"src/types","description":"修复 any 类型导致的类型告警","constraints":["不放松类型检查"],"verify":["type","lint"]}"#,
        ),
        (
            "解释这个错误码的含义",
            "go",
            r#"{"action":"explain","target":"errors.go","description":"解释 errors.go 中错误码语义","constraints":[],"verify":[]}"#,
        ),
        (
            "审查 auth 中间件的安全性",
            "typescript",
            r#"{"action":"review","target":"middleware/auth.ts","description":"审查 auth 中间件安全漏洞","constraints":["只报告不修改"],"verify":["lint"]}"#,
        ),
        (
            "修复内存泄漏",
            "java",
            r#"{"action":"fix_bug","target":"CacheManager","description":"修复缓存管理器的内存泄漏","constraints":[],"verify":["test"]}"#,
        ),
        (
            "为 API 层补集成测试",
            "rust",
            r#"{"action":"add_test","target":"api","description":"为 api 模块补集成测试","constraints":["使用内存数据库"],"verify":["type","lint","test"]}"#,
        ),
        (
            "把回调改成 async/await",
            "python",
            r#"{"action":"cross_file","target":"*","description":"全局把回调风格改为 async/await","constraints":["保持行为一致","分批提交"],"verify":["type","test"]}"#,
        ),
        (
            "依赖都升级到最新小版本",
            "rust",
            r#"{"action":"upgrade","target":"Cargo.toml","description":"升级依赖到最新小版本","constraints":["不引入破坏性更新"],"verify":["type","test"]}"#,
        ),
        (
            "按钮点击偶尔无响应",
            "javascript",
            r#"{"action":"fix_bug","target":"components/Button","description":"修复按钮偶发无响应","constraints":[],"verify":["test"]}"#,
        ),
        (
            "解释项目的构建流程",
            "rust",
            r#"{"action":"explain","target":"","description":"梳理项目构建流程","constraints":[],"verify":[]}"#,
        ),
    ];

    /// 单个样例的完整断言：JSON 可解析、键集合恰好是输出契约五键、
    /// 能反序列化为 [`Intent`] 且 action/verify 合法、提示词携带语言与任务。
    fn assert_sample(idx: usize) {
        let (task, lang, json) = SAMPLES[idx];

        let value: Value =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("样例 {idx} JSON 不可解析: {e}"));
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("样例应为 JSON 对象")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected_keys = INTENT_KEYS;
        expected_keys.sort_unstable();
        assert_eq!(keys, expected_keys, "样例 {idx} 的键集合应与输出契约一致");

        let intent: Intent = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("样例 {idx} 无法反序列化为 Intent: {e}"));
        assert!(
            ACTIONS.contains(&intent.action.as_str()),
            "样例 {idx} action 越界: {}",
            intent.action
        );
        assert!(
            intent
                .verify
                .iter()
                .all(|v| VERIFY_KINDS.contains(&v.as_str())),
            "样例 {idx} verify 越界: {:?}",
            intent.verify
        );

        assert_eq!(
            build(task, lang),
            format!("项目语言：{lang}\n任务：{task}"),
            "样例 {idx} build 输出格式不符"
        );
        let user = render(task, lang);
        assert!(user.contains(task), "样例 {idx} 用户消息应包含任务原文");
        assert!(user.contains(lang), "样例 {idx} 用户消息应包含项目语言");
        assert!(user.contains("只输出 JSON"), "样例 {idx} 应重申只输出 JSON");
    }

    /// 把 20 个样例逐个注册为独立测试：`cargo test` 中每个样例一条，
    /// 失败时直接定位到具体样例。
    macro_rules! sample_tests {
        ($($name:ident => $idx:literal);* $(;)?) => {
            $(
                #[test]
                fn $name() {
                    assert_sample($idx);
                }
            )*
        };
    }

    sample_tests! {
        sample_01_fix_bug_login => 0;
        sample_02_add_test_auth => 1;
        sample_03_cross_file_axios => 2;
        sample_04_fix_bug_export_pdf => 3;
        sample_05_refactor_user_service => 4;
        sample_06_review_last_commit => 5;
        sample_07_explain_regex => 6;
        sample_08_upgrade_react => 7;
        sample_09_cross_file_dayjs => 8;
        sample_10_unknown_lang_blank_target => 9;
        sample_11_add_test_parser_edges => 10;
        sample_12_fix_bug_any_warnings => 11;
        sample_13_explain_error_codes => 12;
        sample_14_review_auth_middleware => 13;
        sample_15_fix_bug_memory_leak => 14;
        sample_16_add_test_api_integration => 15;
        sample_17_cross_file_async_await => 16;
        sample_18_upgrade_dependencies => 17;
        sample_19_fix_bug_button_click => 18;
        sample_20_explain_build_flow => 19;
    }

    #[test]
    fn system_prompt_states_output_contract() {
        for rule in [
            "只输出 JSON",
            "不编造不存在的文件",
            "不确定的字段留空",
            "action 必须是给定枚举之一",
            "target",
            "constraints",
            "verify",
        ] {
            assert!(SYSTEM.contains(rule), "SYSTEM 缺少规则/字段: {rule}");
        }
        for action in ACTIONS {
            assert!(SYSTEM.contains(action), "SYSTEM 缺少枚举值: {action}");
        }
    }

    #[test]
    fn few_shot_examples_are_valid_intents() {
        assert_eq!(FEW_SHOT.len(), 3);
        for (input, output) in FEW_SHOT {
            assert!(!input.trim().is_empty(), "few-shot 输入不应为空");
            let value: Value = serde_json::from_str(output)
                .unwrap_or_else(|e| panic!("few-shot 输出 JSON 不可解析: {e}"));
            assert_eq!(value.as_object().unwrap().len(), INTENT_KEYS.len());
            let intent: Intent =
                serde_json::from_str(output).expect("few-shot 输出应能反序列化为 Intent");
            assert!(ACTIONS.contains(&intent.action.as_str()));
        }
        // 样例表前三个与 FEW_SHOT 保持一致，避免两处漂移。
        for (sample, (input, output)) in SAMPLES.iter().take(3).zip(FEW_SHOT) {
            assert_eq!(&sample.0, input, "样例任务应与 few-shot 输入一致");
            assert_eq!(&sample.2, output, "样例 JSON 应与 few-shot 输出一致");
        }
    }

    #[test]
    fn render_composes_examples_task_and_json_instruction() {
        let text = render("修复导出按钮无响应", "typescript");
        assert!(text.contains("修复登录空指针"), "应包含 few-shot 示例");
        assert!(text.contains("fix_bug"), "应包含 few-shot 输出");
        assert!(text.contains("项目语言：typescript"), "应包含项目语言");
        assert!(text.contains("任务：修复导出按钮无响应"), "应包含用户任务");
        assert!(text.ends_with("只输出 JSON。"), "结尾应重申只输出 JSON");
    }
}
