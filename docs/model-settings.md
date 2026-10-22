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

Anthropic 使用原生 Messages 适配器；通过网关使用 Claude 时，需选择对应兼容协议并遵循网关支持的参数。

## 参考

核对日期：2026-10-08。

- [OpenAI Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)：`max_completion_tokens` 包含推理 token；旧 `max_tokens` 不适用于 o 系列；Temperature 控制采样随机性。
- [Codex 配置参考](https://developers.openai.com/codex/config-reference/)：推理强度由模型能力决定，配置保留显式选择与默认值的区别。
- [Claude Code 模型配置](https://code.claude.com/docs/en/model-config)：模型与 effort 分别配置，显式选择优先于已保存设置和模型默认。未把 Claude 原生参数映射为 OpenAI 参数。

跨入口同步使用组件集成测试验证；参数发送使用本地模拟 HTTP 服务验证，无需真实模型 API 或密钥。

## Responses（2026-10-22）

在设置或管理模型的“请求协议”选择 Responses。OpenAI、NewAPI、DeepSeek 提供此选项；旧配置默认 Chat Completions，不自动迁移。Qwen/Ollama 暂保留 Chat，Anthropic 使用 Messages。NewAPI 的版本、渠道和模型都需要支持所选端点。

Responses 发往 `{baseUrl}/responses`，使用 `input`、扁平 `function` 工具、`function_call_output` 和 `max_output_tokens`。采用 `store:false`，客户端每轮重放历史与原始 reasoning/function_call 项；OpenAI/网关请求加密 reasoning，私有状态不会进入公开日志或 Chat 请求。流必须收到 completed/incomplete/failed 结束事件；没有结束事件不执行工具，传输中断沿用有限重试。仅 max_output_tokens 截断进入现有截断恢复，失败、拒绝或无效参数均不能执行工具。此版本实现文本及函数工具，不提供内置搜索、图片或服务端 conversation。

DeepSeek 官方 Responses 默认地址是 `https://api.deepseek.com`（不加 `/v1`），文档支持模型为 `deepseek-flash` 和 `deepseek-v4-pro`。原生 Responses 将思考关闭映射为 `reasoning.effort:none`，其余强度仅显式发送；网关模型名由运营方确定，不能假定相同。自定义 Base URL 保持用户指定路径。

预算收尾自动复用当前文件版本的检查结果，隐藏重复验证工具，保留最后一次仅提交状态的请求；有待办仍保存 partial，不能伪装完成。没有清单的项目仅对根目录最多 64 个 JS/CJS/MJS 文件执行 `node --check`，不执行 build.js 等未知脚本；这只证明语法检查结果，不代表构建、页面效果或功能测试通过。缺少 Node 或检查命令仍报告 unavailable。

- [OpenAI Responses 参考](https://developers.openai.com/api/reference/resources/responses/methods/create)
- [DeepSeek Responses 兼容范围](https://api-docs.deepseek.com/guides/responses_api)
- [DeepSeek Responses 参数](https://api-docs.deepseek.com/api/create-response)
