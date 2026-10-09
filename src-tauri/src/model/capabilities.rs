//! Conservative request budgets; unknown endpoints are not treated as known model limits.

#[derive(Debug, Clone, Copy)]
pub struct ModelLimits {
    pub context_tokens: usize,
    pub max_output_tokens: u32,
    pub default_output_tokens: u32,
    pub documented: bool,
}

pub fn reasoning_efforts(provider: &str, model: &str) -> &'static [&'static str] {
    match provider {
        // Custom compatible OpenAI endpoints remain an explicit profile opt-in.
        "openai" | "newapi" => &["low", "medium", "high"],
        "deepseek" if matches!(model, "deepseek-flash" | "deepseek-v4-pro") => {
            &["low", "high", "max"]
        }
        _ => &[],
    }
}

pub fn supports_thinking_toggle(provider: &str, model: &str) -> bool {
    match provider {
        "deepseek" => matches!(model, "deepseek-flash" | "deepseek-v4-pro"),
        "qwen" => matches!(
            model,
            "qwen3-235b-a22b"
                | "qwen3-30b-a3b"
                | "qwen3-32b"
                | "qwen3-14b"
                | "qwen3-8b"
                | "qwen3-4b"
                | "qwen3-1.7b"
                | "qwen3-0.6b"
        ),
        _ => false,
    }
}

pub fn limits(provider: &str, model: &str, reasoning: bool) -> ModelLimits {
    match (provider, model) {
        ("deepseek", "deepseek-flash" | "deepseek-v4-pro") => ModelLimits {
            context_tokens: 1_000_000,
            max_output_tokens: 384_000,
            default_output_tokens: 32_768,
            documented: true,
        },
        ("deepseek", "deepseek-chat") => ModelLimits {
            context_tokens: 128_000,
            max_output_tokens: 8192,
            default_output_tokens: 8192,
            documented: true,
        },
        ("deepseek", "deepseek-reasoner") => ModelLimits {
            context_tokens: 128_000,
            max_output_tokens: 65_536,
            default_output_tokens: 32_768,
            documented: true,
        },
        ("openai", "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-luna") => ModelLimits {
            context_tokens: 1_050_000,
            max_output_tokens: 128_000,
            default_output_tokens: 32_768,
            documented: true,
        },
        ("openai", "gpt-5-codex") => ModelLimits {
            context_tokens: 400_000,
            max_output_tokens: 128_000,
            default_output_tokens: 32_768,
            documented: true,
        },
        _ => ModelLimits {
            context_tokens: 128_000,
            max_output_tokens: if reasoning { 32_768 } else { 8192 },
            default_output_tokens: if reasoning { 32_768 } else { 8192 },
            documented: false,
        },
    }
}

pub fn validate_output(
    provider: &str,
    model: &str,
    output: Option<u32>,
) -> crate::errors::Result<()> {
    if let Some(output) = output {
        let caps = limits(provider, model, false);
        if output == 0 || caps.documented && output > caps.max_output_tokens {
            return Err(crate::errors::XaimError::Config(format!(
                "{model}: 输出额度为零或超过已知模型上限"
            )));
        }
    }
    Ok(())
}
