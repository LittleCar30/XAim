import type { ContextFile, ToolResult } from '@/types'

/** Use the same actual read results in live and restored conversation views. */
export function toolContext(result: ToolResult): ContextFile[] {
  if (!result.success || !['read_file', 'read_files', 'read_symbol'].includes(result.name)) return []
  try {
    const output = JSON.parse(result.output)
    const files: unknown[] = result.name === 'read_files' ? output.files : [output]
    if (!Array.isArray(files)) return []
    return files.flatMap((item) => {
      if (!item || typeof item !== 'object') return []
      const read = item as Record<string, unknown>
      if (typeof read.path !== 'string' || typeof read.content !== 'string' || read.error) return []
      return [{ path: read.path, content: read.content, reason: '模型读取', tokens: Math.ceil(read.content.length / 3) }]
    })
  } catch {
    return []
  }
}
