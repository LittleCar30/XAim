# 模型配置与实际请求

输入框的“管理模型”和右上角设置共用 `ModelParameters`、配置 store 与后端的 `app:config`。模型参数保存后同步当前 profile、`config.model` 和任务路由；旧的异步配置读取不能覆盖刚保存的选项。保存失败保留上一次有效配置。模型选项在保存后对新任务生效，运行中的任务使用启动时的配置。

## 默认值

- Temperature：0.7，保留原有默认；普通模型实际使用该值。推理配置不支持采样时省略此参数，编辑器禁用输入并解释原因。
- Max Tokens：新增模型默认留空，不再固定 4096。工具循环按已知模型能力或保守预算选择单轮额度；显式值是上限，重试及连接测试不能超过它。
- 思考模式、思考强度：提供方默认。不发送对应字段，不根据 HTML/SVG 任务隐式设置 low。
- 思考强度等级取决于当前适配器和模型能力；自定义 OpenAI 兼容网关需要显式声明支持。思考强度与 Temperature 各自独立。
- 右上角的本地验证偏好默认关闭，避免空验证命令阻止首次保存；项目检查执行仍由每次任务的授权控制。

## 协议映射

| 配置 | 输出上限 | 思考设置 |
| --- | --- | --- |
| OpenAI / NewAPI，支持推理 | `max_completion_tokens` | 显式选择时发送 `reasoning_effort` |
| OpenAI / NewAPI，普通模型 | `max_tokens` | 不发送推理字段 |
| DeepSeek | `max_tokens` | 显式选择时发送 `thinking.type`；支持的模型可发送 `reasoning_effort` |
| Qwen | `max_tokens` | 显式选择时发送 `enable_thinking` |
| Ollama 的 OpenAI 兼容接口 | `max_tokens` | 使用模型默认 |

每次请求只发送一个输出上限字段。推理模型的输出预算可能同时包含思考和可见输出。网关 URL 不决定模型能力，不能把 OpenAI 字段直接套用到所有 Claude、DeepSeek 或 Qwen 端点。

连接测试使用同一 provider 和参数构建器，保持推理支持、思考模式、强度、Temperature 与输出上限规则；探测请求只申请较小输出额度，不代表验证工具调用、上下文容量或任务完成能力。

Anthropic 原生适配器当前尚未实现。配置界面明确提示这一限制；通过网关使用 Claude 时，需选择对应 OpenAI 兼容提供方并遵循网关支持的参数。

## 参考

核对日期：2026-10-08。

- [OpenAI Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)：`max_completion_tokens` 包含推理 token；旧 `max_tokens` 不适用于 o 系列；Temperature 控制采样随机性。
- [Codex 配置参考](https://developers.openai.com/codex/config-reference/)：推理强度由模型能力决定，配置保留显式选择与默认值的区别。
- [Claude Code 模型配置](https://code.claude.com/docs/en/model-config)：模型与 effort 分别配置，显式选择优先于已保存设置和模型默认。未把 Claude 原生参数映射为 OpenAI 参数。

跨入口同步使用组件集成测试验证；参数发送使用本地模拟 HTTP 服务验证，无需真实模型 API 或密钥。
