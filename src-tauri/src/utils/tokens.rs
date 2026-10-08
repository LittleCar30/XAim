//! token 估算。
//!
//! 不接入 tiktoken（Rust 生态不成熟），采用规格约定的线性近似模型：
//! - 英文（ASCII）：4 字符 ≈ 1 token；
//! - 中文等非 ASCII 字符：1.5 字符 ≈ 1 token；
//! - 混合文本：按字符类型**分别**向上取整后求和。
//!
//! 对代码而言 4 字符/token 与主流 BPE 分词器（tiktoken 对代码均值约
//! 3.5~4.3 字符/token）偏差在 20% 以内；中文 1.5 字符/token 亦落在
//! cl100k 实测的 1~2 字符/token 区间。结果一律向上取整，宁可高估，
//! 保证调用方（如上下文组装的预算控制）不超支。

/// 混合文本的 token 估算：英文部分按 4 字符 1 token、中文部分按
/// 1.5 字符 1 token 分别向上取整后求和。
pub fn estimate_tokens(text: &str) -> usize {
    let (ascii, wide) = char_counts(text);
    ceil_div(ascii, 4) + ceil_div(2 * wide, 3)
}

/// 整串按中文速率（1.5 字符 1 token）估算，用于已知以中文为主的文本
/// （如用户任务描述）。对英文文本会显著高估（保守方向）；纯中文文本
/// 与 [`estimate_tokens`] 结果一致，混合文本请以 [`estimate_tokens`] 为准。
pub fn estimate_tokens_cjk(text: &str) -> usize {
    ceil_div(2 * text.chars().count() as u64, 3)
}

/// (ASCII 字符数, 非 ASCII 字符数)。emoji 等其他非 ASCII 字符统一按
/// 中文速率近似（对多数分词器而言这是保守方向）。
fn char_counts(text: &str) -> (u64, u64) {
    let mut counts = (0u64, 0u64);
    for c in text.chars() {
        if c.is_ascii() {
            counts.0 += 1;
        } else {
            counts.1 += 1;
        }
    }
    counts
}

/// 非负整数除法向上取整。
fn ceil_div(a: u64, b: u64) -> usize {
    a.div_ceil(b) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_costs_nothing() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens_cjk(""), 0);
    }

    #[test]
    fn ascii_counts_four_chars_per_token() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        // 不足 4 字符向上取整。
        assert_eq!(estimate_tokens("abc"), 1);
    }

    #[test]
    fn cjk_counts_one_and_half_chars_per_token() {
        assert_eq!(estimate_tokens("中"), 1); // 0.67 → 取整 1
        assert_eq!(estimate_tokens("中文中"), 2); // 3 / 1.5 = 2
        assert_eq!(estimate_tokens("中文中文中中"), 4); // 6 / 1.5 = 4
    }

    #[test]
    fn mixed_text_computes_per_char_type() {
        // 英文 3/4 → 1，中文 2/1.5 → 2，分别取整后求和。
        assert_eq!(estimate_tokens("abc中文"), 3);
        // "ab中中"：1 + 2 = 3；合并后再取整只算 2，会低估真实分词。
        assert_eq!(estimate_tokens("ab中中"), 3);
        assert_eq!(estimate_tokens("a中"), 2);
    }

    #[test]
    fn cjk_variant_uses_cjk_rate_for_whole_string() {
        assert_eq!(estimate_tokens_cjk("中文中"), 2);
        // 英文按中文速率估算：4 字符 → 3 token，显著高估（保守方向）。
        assert_eq!(estimate_tokens_cjk("abcd"), 3);
        // 纯中文文本与混合估算一致。
        for text in ["中文中", "上下文组装的预算控制", "修", ""] {
            assert_eq!(
                estimate_tokens_cjk(text),
                estimate_tokens(text),
                "纯中文文本 {text} 两个估算应一致"
            );
        }
    }

    #[test]
    fn estimation_is_linear_within_twenty_percent() {
        let line = "fn process_record(input: &str, cache: &mut Vec<String>) -> bool {";
        let single = estimate_tokens(line);
        let repeated = estimate_tokens(&line.repeat(10));
        let ratio = repeated as f64 / (single * 10) as f64;
        assert!(
            (0.8..=1.2).contains(&ratio),
            "重复 10 次的估算 {repeated} 应在 10 × {single} 的 ±20% 内"
        );
    }
}
