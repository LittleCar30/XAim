import { useEffect, useState } from 'react'

import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'

/** 预览最多加载的行数（read_file 的 end 为 1-based 闭区间行号）。 */
const PREVIEW_MAX_LINES = 2000

/** 把 IPC 抛出值转为展示消息（Tauri 命令失败 reject string）。 */
function toMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}

export interface FilePreviewProps {
  /** 项目内相对路径（与 FileNode.path 一致）。 */
  path: string
  onClose: () => void
}

/** 文件预览内容区：加载项目内文件文本，带行号与截断提示。头部由右栏容器提供。 */
export default function FilePreview({ path, onClose }: FilePreviewProps) {
  const projectId = useProjectStore((s) => s.currentProject?.id)
  const [content, setContent] = useState<string | null>(null)
  const [truncated, setTruncated] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setContent(null)
    setTruncated(false)
    setError(null)
    if (projectId == null) {
      setError('未打开项目，无法读取文件。')
      return
    }
    let cancelled = false
    ipc
      .readFile(projectId, path, 1, PREVIEW_MAX_LINES)
      .then((text) => {
        if (cancelled) return
        setContent(text)
        setTruncated(text.split('\n').length >= PREVIEW_MAX_LINES)
      })
      .catch((e) => {
        if (!cancelled) setError(toMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [projectId, path])

  if (error) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-start gap-2 overflow-auto p-3">
        <p className="text-xs text-rose-400">{error}</p>
        <button
          type="button"
          onClick={onClose}
          className="rounded-md border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:bg-slate-800"
        >
          返回上下文
        </button>
      </div>
    )
  }

  if (content === null) {
    return <p className="min-h-0 flex-1 p-3 text-xs text-slate-500">加载文件…</p>
  }

  const lines = content.split('\n')

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto p-3">
        <pre className="font-mono text-[11px] leading-4 text-slate-300">
          {lines.map((line, i) => (
            <div key={i} className="flex">
              <span className="w-9 shrink-0 select-none pr-2 text-right text-slate-600">
                {i + 1}
              </span>
              <span className="whitespace-pre">{line === '' ? ' ' : line}</span>
            </div>
          ))}
        </pre>
      </div>
      {truncated && (
        <p className="shrink-0 border-t border-slate-800 px-3 py-1 text-[11px] text-slate-500">
          仅显示前 {PREVIEW_MAX_LINES} 行
        </p>
      )}
    </div>
  )
}
