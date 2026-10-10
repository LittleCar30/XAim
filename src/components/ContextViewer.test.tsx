import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import ContextViewer from './ContextViewer'
import { useTaskStore } from '@/stores/task'
import { useConversationStore } from '@/stores/conversation'
import { resetStores, task } from '@/test/fixtures'

describe('ContextViewer', () => {
  beforeEach(resetStores)

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

  it('restores the file excerpts actually read in a historical task', () => {
    useConversationStore.setState({ selectedId: task.id, record: { task, patches: [], verifies: [], steps: [{
      id: 1, taskId: task.id, stepType: 'event', createdAt: 0,
      content: JSON.stringify({ type: 'toolFinished', taskId: task.id, callId: 'read', result: {
        name: 'read_file', success: true, output: JSON.stringify({ path: 'README.md', content: 'Saved project context' }), error: null,
      } }),
    }] } })
    render(<ContextViewer />)
    expect(screen.getByText('README.md')).toBeInTheDocument()
    expect(screen.getByText('Saved project context')).toBeInTheDocument()
  })

  it('shows successful batch excerpts in live and historical conversations', () => {
    const result = { name: 'read_files', success: true, error: null, durationMs: 0, output: JSON.stringify({ files: [
      { path: 'src/a.ts', hash: 'a', content: 'const alpha = 1' },
      { path: 'missing.ts', error: 'not found' },
      { path: 'src/b.ts', hash: 'b', content: 'const beta = 2' },
    ] }) }
    act(() => useTaskStore.getState().handleEvent({ type: 'toolFinished', taskId: task.id, callId: 'batch', result }))
    const { rerender } = render(<ContextViewer />)
    expect(screen.getByText('src/a.ts')).toBeInTheDocument()
    expect(screen.getByText('src/b.ts')).toBeInTheDocument()
    expect(screen.queryByText('missing.ts')).not.toBeInTheDocument()
    expect(useTaskStore.getState().context).toHaveLength(2)
    act(() => useConversationStore.setState({ selectedId: task.id, record: { task, patches: [], verifies: [], steps: [{
      id: 1, taskId: task.id, stepType: 'event', createdAt: 0,
      content: JSON.stringify({ type: 'toolFinished', taskId: task.id, callId: 'batch', result }),
    }] } }))
    rerender(<ContextViewer />)
    expect(screen.getByText('src/a.ts')).toBeInTheDocument()
    expect(screen.getByText('src/b.ts')).toBeInTheDocument()
    expect(screen.queryByText('missing.ts')).not.toBeInTheDocument()
  })
})

