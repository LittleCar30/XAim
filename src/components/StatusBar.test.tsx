import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import StatusBar from './StatusBar'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { config, project, resetStores, task } from '@/test/fixtures'

vi.mock('@/services/ipc', () => ({ ipc: { getModelConfig: vi.fn() } }))
beforeEach(() => { resetStores(); vi.mocked(ipc.getModelConfig).mockResolvedValue(config.model) })
afterEach(() => vi.useRealTimers())

it('shows index, model, duration, tokens and cost', async () => {
  useProjectStore.setState({ currentProject: project })
  useTaskStore.setState({ currentTask: task })
  render(<StatusBar />)
  expect(await screen.findByText('fast')).toBeInTheDocument()
  expect(screen.getByText('已索引')).toBeInTheDocument()
  expect(screen.getByText('00:02')).toBeInTheDocument()
  expect(screen.getByText('↑1.2k ↓80 tok')).toBeInTheDocument()
  expect(screen.getByText('$0.0030')).toBeInTheDocument()
})

it('updates elapsed time for running tasks and changes indexing state', () => {
  vi.useFakeTimers()
  vi.setSystemTime(5000)
  useProjectStore.setState({ loading: true })
  useTaskStore.setState({ currentTask: { ...task, status: 'generating', durationMs: null, finishedAt: null } })
  render(<StatusBar />)
  expect(screen.getByText('索引中')).toBeInTheDocument()
  expect(screen.getByText('00:04')).toBeInTheDocument()
  act(() => vi.advanceTimersByTime(1000))
  expect(screen.getByText('00:05')).toBeInTheDocument()
})

it('shows the selected historical task usage instead of the live task', async () => {
  useTaskStore.setState({ currentTask: task })
  render(<StatusBar task={{ ...task, tokenInput: 500, tokenOutput: 30, cost: 0.001 }} />)
  expect(await screen.findByText('fast')).toBeInTheDocument()
  expect(screen.getByText('↑500 ↓30 tok')).toBeInTheDocument()
  expect(screen.queryByText('↑1.2k ↓80 tok')).not.toBeInTheDocument()
})
