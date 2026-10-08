import { DiffEditor } from '@monaco-editor/react'
// 先于 DiffEditor 执行：把 loader 指向本地 monaco（含 worker），保证离线可用。
import '@/lib/monaco'
import { useMemo } from 'react'
import { useThemeStore } from '@/stores/theme'

export interface PatchDiffProps {
  /** 补丁 id（用于模型 URI 缓存键）。 */
  patchId: number
  /** 补丁目标文件（决定语法高亮语言）。 */
  path: string
  /** unified diff 文本（与 Patch.diff 一致）。 */
  diff: string
}

/** Monaco DiffEditor 的行高与高度自适应参数。 */
const LINE_HEIGHT = 16
const MIN_HEIGHT = 120
const MAX_HEIGHT = 480
/** 超大补丁的渲染上限：超出部分截断，防止 DOM/内存膨胀。 */
const MAX_DIFF_LINES = 5000

/** 扩展名 → Monaco 语言 id（Monarch 语法高亮；未识别退化为纯文本）。 */
const LANG_BY_EXT: Record<string, string> = {
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  json: 'json', rs: 'rust', py: 'python', go: 'go', md: 'markdown',
  css: 'css', scss: 'scss', less: 'less', html: 'html', htm: 'html',
  yml: 'yaml', yaml: 'yaml', toml: 'ini', ini: 'ini', sh: 'shell', bash: 'shell',
  xml: 'xml', sql: 'sql', java: 'java', c: 'c', h: 'c', cpp: 'cpp', hpp: 'cpp',
  cs: 'csharp', rb: 'ruby', php: 'php',
}

function languageOf(path: string): string {
  const dot = path.lastIndexOf('.')
  if (dot <= 0) return 'plaintext'
  return LANG_BY_EXT[path.slice(dot + 1).toLowerCase()] ?? 'plaintext'
}

interface Reconstructed {
  original: string
  modified: string
  /** 双侧行数的上限。 */
  lines: number
  truncated: boolean
  binary: boolean
}

/**
 * 从 unified diff 重建 DiffEditor 所需的双侧文本：
 * 上下文行进两侧，+ 行进 modified，- 行进 original；hunk 间被略去的未变更区间
 * 以单行占位（双侧同文）保持对齐，占位文案带被略去行数。超长截断。
 */
function reconstruct(diff: string): Reconstructed {
  const original: string[] = []
  const modified: string[] = []
  let count = 0
  let truncated = false
  let binary = false
  let oLine = 1
  let mLine = 1

  const push = (o: string | null, m: string | null) => {
    if (count >= MAX_DIFF_LINES) {
      truncated = true
      return
    }
    if (o !== null) original.push(o)
    if (m !== null) modified.push(m)
    count++
  }
  const pushGap = (n: number) => {
    if (n > 0) push(`…… 略去 ${n} 行未变更内容 ……`, `…… 略去 ${n} 行未变更内容 ……`)
  }
  const isMeta = (l: string) =>
    l.startsWith('diff --git') ||
    l.startsWith('index ') ||
    l.startsWith('--- ') ||
    l.startsWith('+++ ') ||
    l.startsWith('new file mode') ||
    l.startsWith('deleted file mode') ||
    l.startsWith('old mode') ||
    l.startsWith('new mode') ||
    l.startsWith('rename ') ||
    l.startsWith('similarity index') ||
    l.startsWith('\\ No newline')

  for (const line of diff.split('\n')) {
    if (isMeta(line)) continue
    if (line.startsWith('Binary files') || line.startsWith('GIT binary patch')) {
      binary = true
      continue
    }
    if (line.startsWith('@@')) {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line)
      if (m) {
        const oStart = parseInt(m[1], 10)
        const mStart = parseInt(m[2], 10)
        pushGap(Math.max(oStart - oLine, mStart - mLine))
        oLine = oStart
        mLine = mStart
      }
      continue
    }
    if (line.startsWith('+')) {
      push(null, line.slice(1))
      mLine++
    } else if (line.startsWith('-')) {
      push(line.slice(1), null)
      oLine++
    } else {
      // 上下文行（含空行）；无法归类的行按上下文处理。
      const text = line.startsWith(' ') ? line.slice(1) : line
      push(text, text)
      oLine++
      mLine++
    }
  }

  return {
    original: original.join('\n'),
    modified: modified.join('\n'),
    lines: Math.max(original.length, modified.length),
    truncated,
    binary,
  }
}

/** 单个补丁的 Monaco Diff 视图：readOnly、双栏、高度按行数自适应（封顶防溢出）。 */
export default function PatchDiff({ patchId, path, diff }: PatchDiffProps) {
  const theme = useThemeStore((s) => s.theme)
  const rec = useMemo(() => reconstruct(diff), [diff])
  const language = useMemo(() => languageOf(path), [path])
  const height = Math.min(Math.max(rec.lines * LINE_HEIGHT + 12, MIN_HEIGHT), MAX_HEIGHT)

  if (rec.binary) {
    return (
      <div className="border-t border-slate-800 px-3 py-3 text-xs text-slate-500">
        二进制文件，无法渲染 Diff。
      </div>
    )
  }

  return (
    <div className="border-t border-slate-800">
      {rec.truncated && (
        <p className="px-3 pt-2 text-[11px] text-amber-400/90">
          补丁超过 {MAX_DIFF_LINES} 行，仅渲染前 {MAX_DIFF_LINES} 行。
        </p>
      )}
      <DiffEditor
        original={rec.original}
        modified={rec.modified}
        language={language}
        originalModelPath={`xaim-patch://${patchId}/original`}
        modifiedModelPath={`xaim-patch://${patchId}/modified`}
        keepCurrentOriginalModel
        keepCurrentModifiedModel
        theme={theme === 'dark' ? 'vs-dark' : 'vs'}
        height={height}
        onMount={(editor, monacoInstance) => {
          // DEV 调试句柄：浏览器 dev 下可检查编辑器选项/状态，生产构建剪除。
          if (import.meta.env.DEV) {
            const devWindow = window as unknown as Record<string, unknown>
            devWindow.__monacoDiff = { editor, monaco: monacoInstance }
          }
        }}
        loading={
          <div className="flex h-24 items-center justify-center text-xs text-slate-500">
            加载 Diff 视图…
          </div>
        }
        options={{
          readOnly: true,
          renderSideBySide: true,
          automaticLayout: true,
          scrollBeyondLastLine: false,
          renderOverviewRuler: false,
          diffWordWrap: 'off',
          fontSize: 12,
          lineHeight: LINE_HEIGHT,
          lineNumbersMinChars: 3,
          minimap: { enabled: false },
          folding: false,
          renderLineHighlight: 'none',
          padding: { top: 4, bottom: 4 },
          scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
        }}
      />
    </div>
  )
}
