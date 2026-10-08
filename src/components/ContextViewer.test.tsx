import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import ContextViewer from './ContextViewer'
import { useTaskStore } from '@/stores/task'

describe('ContextViewer', () => {
  beforeEach(() => useTaskStore.getState().reset())

  it('renders empty context and highlights task keywords in selected files', () => {
    const { rerender } = render(<ContextViewer />)
    expect(screen.getByText('上下文 · 暂无')).toBeInTheDocument()
    act(() => useTaskStore.setState({
      currentTask: { id: 1, projectId: 1, prompt: '修复 login bug', status: 'planning', intent: null, plan: null, createdAt: 0, finishedAt: null, durationMs: null, tokenInput: 0, tokenOutput: 0, cost: null },
      context: [{ path: 'src/auth.ts', content: 'const login = true\nconst other = false', reason: '命中符号', tokens: 8 }],
    }))
    rerender(<ContextViewer />)
    expect(screen.getByText('上下文 · 1 个文件 · 8 tokens')).toBeInTheDocument()
    expect(screen.getByText('src/auth.ts')).toBeInTheDocument()
    expect(screen.getByText('命中符号')).toBeInTheDocument()
    const line = screen.getByText('const login = true')
    expect(line.parentElement).toHaveClass('text-sky-200')
    const button = screen.getByRole('button', { name: /src\/auth.ts/ })
    fireEvent.click(button)
    expect(screen.queryByText('const login = true')).not.toBeInTheDocument()
    fireEvent.click(button)
    expect(screen.getByText('const login = true')).toBeInTheDocument()
  })
})

