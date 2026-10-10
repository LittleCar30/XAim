import { useDeferredValue, useEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react'

import { useProjectStore } from '@/stores/project'
import { useUiStore } from '@/stores/ui'
import { useTaskStore } from '@/stores/task'
import { useConversationStore } from '@/stores/conversation'
import type { FileNode } from '@/types'

/** 行高与上下越界行数：虚拟滚动的常量。 */
const ROW_HEIGHT = 24
const OVERSCAN = 8

/** 扁平化后的一行：节点 + 深度 + 父目录所在行（键盘 Left 跳转用）。 */
interface TreeRow {
  node: FileNode
  depth: number
  parentIndex: number
}

/**
 * 把可见树扁平化为行数组。普通模式按 expanded 集合展开目录；
 * 搜索模式忽略折叠状态，保留自身或子树命中的节点（无命中子树整段回退）。
 */
function flatten(root: FileNode | null, expanded: ReadonlySet<string>, rawQuery: string): TreeRow[] {
  const rows: TreeRow[] = []
  if (!root) return rows
  const q = rawQuery.trim().toLowerCase()
  const matches = (n: FileNode) => q !== '' && n.name.toLowerCase().includes(q)

  const walkNormal = (node: FileNode, depth: number, parentIndex: number) => {
    rows.push({ node, depth, parentIndex })
    if (node.isDir && expanded.has(node.path)) {
      const myIndex = rows.length - 1
      for (const child of node.children) walkNormal(child, depth + 1, myIndex)
    }
  }

  const walkSearch = (node: FileNode, depth: number, parentIndex: number): boolean => {
    if (!node.isDir) {
      if (!matches(node)) return false
      rows.push({ node, depth, parentIndex })
      return true
    }
    const myIndex = rows.length
    rows.push({ node, depth, parentIndex })
    const base = rows.length
    let kept = matches(node)
    for (const child of node.children) {
      if (walkSearch(child, depth + 1, myIndex)) kept = true
    }
    if (!kept) rows.length = base
    return kept
  }

  // 根节点（path 为 "."）本身不渲染，从其子级开始。
  for (const child of root.children) {
    if (q !== '') walkSearch(child, 0, -1)
    else walkNormal(child, 0, -1)
  }
  return rows
}

/** 扩展名 → 徽标（文案 + 配色）。未识别的扩展名退化为灰色前 4 位大写。 */
const FILE_BADGES: Record<string, { label: string; cls: string }> = {
  ts: { label: 'TS', cls: 'bg-sky-500/15 text-sky-400' },
  tsx: { label: 'TSX', cls: 'bg-sky-500/15 text-sky-400' },
  js: { label: 'JS', cls: 'bg-yellow-500/15 text-yellow-400' },
  jsx: { label: 'JSX', cls: 'bg-yellow-500/15 text-yellow-400' },
  mjs: { label: 'JS', cls: 'bg-yellow-500/15 text-yellow-400' },
  cjs: { label: 'JS', cls: 'bg-yellow-500/15 text-yellow-400' },
  json: { label: 'JSON', cls: 'bg-amber-500/15 text-amber-400' },
  rs: { label: 'RS', cls: 'bg-orange-500/15 text-orange-400' },
  py: { label: 'PY', cls: 'bg-emerald-500/15 text-emerald-400' },
  go: { label: 'GO', cls: 'bg-cyan-500/15 text-cyan-400' },
  md: { label: 'MD', cls: 'bg-slate-500/15 text-slate-400' },
  css: { label: 'CSS', cls: 'bg-violet-500/15 text-violet-400' },
  scss: { label: 'SCSS', cls: 'bg-pink-500/15 text-pink-400' },
  less: { label: 'LESS', cls: 'bg-violet-500/15 text-violet-400' },
  html: { label: 'HTML', cls: 'bg-orange-500/15 text-orange-400' },
  toml: { label: 'TOML', cls: 'bg-slate-500/15 text-slate-400' },
  yaml: { label: 'YML', cls: 'bg-slate-500/15 text-slate-400' },
  yml: { label: 'YML', cls: 'bg-slate-500/15 text-slate-400' },
  svg: { label: 'SVG', cls: 'bg-fuchsia-500/15 text-fuchsia-400' },
}

function fileBadge(name: string): { label: string; cls: string } {
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot === name.length - 1) {
    return { label: 'FILE', cls: 'bg-slate-500/15 text-slate-400' }
  }
  const ext = name.slice(dot + 1).toLowerCase()
  return (
    FILE_BADGES[ext] ?? { label: ext.slice(0, 4).toUpperCase(), cls: 'bg-slate-500/15 text-slate-400' }
  )
}

