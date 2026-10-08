import { useEffect, useRef, useState } from 'react'

import { useTaskStore } from '@/stores/task'

/** 工具日志区固定高度：内部滚动，保证上方上下文面板稳定。 */
const PANEL_HEIGHT = 'h-40'

function formatTime(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

/** 参数美化：合法 JSON 展开缩进，否则原样输出。 */
function prettyArgs(args: string): string {
  try {
    return JSON.stringify(JSON.parse(args), null, 2)
  } catch {
    return args
  }
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

function ResultIcon({ success }: { success: boolean }) {
  return success ? (
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
      className="shrink-0 text-emerald-400"
    >
      <circle cx="12" cy="12" r="9" />
      <polyline points="8.5 12.5 11 15 15.5 9.5" />
    </svg>
  ) : (
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
      className="shrink-0 text-rose-400"
    >
      <circle cx="12" cy="12" r="9" />
      <polyline points="9 9 15 15" />
      <polyline points="15 9 9 15" />
    </svg>
  )
}

/** 工具调用日志：事件流实时追加，逐条折叠展示参数与执行结果。 */
export default function ToolLog() {
  const entries = useTaskStore((s) => s.toolLogs)
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set())
  const listRef = useRef<HTMLUListElement | null>(null)

  // 实时更新：新条目到达时滚到最新一条。
  useEffect(() => {
    const el = listRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [entries.length])

  const toggle = (i: number) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(i)) next.delete(i)
      else next.add(i)
      return next
    })
  }

  return (
    <div className={`flex shrink-0 flex-col border-t border-slate-800 ${PANEL_HEIGHT}`}>
      <p className="shrink-0 px-3 pt-2 text-[11px] text-slate-600">工具调用 · {entries.length}</p>
      {entries.length === 0 ? (
        <p className="flex-1 px-3 py-2 text-xs text-slate-500">暂无工具调用</p>
      ) : (
        <ul ref={listRef} className="min-h-0 flex-1 overflow-auto p-2" aria-label="工具调用日志">
          {entries.map((entry, i) => {
            const open = expanded.has(i)
            return (
              <li key={i} className="mb-1 overflow-hidden rounded-md bg-slate-900/60">
                <button
                  type="button"
                  onClick={() => toggle(i)}
                  aria-expanded={open}
                  title={open ? '收起' : '展开参数与结果'}
                  className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-xs hover:bg-slate-800/60"
                >
                  <ChevronIcon open={open} />
                  <span className="shrink-0 font-mono text-[11px] text-slate-600">
                    {formatTime(entry.createdAt)}
                  </span>
                  <span className="shrink-0 rounded bg-sky-500/15 px-1 text-[10px] font-medium leading-4 text-sky-300">
                    {entry.tool}
                  </span>
                  <span className="min-w-0 truncate font-mono text-[11px] text-slate-500">
                    {entry.args}
                  </span>
                  {entry.result && (
                    <span
                      className={`ml-auto shrink-0 rounded px-1 text-[10px] leading-4 ${
                        entry.result.success
                          ? 'bg-emerald-500/15 text-emerald-400'
                          : 'bg-rose-500/15 text-rose-400'
                      }`}
                    >
                      {entry.result.success ? '成功' : '失败'}
                    </span>
                  )}
                </button>
                {open && (
                  <div className="border-t border-slate-800 bg-slate-950 px-2 py-1.5">
                    <p className="text-[10px] text-slate-600">参数</p>
                    <pre className="mt-0.5 max-h-32 overflow-auto whitespace-pre-wrap font-mono text-[11px] leading-4 text-slate-300">
                      {prettyArgs(entry.args)}
                    </pre>
                    {entry.result ? (
                      <div className="mt-1.5">
                        <div className="flex items-center gap-1.5 text-[11px]">
                          <ResultIcon success={entry.result.success} />
                          <span
                            className={
                              entry.result.success ? 'text-emerald-400' : 'text-rose-400'
                            }
                          >
                            {entry.result.success ? '执行成功' : '执行失败'}
                          </span>
                          {entry.result.durationMs != null && (
                            <span className="font-mono text-slate-600">
                              {entry.result.durationMs < 1000
                                ? `${entry.result.durationMs}ms`
                                : `${(entry.result.durationMs / 1000).toFixed(1)}s`}
                            </span>
                          )}
                        </div>
                        {entry.result.output !== '' && (
                          <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap rounded bg-slate-900 p-1.5 font-mono text-[11px] leading-4 text-slate-400">
                            {entry.result.output}
                          </pre>
                        )}
                        {entry.result.error && (
                          <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap rounded bg-slate-900 p-1.5 font-mono text-[11px] leading-4 text-rose-400">
                            {entry.result.error}
                          </pre>
                        )}
                      </div>
                    ) : (
                      <p
                        className="mt-1.5 text-[11px] text-slate-600"
                        title="后端工具事件暂不携带执行结果"
                      >
                        结果未上报
                      </p>
                    )}
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
