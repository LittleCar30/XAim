import { useEffect, useState } from 'react'
import { Check, Clock3, Copy } from 'lucide-react'

import { formatDuration } from '@/lib/task-format'

interface MessageFooterProps {
  text: string
  label: '输入' | '输出'
  timestamp: number | null
  timeLabel?: string
  durationMs?: number | null
}

/** 消息正文的复制操作与时间信息，不将界面控件写入剪贴板。 */
export default function MessageFooter({ text, label, timestamp, timeLabel, durationMs }: MessageFooterProps) {
  const [copyState, setCopyState] = useState<'idle' | 'copying' | 'copied' | 'failed'>('idle')

  useEffect(() => { setCopyState('idle') }, [text])
  useEffect(() => {
    if (copyState !== 'copied') return
    const timer = setTimeout(() => setCopyState('idle'), 2000)
    return () => clearTimeout(timer)
  }, [copyState])

  const copy = async () => {
    setCopyState('copying')
    try {
      await navigator.clipboard.writeText(text)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
  }

  return <footer className="message-footer" aria-label={`${label}信息`}>
    <button type="button" onClick={() => void copy()} disabled={copyState === 'copying' || !text}
      aria-label={`复制${label}`} title={`复制${label}文本`}>
      {copyState === 'copied' ? <Check size={13} /> : <Copy size={13} />}
      <span>{copyState === 'copied' ? '已复制' : copyState === 'copying' ? '复制中…' : '复制'}</span>
    </button>
    <span role="status" className={copyState === 'failed' ? 'text-rose-400' : 'sr-only'}>
      {copyState === 'copied' ? `${label}已复制` : copyState === 'failed' ? '复制失败，请重试' : ''}
    </span>
    {timestamp != null && <time dateTime={new Date(timestamp).toISOString()} title={new Date(timestamp).toLocaleString('zh-CN')}>
      {timeLabel && `${timeLabel} `}{new Date(timestamp).toLocaleString('zh-CN', {
        year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false,
      })}
    </time>}
    {durationMs != null && <span className="message-duration"><Clock3 size={13} />总耗时 {formatDuration(Math.max(0, durationMs))}</span>}
  </footer>
}
