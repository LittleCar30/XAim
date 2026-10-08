//! 总结提示词：任务过程（补丁 + 验证）-> 面向用户的变更摘要。

use crate::types::{Patch, VerifyResult};

/// 系统提示词。
pub const SYSTEM: &str = r#"你是 XAim，总结任务执行结果。
用简洁中文总结：
1. 做了什么
2. 改了哪些文件
3. 验证结果
4. 注意事项
"#;

/// 渲染总结用户消息：原任务 + 改动文件摘要 + 验证结果摘要。
/// 补丁按文件路径去重（同文件多个候选取"已应用"状态），只给文件清单、
/// 不带 diff 正文；验证只给 kind/结论/耗时，不带原始输出。
pub fn build(task: &str, patches: &[Patch], verifies: &[VerifyResult]) -> String {
    let mut files: Vec<(String, bool)> = Vec::new();
    for patch in patches {
        match files.iter_mut().find(|(path, _)| *path == patch.file_path) {
            Some(entry) => entry.1 |= patch.applied,
            None => files.push((patch.file_path.clone(), patch.applied)),
        }
    }

    let patch_digest = if files.is_empty() {
        "（无）".to_string()
    } else {
        files
            .iter()
            .map(|(path, applied)| {
                format!("- {path}（{}）", if *applied { "已应用" } else { "未应用" })
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let verify_digest = if verifies.is_empty() {
        "（无）".to_string()
    } else {
        verifies
            .iter()
            .map(|v| {
                format!(
                    "- {}：{}（{}）",
                    v.kind,
                    if v.passed { "通过" } else { "未通过" },
                    format_duration(v.duration_ms),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "任务：{task}\n\n改动的文件：\n{patch_digest}\n\n验证结果：\n{verify_digest}\n\n\
         请用简洁中文总结本次任务执行结果，覆盖：做了什么、改了哪些文件、验证结果、注意事项。\
         只输出总结文字。"
    )
}

/// 毫秒时长渲染：不足 1 秒显示毫秒，否则显示一位小数的秒。
fn format_duration(ms: i64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(path: &str, applied: bool) -> Patch {
        Patch {
            id: 0,
            task_id: 0,
            file_path: path.to_string(),
            diff: String::new(),
            applied,
            score: None,
            created_at: 0,
        }
    }

    fn verify(kind: &str, passed: bool, duration_ms: i64) -> VerifyResult {
        VerifyResult {
            kind: kind.to_string(),
            passed,
            output: String::new(),
            duration_ms,
        }
    }

    #[test]
    fn system_prompt_states_summary_contract() {
        for item in ["做了什么", "改了哪些文件", "验证结果", "注意事项"] {
            assert!(SYSTEM.contains(item), "SYSTEM 缺少总结维度: {item}");
        }
    }

    #[test]
    fn build_renders_patches_and_verifies_readably() {
        let patches = [
            patch("src/a.ts", true),
            patch("src/b.ts", false),
            // 同文件多候选取"已应用"，且不重复列文件。
            patch("src/a.ts", false),
        ];
        let verifies = [
            verify("test", true, 1_200),
            verify("type_check", false, 800),
        ];
        let text = build("修复登录空指针", &patches, &verifies);

        assert!(text.contains("任务：修复登录空指针"), "{text}");
        assert!(text.contains("- src/a.ts（已应用）"), "{text}");
        assert!(text.contains("- src/b.ts（未应用）"), "{text}");
        assert_eq!(text.matches("src/a.ts").count(), 1, "文件应去重: {text}");
        assert!(text.contains("- test：通过（1.2s）"), "{text}");
        assert!(text.contains("- type_check：未通过（800ms）"), "{text}");
        assert!(text.contains("做了什么"), "应重申总结维度: {text}");
        assert!(text.ends_with("只输出总结文字。"), "{text}");
    }

    #[test]
    fn build_empty_sections_use_placeholder() {
        let text = build("任务A", &[], &[]);
        assert!(text.contains("改动的文件：\n（无）"), "{text}");
        assert!(text.contains("验证结果：\n（无）"), "{text}");
    }

    #[test]
    fn build_output_has_stable_shape() {
        let text = build(
            "任务T",
            &[patch("a.ts", true)],
            &[verify("lint", true, 2_000)],
        );
        assert_eq!(
            text,
            "任务：任务T\n\n改动的文件：\n- a.ts（已应用）\n\n验证结果：\n- lint：通过（2.0s）\n\n\
             请用简洁中文总结本次任务执行结果，覆盖：做了什么、改了哪些文件、验证结果、注意事项。\
             只输出总结文字。"
        );
    }

    #[test]
    fn duration_formats_ms_below_one_second() {
        assert_eq!(format_duration(0), "0ms");
        assert_eq!(format_duration(999), "999ms");
        assert_eq!(format_duration(1_000), "1.0s");
        assert_eq!(format_duration(1_500), "1.5s");
    }
}
