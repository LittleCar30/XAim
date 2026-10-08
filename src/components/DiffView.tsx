import { lazy, Suspense, useEffect, useRef, useState } from 'react'

import type { Patch } from '@/types'

// Monaco 体积较大：整个 Diff 编辑器（含 monaco-editor 与 @monaco-editor/react）
// 走动态 import，仅在首个补丁展开时由 Vite 异步加载，不进首屏包体。
const PatchDiff = lazy(() => import('@/components/PatchDiff'))

export interface DiffViewProps {
  compact?: boolean
  /** 候选补丁列表（来自 task store 的事件流）。 */
  patches: Patch[]
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

/** 候选 Diff 列表：手风琴式展开（同时最多一个 Monaco 实例），显示评分与应用状态。 */
export default function DiffView({ patches, compact = false }: DiffViewProps) {
  // 挂载时（如任务中途刷新页面）直接展开最新补丁；此后流式到达的新补丁接管展开。
  const [expandedId, setExpandedId] = useState<number | null>(
    () => (!compact && patches.length > 0 ? patches[patches.length - 1].id : null),
  )
  const lastCountRef = useRef(patches.length)

  useEffect(() => {
    if (!compact && patches.length > lastCountRef.current) {
      setExpandedId(patches[patches.length - 1].id)
    }
    lastCountRef.current = patches.length
  }, [patches, compact])

  if (patches.length === 0) {
    return (
      <div className="shrink-0 border-t border-slate-800 px-3 py-2">
        <p className="text-[11px] text-slate-600">候选补丁 · 暂无</p>
      </div>
    )
  }

  return (
    <div className={compact ? 'compact-diffs' : 'flex shrink-0 flex-col border-t border-slate-800'}>
      {!compact &&
      <p className="px-3 pt-2 text-[11px] text-slate-600">
        候选补丁 · {patches.length}（{patches.filter((p) => p.applied).length} 已应用）
      </p>}
      <ul className="max-h-[26rem] overflow-auto p-2">
        {patches.map((patch) => {
          const open = expandedId === patch.id
          return (
            <li key={patch.id} className="mb-1 overflow-hidden rounded-md bg-slate-900/60">
              <button
                type="button"
                onClick={() => setExpandedId(open ? null : patch.id)}
                aria-expanded={open}
                className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-xs text-slate-300 hover:bg-slate-800/60"
              >
                <ChevronIcon open={open} />
                <span className="min-w-0 truncate font-mono" title={patch.filePath}>
                  {patch.filePath}
                </span>
                {patch.score != null && (
                  <span
                    className="shrink-0 rounded bg-slate-500/15 px-1 text-[10px] leading-4 text-slate-400"
                    title="补丁评分"
                  >
                    score {patch.score}
                  </span>
                )}
                <span
                  className={`ml-auto shrink-0 rounded px-1 text-[10px] leading-4 ${
                    patch.applied
                      ? 'bg-emerald-500/15 text-emerald-400'
                      : 'bg-slate-500/15 text-slate-400'
                  }`}
                >
                  {patch.applied ? '已应用' : '候选'}
                </span>
              </button>
              {open && (
                <Suspense
                  fallback={
                    <div className="border-t border-slate-800 px-3 py-3 text-xs text-slate-500">
                      加载 Diff 视图…
                    </div>
                  }
                >
                  <PatchDiff patchId={patch.id} path={patch.filePath} diff={patch.diff} />
                </Suspense>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
