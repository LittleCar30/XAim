/**
 * 前后端共享领域类型（与 src-tauri/src/types.rs 一一对应的数据契约）。
 *
 * 约定：
 * - 字段均为 camelCase，与 Rust 侧 serde(rename_all = "camelCase") 的 JSON 输出一致；
 * - Rust 的 `Option<T>` 对应 `| null`；`Vec<T>` 对应 `T[]`（空集合不传 null）；
 * - 时间戳为毫秒级 number（i64，未超 JS 安全整数范围）。
 */

// ---------------------------------------------------------------------------
// 核心领域类型
// ---------------------------------------------------------------------------

/**
 * 任务生命周期状态（15 个），与 Rust TaskStatus 枚举的 camelCase 序列化值一致。
 */
export type TaskStatus =
  | 'idle'
  | 'parsing'
  | 'indexing'
  | 'retrieving'
  | 'planning'
  | 'generating'
  | 'applyingPatch'
  | 'verifying'
  | 'repairing'
  | 'waitingConfirm'
  | 'applied'
  | 'done'
  | 'failed'
  | 'rolledBack'
  | 'cancelled'

/** 项目元信息，对应 Rust Project（projects 表）。 */
export interface Project {
  id: number
  path: string
  name: string
  createdAt: number
  lastOpenedAt: number
}

export interface StorageInfo {
  home: string
  database: string
  workspaces: string
  backups: string
  bytes: number
  workspaceCount: number
  retentionDays: number
  pendingHome: string | null
  environmentOverride: boolean
  notice: string | null
}

export interface RuntimeOverrides {
  taskTimeoutSec?: number
  maxTurns?: number
}

export interface RuntimeSettings {
  defaults: RuntimeOverrides
  shared: RuntimeOverrides
  local: RuntimeOverrides
  effective: Required<RuntimeOverrides>
}

/** 一次编码任务及其统计信息，对应 Rust Task（tasks 表）。 */
export interface Task {
  id: number
  conversationId?: number | null
  parentTaskId?: number | null
  projectId: number
  prompt: string
  status: TaskStatus
  intent: string | null
  plan: string | null
  createdAt: number
  finishedAt: number | null
  durationMs: number | null
  tokenInput: number | null
  tokenOutput: number | null
  cost: number | null
}

/** 任务执行流水中的单步记录，对应 Rust TaskStep（task_steps 表）。 */
export interface TaskStep {
  id: number
  taskId: number
  stepType: string
  content: string
  createdAt: number
}

/** 一份候选补丁，对应 Rust Patch（patches 表）。 */
export interface Patch {
  id: number
  taskId: number
  filePath: string
  diff: string
  applied: boolean
  score: number | null
  createdAt: number
}

/** 单项验证（构建/测试/静态检查）的结果，对应 Rust VerifyResult。 */
export interface VerifyResult {
  kind: string
  passed: boolean
  status?: 'passed' | 'failed' | 'unavailable'
  output: string
  durationMs: number
}

/** 模型接入配置，对应 Rust ModelConfig。 */
export interface ModelConfig {
  provider: string
  name: string
  apiKey: string | null
  baseUrl: string | null
  temperature: number
  maxTokens: number | null
}

/** 任务类型，路由的输入，对应 Rust TaskKind。 */
export type TaskKind = 'intent' | 'retrieve' | 'generate' | 'repair' | 'summarize'

/** 选中的模型（外部标签形式），对应 Rust ModelChoice。 */
export type ModelChoice = { fast: string } | { strong: string }

/** 任务 → 模型路由配置，对应 Rust RouterConfig。 */
export interface RouterConfig {
  fastModel: string
  strongModel: string
  /** 按任务覆盖档位与模型名；未覆盖时序列化省略该字段。 */
  overrides?: Partial<Record<TaskKind, ModelChoice>>
}

/** 应用配置，对应 Rust Config。 */
export interface Config {
  model: ModelConfig
  router: RouterConfig
  modelProfiles?: ModelProfile[]
  activeModelId?: string | null
}

export type ReasoningEffort = 'low' | 'medium' | 'high' | 'max'
export interface ModelProfile {
  id: string
  label: string
  config: ModelConfig
  supportsReasoning: boolean
  reasoningEffort: ReasoningEffort | null
  thinking?: boolean | null
}

/**
 * 文件树节点，对应 Rust FileNode。目录 children 非空；文件 children 为 []，
 * size 仅文件有值（字节数），目录为 null。
 */
export interface FileNode {
  name: string
  path: string
  isDir: boolean
  size: number | null
  children: FileNode[]
}

/** 进入模型上下文的文件片段及入选原因与 token 估算，对应 Rust ContextFile。 */
export interface ContextFile {
  path: string
  content: string
  reason: string
  tokens: number
}

