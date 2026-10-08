import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import ToolLog from './ToolLog'
import { useTaskStore } from '@/stores/task'

describe('ToolLog', () => {
  beforeEach(() => useTaskStore.getState().reset())

  it('renders empty and expands formatted args and result', () => {
    const { rerender } = render(<ToolLog />)
    expect(screen.getByText('暂无工具调用')).toBeInTheDocument()
    act(() => useTaskStore.setState({ toolLogs: [{
      taskId: 1, tool: 'search', args: '{"query":"login"}', createdAt: 0,
      result: { name: 'search', success: true, output: 'src/auth.ts:2', error: null, durationMs: 12 },
    }] }))
    rerender(<ToolLog />)
    expect(screen.getByText('工具调用 · 1')).toBeInTheDocument()
    const button = screen.getByRole('button', { name: /search/ })
    fireEvent.click(button)
    expect(screen.getByText(/"query": "login"/)).toBeInTheDocument()
    expect(screen.getByText('执行成功')).toBeInTheDocument()
    expect(screen.getByText('src/auth.ts:2')).toBeInTheDocument()
    fireEvent.click(button)
    expect(screen.queryByText('执行成功')).not.toBeInTheDocument()
  })

  it('shows failed results and missing result state', () => {
    useTaskStore.setState({ toolLogs: [
      { taskId: 1, tool: 'run_command', args: 'not-json', createdAt: 0, result: { name: 'run_command', success: false, output: '', error: 'exit 1', durationMs: 1000 } },
      { taskId: 1, tool: 'read_file', args: '{}', createdAt: 0 },
    ] })
    render(<ToolLog />)
    fireEvent.click(screen.getByRole('button', { name: /run_command/ }))
    expect(screen.getByText('执行失败')).toBeInTheDocument()
    expect(screen.getByText('exit 1')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /read_file/ }))
    expect(screen.getByText('结果未上报')).toBeInTheDocument()
  })
})

