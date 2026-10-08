import { useEffect, useRef } from 'react'

import { useTaskStore } from '@/stores/task'
import type { TaskStep } from '@/types'

/** 步骤状态：由事件流 content 字段映射（后端每阶段成对推送 running/done）。 */
type StepStatus = 'running' | 'done' | 'failed'

/** content → 状态；未知取值按进行中处理，失败类关键词防御性命中。 */
function statusOf(step: TaskStep): StepStatus {
  const s = step.content.trim().toLowerCase()
  if (/(^done$|success|^ok$|完成|成功|通过)/.test(s)) return 'done'
  if (/(fail|error|cancel|timeout|失败|错误|取消|超时)/.test(s)) return 'failed'
  return 'running'
}

const STATUS_LABELS: Record<StepStatus, string> = {
  running: '进行中',
  done: '完成',
  failed: '失败',
}

const STATUS_TEXT_CLS: Record<StepStatus, string> = {
  running: 'text-sky-400',
  done: 'text-emerald-400',
  failed: 'text-rose-400',
}

function CheckIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="shrink-0 text-emerald-400"
    >
      <circle cx="12" cy="12" r="9" />
      <polyline points="8.5 12.5 11 15 15.5 9.5" />
    </svg>
  )
}

function RunningIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="shrink-0 animate-pulse text-sky-400"
    >
      <circle cx="12" cy="12" r="9" />
      <polyline points="12 7 12 12 15.5 14" />
    </svg>
  )
}

function FailIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="shrink-0 text-rose-400"
    >
      <circle cx="12" cy="12" r="9" />
      <polyline points="9 9 15 15 15 15 9 9" />
      <polyline points="15 9 9 15" />
    </svg>
  )
}

function StatusIcon({ status }: { status: StepStatus }) {
  if (status === 'done') return <CheckIcon />
  if (status === 'failed') return <FailIcon />
  return <RunningIcon />
}

/** 毫秒时间戳 → HH:MM:SS（本地时区，固定宽度）。 */
function formatTime(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

/** 步骤时间线：展示任务事件流的步骤推进，最新一行高亮为当前并自动滚入视野。 */
export default function StepTimeline() {
  const steps = useTaskStore((s) => s.steps)
  const scrollRef = useRef<HTMLOListElement | null>(null)

  // 新步骤到达时滚到最新一行（当前进度总在列表末尾）。
  useEffect(() => {
    const el = scrollRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [steps.length])

  if (steps.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <p className="p-3 text-xs text-slate-500">暂无步骤</p>
      </div>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ol ref={scrollRef} aria-label="任务步骤" className="min-h-0 flex-1 overflow-auto py-1">
        {steps.map((step, i) => {
          const status = statusOf(step)
          const isCurrent = i === steps.length - 1
          return (
            <li
              key={`${step.id}-${i}`}
              aria-current={isCurrent ? 'step' : undefined}
              className={`flex h-7 shrink-0 items-center gap-2 rounded-md px-2 text-xs ${
                isCurrent ? 'bg-sky-500/10 text-slate-100' : 'text-slate-400 hover:bg-slate-800/50'
              }`}
            >
              <StatusIcon status={status} />
              <span className="min-w-0 truncate font-medium text-slate-200">{step.stepType}</span>
              <span className={`shrink-0 text-[11px] ${STATUS_TEXT_CLS[status]}`}>
                {STATUS_LABELS[status]}
              </span>
              {isCurrent && (
                <span className="shrink-0 rounded bg-sky-500/20 px-1 text-[10px] leading-4 text-sky-300">
                  当前
                </span>
              )}
              <span className="ml-auto shrink-0 font-mono text-[11px] text-slate-600">
                {formatTime(step.createdAt)}
              </span>
            </li>
          )
        })}
      </ol>
    </div>
  )
}