function FileBadge({ name }: { name: string }) {
  const badge = fileBadge(name)
  return (
    <span className={`rounded px-1 text-center text-[9px] font-bold leading-4 ${badge.cls}`}>
      {badge.label}
    </span>
  )
}

function formatSize(size: number): string {
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
  return `${(size / (1024 * 1024)).toFixed(1)} MB`
}

/** 搜索命中高亮：命中片段提亮，其余保持原色。 */
function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim().toLowerCase()
  if (q === '') return <>{text}</>
  const lower = text.toLowerCase()
  const parts: ReactNode[] = []
  let from = 0
  let at = lower.indexOf(q)
  while (at !== -1) {
    parts.push(text.slice(from, at))
    parts.push(
      <mark key={at} className="bg-transparent text-sky-300">
        {text.slice(at, at + q.length)}
      </mark>,
    )
    from = at + q.length
    at = lower.indexOf(q, from)
  }
  parts.push(text.slice(from))
  return <>{parts}</>
}

function SearchIcon() {
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
    >
      <circle cx="11" cy="11" r="8" />
      <polyline points="21 21 16.65 16.65" />
    </svg>
  )
}

function FolderIcon({ open }: { open: boolean }) {
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
      className={open ? 'text-sky-400' : 'text-slate-400'}
    >
      {open ? (
        <>
          <path d="M6 14l1.5-4.5A2 2 0 0 1 9.4 8H21l-2.1 7.2a2 2 0 0 1-1.9 1.8H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v1" />
        </>
      ) : (
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
      )}
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

/** 左侧文件树：虚拟滚动 + 目录折叠 + 搜索 + 键盘导航 + 点选预览。 */
export default function FileTree() {
  const projectTree = useProjectStore((s) => s.fileTree)
  const livePatches = useTaskStore((s) => s.patches)
  const liveTask = useTaskStore((s) => s.currentTask)
  const { selectedId, record } = useConversationStore()
  const task = selectedId !== null ? record?.task : liveTask
  const patches = useMemo(() => selectedId !== null ? record?.patches ?? [] : livePatches, [selectedId, record, livePatches])
  const pendingPaths = useMemo(() => new Set(task && ['waitingConfirm', 'failed'].includes(task.status)
    ? patches.filter((p) => !p.applied && !p.diff.includes('+++ /dev/null')).map((p) => p.filePath) : []), [patches, task])
  const fileTree = useMemo(() => {
    if (!projectTree || pendingPaths.size === 0) return projectTree
    const tree = structuredClone(projectTree)
    for (const path of pendingPaths) {
      const parts = path.split('/')
      let parent = tree
      for (let i = 0; i < parts.length; i++) {
        const name = parts[i]
        let child = parent.children.find((node) => node.name === name)
        if (!child) {
          child = { name, path: parts.slice(0, i + 1).join('/'), isDir: i < parts.length - 1, size: null, children: [] }
          parent.children.push(child)
        }
        parent = child
      }
    }
    return tree
  }, [projectTree, pendingPaths])
  const previewPath = useUiStore((s) => s.previewPath)
  const setPreviewPath = useUiStore((s) => s.setPreviewPath)

  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query)
  const searching = deferredQuery.trim() !== ''

  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const [activeIndex, setActiveIndex] = useState(0)

  const scrollRef = useRef<HTMLDivElement | null>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportH, setViewportH] = useState(360)

  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const measure = () => setViewportH(el.clientHeight)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const rows = useMemo(
    () => flatten(fileTree, expanded, deferredQuery),
    [fileTree, expanded, deferredQuery],
  )

  // 虚拟窗口：只渲染视口内（含越界余量）的行。
  const totalHeight = rows.length * ROW_HEIGHT
  const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
  const end = Math.min(rows.length, Math.ceil((scrollTop + viewportH) / ROW_HEIGHT) + OVERSCAN)
  const visible = rows.slice(start, end)

  // 键盘焦点行（越界保护）。
  const activeIdx = rows.length === 0 ? -1 : Math.min(activeIndex, rows.length - 1)

  const isExpanded = (row: TreeRow) => (searching ? true : expanded.has(row.node.path))

  const toggleDir = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  const activate = (row: TreeRow, index: number) => {
    setActiveIndex(index)
    if (row.node.isDir) {
      if (!searching) toggleDir(row.node.path)
    } else {
      setPreviewPath(row.node.path)
    }
  }

  // 键盘焦点行滚入视口。
  useEffect(() => {
    const el = scrollRef.current
    if (activeIdx < 0 || !el) return
    const top = activeIdx * ROW_HEIGHT
    if (top < el.scrollTop) el.scrollTop = top
    else if (top + ROW_HEIGHT > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_HEIGHT - el.clientHeight
    }
  }, [activeIdx])

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (activeIdx < 0 || rows.length === 0) return
    const row = rows[activeIdx]
    let idx = activeIdx
    switch (e.key) {
      case 'ArrowDown':
        idx = Math.min(activeIdx + 1, rows.length - 1)
        break
      case 'ArrowUp':
        idx = Math.max(activeIdx - 1, 0)
        break
      case 'ArrowRight':
        if (row.node.isDir && !isExpanded(row)) toggleDir(row.node.path)
        else idx = Math.min(activeIdx + 1, rows.length - 1)
        break
      case 'ArrowLeft':
        if (row.node.isDir && isExpanded(row) && !searching) toggleDir(row.node.path)
        else if (row.parentIndex >= 0) idx = row.parentIndex
        break
      case 'Home':
        idx = 0
        break
      case 'End':
        idx = rows.length - 1
        break
      case 'Enter':
      case ' ':
        activate(row, activeIdx)
        break
      default:
        return
    }
    e.preventDefault()
    setActiveIndex(idx)
  }

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-slate-800 bg-slate-900/50">
      <div className="border-b border-slate-800 p-2">
        <div className="relative">
          <span className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-500">
            <SearchIcon />
          </span>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索文件…"
            aria-label="搜索文件"
            className="w-full rounded-md bg-slate-950 py-1.5 pl-7 pr-6 text-xs text-slate-100 outline-none ring-1 ring-slate-800 placeholder:text-slate-600 focus:ring-sky-500"
          />
          {query !== '' && (
            <button
              type="button"
              onClick={() => setQuery('')}
              aria-label="清除搜索"
              title="清除"
              className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-slate-500 hover:text-slate-300"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                aria-hidden="true"
              >
                <polyline points="18 6 6 18 6 6 18 18" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {!fileTree ? (
        <p className="p-3 text-xs text-slate-500">未打开项目</p>
      ) : (
        <div
          ref={scrollRef}
          role="tree"
          aria-label="文件树"
          aria-activedescendant={activeIdx >= 0 ? `xaim-file-row-${activeIdx}` : undefined}
          tabIndex={0}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          onKeyDown={onKeyDown}
          className="min-h-0 flex-1 overflow-auto py-1 outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-sky-500/60"
        >
          <div style={{ height: totalHeight, paddingTop: start * ROW_HEIGHT }}>
            {rows.length === 0 ? (
              <p className="p-3 text-xs text-slate-500">
                {searching ? '无匹配文件' : '（空目录）'}
              </p>
            ) : (
              visible.map((row, i) => {
                const idx = start + i
                const isActive = idx === activeIdx
                const isSelected = !row.node.isDir && row.node.path === previewPath
                const open = isExpanded(row)
                const title = row.node.isDir
                  ? row.node.path
                  : `${row.node.path}（${formatSize(row.node.size ?? 0)}）`
                return (
                  <div
                    key={row.node.path}
                    id={`xaim-file-row-${idx}`}
                    role="treeitem"
                    aria-level={row.depth + 1}
                    aria-selected={isSelected || undefined}
                    aria-expanded={row.node.isDir ? open : undefined}
                    title={title}
                    style={{ paddingLeft: 6 + row.depth * 12 }}
                    onClick={() => activate(row, idx)}
                    className={`flex h-6 cursor-pointer select-none items-center gap-1 pr-2 text-xs ${
                      isSelected
                        ? 'bg-slate-800 text-slate-100'
                        : 'text-slate-300 hover:bg-slate-800/50'
                    } ${isActive ? 'ring-1 ring-inset ring-sky-500/60' : ''}`}
                  >
                    {row.node.isDir ? (
                      <ChevronIcon open={open} />
                    ) : (
                      <span className="w-3 shrink-0" aria-hidden="true" />
                    )}
                    <span className="flex w-7 shrink-0 items-center justify-center">
                      {row.node.isDir ? (
                        <FolderIcon open={open} />
                      ) : (
                        <FileBadge name={row.node.name} />
                      )}
                    </span>
                    <span className="min-w-0 truncate">
                      <Highlight text={row.node.name} query={deferredQuery} />
                    </span>
                    {pendingPaths.has(row.node.path) && <span className="ml-auto shrink-0 text-[10px] text-amber-500">待应用</span>}
                  </div>
                )
              })
            )}
          </div>
        </div>
      )}
    </aside>
  )
}