/** 可跳转的代码符号及其行区间，对应 Rust Symbol。 */
export interface Symbol {
  name: string
  kind: string
  file: string
  startLine: number
  endLine: number
}

/** 一次工具调用的结构化结果，对应 Rust ToolResult。 */
export interface ToolResult {
  name: string
  success: boolean
  output: string
  error: string | null
  durationMs: number | null
}

/** 工具调用日志（UI 视图模型），由 XaimToolEvent 追加；事件流不含执行结果。 */
export interface ToolLog {
  taskId: number
  tool: string
  args: string
  /** 前端收到事件时的本地时间戳（毫秒）。 */
  createdAt: number
  /** 执行结果；后端工具事件暂不携带，待结果事件/查询桥接后填充。 */
  result?: ToolResult
}

/** 一条 grep 命中，对应 Rust GrepMatch。 */
export interface GrepMatch {
  path: string
  line: number
  column: number
  text: string
}

/** 从构建/测试输出解析出的一条诊断错误，对应 Rust DiagError。 */
export interface DiagError {
  code: string
  message: string
  severity: string
  file: string | null
  line: number | null
  column: number | null
}

// ---------------------------------------------------------------------------
// 后端事件流
// ---------------------------------------------------------------------------

/** 任务步骤推进事件。 */
export interface XaimStepEvent {
  type: 'step'
  taskId: number
  name: string
  status: string
}

/** 工具调用事件。 */
export interface XaimToolEvent {
  type: 'tool'
  taskId: number
  tool: string
  args: string
}

/** 补丁产出事件。 */
export interface XaimPatchEvent {
  type: 'patch'
  taskId: number
  file: string
  diff: string
}

/** 验证结果事件。 */
export interface XaimVerifyEvent {
  type: 'verify'
  taskId: number
  kind: string
  passed: boolean
  output: string
  status?: 'passed' | 'failed' | 'unavailable'
}

/** 修复回路事件。 */
export interface XaimRepairEvent {
  type: 'repair'
  taskId: number
  round: number
}

/** token 用量事件。 */
export interface XaimTokenEvent {
  type: 'token'
  taskId: number
  input: number
  output: number
}

/** 任务结束事件。 */
export interface XaimDoneEvent {
  type: 'done'
  taskId: number
  status: string
}

/** 错误事件。 */
export interface XaimErrorEvent {
  type: 'error'
  taskId: number
  message: string
}

/** 后端事件流载荷的判别联合，按 `type` 字段区分。 */
export type XaimEvent =
  | { type: 'modelText'; taskId: number; round: number; delta: string; status: 'started' | 'streaming' | 'complete' | 'interrupted' }
  | { type: 'modelResponse'; taskId: number; round: number; finishReason: string; durationMs: number; toolCalls: number }
  | { type: 'toolFinished'; taskId: number; callId: string; result: ToolResult }
  | { type: 'message'; taskId: number; content: string }
  | XaimStepEvent
  | XaimToolEvent
  | XaimPatchEvent
  | XaimVerifyEvent
  | XaimRepairEvent
  | XaimTokenEvent
  | XaimDoneEvent
  | XaimErrorEvent

// ---------------------------------------------------------------------------
// 旧占位类型：组件 / store 仍在引用，待迁移到上面的核心领域类型后移除。
// ---------------------------------------------------------------------------

/** 项目信息（旧）。TODO: 迁移到 Project。 */
export interface ProjectInfo {
  id: string
  name: string
  root: string
}

/** 任务信息（旧）。TODO: 迁移到 Task。 */
export interface TaskInfo {
  id: string
  project: string
  title: string
}

/** 计划步骤状态（旧 UI 视图模型，区别于后端 TaskStep 记录）。 */
export type PlanStepStatus =
  | 'planning'
  | 'executing'
  | 'verifying'
  | 'repairing'
  | 'done'
  | 'failed'

/** 计划步骤（旧 UI 视图模型）。TODO: 与 agent::planner::PlanStep 对齐。 */
export interface PlanStep {
  index: number
  title: string
  status: PlanStepStatus
}

/** Diff 片段。TODO: 与 utils::diff::DiffHunk 对齐。 */
export interface DiffHunk {
  oldStart: number
  newStart: number
  lines: string[]
}

/** 验证问题。TODO: 与 verify::parser 对齐。 */
export interface VerifyIssue {
  file: string
  line: number
  message: string
}

/** 模型提供方。TODO: 与 model 模块对齐。 */
export type ProviderName = 'openai' | 'anthropic' | 'deepseek' | 'qwen' | 'ollama' | 'newapi'
