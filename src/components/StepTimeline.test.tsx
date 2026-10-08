import { act, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import StepTimeline from './StepTimeline'
import { useTaskStore } from '@/stores/task'

describe('StepTimeline', () => {
  beforeEach(() => useTaskStore.getState().reset())

  it('renders empty state and status labels for each step', () => {
    const { rerender } = render(<StepTimeline />)
    expect(screen.getByText('暂无步骤')).toBeInTheDocument()
    act(() => useTaskStore.setState({ steps: [
      { id: 1, taskId: 1, stepType: '解析', content: 'done', createdAt: 0 },
      { id: 2, taskId: 1, stepType: '验证', content: 'timeout', createdAt: 0 },
      { id: 3, taskId: 1, stepType: '生成', content: 'running', createdAt: 0 },
    ] }))
    rerender(<StepTimeline />)
    expect(screen.getByRole('list', { name: '任务步骤' })).toBeInTheDocument()
    expect(screen.getByText('解析')).toBeInTheDocument()
    expect(screen.getByText('验证')).toBeInTheDocument()
    expect(screen.getByText('生成')).toBeInTheDocument()
    expect(screen.getByText('完成')).toBeInTheDocument()
    expect(screen.getByText('失败')).toBeInTheDocument()
    expect(screen.getByText('进行中')).toBeInTheDocument()
    expect(screen.getByText('当前')).toBeInTheDocument()
  })
})

