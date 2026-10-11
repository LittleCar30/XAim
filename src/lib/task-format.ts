import type { TaskStatus } from '@/types'

/** 任务状态 → 中文标签（与后端 TaskStatus 的 camelCase 序列化值对应）。 */
export const TASK_STATUS_LABELS: Record<TaskStatus, string> = {
  idle: '空闲',
  parsing: '解析中',
  indexing: '索引中',
  retrieving: '检索中',
  planning: '规划中',
  generating: '生成中',
  applyingPatch: '应用补丁',
  verifying: '验证中',
  repairing: '修复中',
  waitingConfirm: '等待确认',
  applied: '已应用',
  done: '已完成',
  failed: '失败',
  blocked: '受阻',
  rolledBack: '已回滚',
  cancelled: '已取消',
}

/** 状态徽标配色：进行中蓝、成功绿、失败红、空闲灰。 */
export function taskStatusBadgeCls(status: TaskStatus): string {
  if (status === 'applied' || status === 'done') return 'bg-emerald-500/15 text-emerald-400'
  if (status === 'failed' || status === 'blocked' || status === 'cancelled' || status === 'rolledBack')
    return 'bg-rose-500/15 text-rose-400'
  if (status === 'idle') return 'bg-slate-500/15 text-slate-400'
  return 'bg-sky-500/15 text-sky-400'
}

/** 毫秒 → 人类可读耗时（ms/s/分:秒自适应）。 */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  const total = Math.floor(ms / 1000)
  const m = Math.floor(total / 60)
  const s = total % 60
  if (m === 0) return `${s}s`
  return `${m}m ${String(s).padStart(2, '0')}s`
}

/** token 数自适应格式化（1.2k / 3.4M）。 */
export function formatTokens(n: number): string {
  if (n < 1000) return String(n)
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`
  return `${(n / 1_000_000).toFixed(2)}M`
}

/** 毫秒时间戳 → MM-DD HH:mm（历史列表用的短绝对时间）。 */
export function formatTaskTime(ms: number): string {
  const d = new Date(ms)
  const p = (x: number) => String(x).padStart(2, '0')
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** IPC/store 抛出值的统一展示消息（Tauri 命令失败时 reject 的是 string）。 */
export function toMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}
