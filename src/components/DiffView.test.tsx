import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import DiffView from './DiffView'
import { patch } from '@/test/fixtures'

vi.mock('@/components/PatchDiff', () => ({
  default: ({ path }: { path: string }) => <div data-testid="patch-diff">{path}</div>,
}))

it('renders empty state and expands only the newest patch as patches arrive', async () => {
  const { rerender } = render(<DiffView patches={[]} />)
  expect(screen.getByText('候选补丁 · 暂无')).toBeInTheDocument()
  rerender(<DiffView patches={[patch]} />)
  expect(await screen.findByTestId('patch-diff')).toHaveTextContent('src/auth.ts')
  expect(screen.getByText('score 0.9')).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: /src\/auth.ts/ }))
  expect(screen.queryByTestId('patch-diff')).not.toBeInTheDocument()
  rerender(<DiffView patches={[patch, { ...patch, id: 10, filePath: 'other.ts', applied: true }]} />)
  expect(await screen.findByTestId('patch-diff')).toHaveTextContent('other.ts')
  expect(screen.getByText('已应用')).toBeInTheDocument()
  expect(screen.getAllByTestId('patch-diff')).toHaveLength(1)
})

it('keeps conversation changes collapsed until the user opens a file', async () => {
  render(<DiffView patches={[patch]} compact />)
  expect(screen.queryByTestId('patch-diff')).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: /src\/auth.ts/ }))
  expect(await screen.findByTestId('patch-diff')).toBeInTheDocument()
})
