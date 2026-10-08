import { useEffect, useMemo, useRef, useState } from 'react'

import { useTaskStore } from '@/stores/task'

export interface ContextViewerProps {
  /** TODO: 高亮关键词覆盖（缺省从当前任务描述提取）。 */
  path?: never
}

/**
 * 从任务描述提取关键词，对齐后端 `task_keywords`：按非字母数字字符切分、
 * ≥2 字符（非 ASCII 单字保留）、小写去重。
 */
function hasNonAscii(s: string): boolean {
  return [...s].some((ch) => ch.charCodeAt(0) > 127)
}

function extractKeywords(prompt: string): string[] {
  return [
    ...new Set(
      prompt
        .split(/[^\p{L}\p{N}]+/u)
        .filter((kw) => kw.length > 0 && (kw.length >= 2 || hasNonAscii(kw)))
        .map((kw) => kw.toLowerCase()),
    ),
  ]
}

/** 行文本含任一关键词即视为相关行（对齐后端"符号名命中关键词"的片段选取思路）。 */
function isRelevantLine(line: string, keywords: readonly string[]): boolean {
  if (keywords.length === 0) return false
  const lc = line.toLowerCase()
  return keywords.some((kw) => lc.includes(kw))
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

function TargetIcon() {
  return (
    <svg
      width="11"
      height="11"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="mt-0.5 shrink-0"
    >
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="4" />
      <polyline points="12 1 12 4" />
      <polyline points="12 20 12 23" />
      <polyline points="1 12 4 12" />
      <polyline points="20 12 23 12" />
    </svg>
  )
}

/** 上下文查看器：展示检索阶段实际注入模型的文件片段，含 token 估算、入选原因与相关行高亮。 */
export default function ContextViewer(_props: ContextViewerProps) {
  const context = useTaskStore((s) => s.context)
  const prompt = useTaskStore((s) => s.currentTask?.prompt ?? '')

  // 挂载时展开首个（相关度最高的）文件；流式到达的新文件接管展开。
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(
    () => new Set(context.length > 0 ? [0] : []),
  )
  const lastCountRef = useRef(context.length)

  useEffect(() => {
    if (context.length > lastCountRef.current) {
      setExpanded((prev) => new Set(prev).add(context.length - 1))
    }
    lastCountRef.current = context.length
  }, [context])

  const keywords = useMemo(() => extractKeywords(prompt), [prompt])
  const totalTokens = useMemo(
    () => context.reduce((sum, f) => sum + f.tokens, 0),
    [context],
  )

  if (context.length === 0) {
    return (
      <section className="flex min-h-0 flex-1 flex-col">
        <div className="shrink-0 border-b border-slate-800 px-3 py-2">
          <p className="text-[11px] text-slate-600">上下文 · 暂无</p>
        </div>
        <div className="flex min-h-0 flex-1 items-start p-3">
          <p className="text-xs text-slate-500">
            任务运行后，检索阶段选中并注入模型的文件片段将显示在此处。
          </p>
        </div>
      </section>
    )
  }

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-slate-800 px-3 py-2">
        <p className="text-[11px] text-slate-600">
          上下文 · {context.length} 个文件 · {totalTokens} tokens
        </p>
      </div>
      <ul className="min-h-0 flex-1 overflow-auto p-2">
        {context.map((file, i) => {
          const open = expanded.has(i)
          const lines = file.content.split('\n')
          return (
            <li key={`${file.path}-${i}`} className="mb-1 overflow-hidden rounded-md bg-slate-900/60">
              <button
                type="button"
                onClick={() =>
                  setExpanded((prev) => {
                    const next = new Set(prev)
                    if (next.has(i)) next.delete(i)
                    else next.add(i)
                    return next
                  })
                }
                aria-expanded={open}
                className="w-full px-2 py-1.5 text-left hover:bg-slate-800/60"
              >
                <div className="flex items-center gap-1.5 text-xs">
                  <ChevronIcon open={open} />
                  <span className="shrink-0 text-[10px] text-slate-600">#{i + 1}</span>
                  <span className="min-w-0 truncate font-mono text-slate-200" title={file.path}>
                    {file.path}
                  </span>
                  <span
                    className="ml-auto shrink-0 rounded bg-slate-500/15 px-1 text-[10px] leading-4 text-slate-400"
                    title="token 估算"
                  >
                    {file.tokens} tok
                  </span>
                </div>
                <div className="mt-0.5 flex items-start gap-1 pl-4 text-[11px] text-amber-400/80">
                  <TargetIcon />
                  <span className="min-w-0 break-all" title={`入选原因：${file.reason}`}>
                    {file.reason}
                  </span>
                </div>
              </button>
              {open && (
                <pre className="max-h-56 overflow-auto border-t border-slate-800 bg-slate-950 font-mono text-[11px] leading-4">
                  {lines.map((line, j) => {
                    const relevant = isRelevantLine(line, keywords)
                    return (
                      <div
                        key={j}
                        className={`flex ${relevant ? 'bg-sky-500/10 text-sky-200' : 'text-slate-500'}`}
                      >
                        <span className="w-8 shrink-0 select-none pr-2 text-right text-slate-700">
                          {j + 1}
                        </span>
                        <span className="whitespace-pre">{line === '' ? ' ' : line}</span>
                      </div>
                    )
                  })}
                </pre>
              )}
            </li>
          )
        })}
      </ul>
    </section>
  )
}
