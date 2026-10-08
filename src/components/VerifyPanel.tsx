import { useState } from 'react'
import { CircleHelp } from 'lucide-react'

import type { VerifyResult } from '@/types'

export interface VerifyPanelProps {
  /** 验证结果列表（来自 task store 的 verify 事件流）。 */
  verifies: VerifyResult[]
}

/** 验证类别：与后端 VerifyKind::as_str 对应；未知 kind 归为其他。 */
type VerifyCategory = 'type_check' | 'lint' | 'test' | 'build' | 'other'

function categoryOf(kind: string): VerifyCategory {
  switch (kind) {
    case 'type_check':
      return 'type_check'
    case 'lint':
      return 'lint'
    case 'test':
      return 'test'
    case 'build':
      return 'build'
    default:
      return 'other'
  }
}

/** 类别 → 中文名与徽标配色。 */
const CATEGORY_META: Record<VerifyCategory, { label: string; badgeCls: string }> = {
  type_check: { label: '类型检查', badgeCls: 'bg-violet-500/15 text-violet-400' },
  lint: { label: 'Lint', badgeCls: 'bg-amber-500/15 text-amber-400' },
  test: { label: '测试', badgeCls: 'bg-sky-500/15 text-sky-400' },
  build: { label: '构建', badgeCls: 'bg-orange-500/15 text-orange-400' },
  other: { label: '其他', badgeCls: 'bg-slate-500/15 text-slate-400' },
}

function PassIcon() {
  return (
    <svg
      width="13"
      height="13"
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

function FailIcon() {
  return (
    <svg
      width="13"
      height="13"
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
      <polyline points="9 9 15 15" />
      <polyline points="15 9 9 15" />
    </svg>
  )
}

function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={`shrink-0 text-slate-500 transition-transform ${open ? 'rotate-90' : ''}`}
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  )
}

/** 毫秒 → 人类可读耗时。 */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

/** 输出摘要：第一行非空文本（行内展示用）。 */
function outputSummary(output: string): string {
  return output.split('\n').find((l) => l.trim() !== '')?.trim() ?? ''
}

/** 验证面板：类型检查 / Lint / 测试 / 构建结果流；通过绿、失败红，行内展开详情。 */
export default function VerifyPanel({ verifies }: VerifyPanelProps) {
  // 展开状态按结果序号记录（事件流追加式，序号稳定）。
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set())

  const toggle = (i: number) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(i)) next.delete(i)
      else next.add(i)
      return next
    })
  }

  if (verifies.length === 0) {
    return (
      <div className="shrink-0 border-t border-slate-800 px-3 py-2">
        <p className="text-[11px] text-slate-600">验证 · 暂无</p>
      </div>
    )
  }

  const passedCount = verifies.filter((v) => v.passed).length

  return (
    <div className="flex shrink-0 flex-col border-t border-slate-800">
      <p className="px-3 pt-2 text-[11px] text-slate-600">
        验证 · {passedCount}/{verifies.length} 通过
      </p>
      <ul className="max-h-36 overflow-auto p-2">
        {verifies.map((v, i) => {
          const meta = CATEGORY_META[categoryOf(v.kind)]
          const open = expanded.has(i)
          return (
            <li key={i} className="mb-1 overflow-hidden rounded-md bg-slate-900/60">
              <button
                type="button"
                onClick={() => toggle(i)}
                aria-expanded={open}
                title={open ? '收起详情' : '查看详情'}
                className="flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs hover:bg-slate-800/60"
              >
                <ChevronIcon open={open} />
                {v.status === 'unavailable' ? <CircleHelp size={13} className="text-slate-500" /> : v.passed ? <PassIcon /> : <FailIcon />}
                <span className={`shrink-0 rounded px-1 text-[10px] leading-4 ${meta.badgeCls}`}>
                  {meta.label}
                </span>
                <span
                  className={`min-w-0 truncate text-[11px] ${v.status === 'unavailable' ? 'text-slate-500' : v.passed ? 'text-emerald-400' : 'text-rose-400'}`}
                >
                  {v.status === 'unavailable' ? '无法验证' : v.passed ? '通过' : '未通过'}
                  {outputSummary(v.output) && (
                    <span className="text-slate-500"> · {outputSummary(v.output)}</span>
                  )}
                </span>
                <span className="ml-auto shrink-0 font-mono text-[11px] text-slate-600">
                  {formatDuration(v.durationMs)}
                </span>
              </button>
              {open && v.output !== '' && (
                <pre className="max-h-32 overflow-auto whitespace-pre-wrap border-t border-slate-800 bg-slate-950 px-2 py-1.5 font-mono text-[11px] leading-4 text-slate-400">
                  {v.output}
                </pre>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
