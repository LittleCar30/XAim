import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import VerifyPanel from './VerifyPanel'

describe('VerifyPanel', () => {
  it('renders empty state and grouped pass count', () => {
    const { rerender } = render(<VerifyPanel verifies={[]} />)
    expect(screen.getByText('验证 · 暂无')).toBeInTheDocument()
    rerender(<VerifyPanel verifies={[
      { kind: 'test', passed: true, output: 'ok', durationMs: 12 },
      { kind: 'lint', passed: false, output: 'line 2\nerror', durationMs: 1200 },
      { kind: 'unknown', passed: true, output: '', durationMs: 0 },
    ]} />)
    expect(screen.getByText('验证 · 2/3 通过')).toBeInTheDocument()
    expect(screen.getByText('测试')).toBeInTheDocument()
    expect(screen.getByText('Lint')).toBeInTheDocument()
    expect(screen.getByText('其他')).toBeInTheDocument()
    expect(screen.getAllByText('通过')).toHaveLength(2)
    expect(screen.getByText(/未通过/)).toBeInTheDocument()
  })

  it('expands and collapses output details', () => {
    render(<VerifyPanel verifies={[{ kind: 'build', passed: false, output: 'compiler failed', durationMs: 1000 }]} />)
    const row = screen.getByRole('button', { name: /未通过/ })
    expect(screen.queryByText('compiler failed')).not.toBeInTheDocument()
    fireEvent.click(row)
    expect(screen.getByText('compiler failed')).toBeInTheDocument()
    expect(row).toHaveAttribute('aria-expanded', 'true')
    fireEvent.click(row)
    expect(screen.queryByText('compiler failed')).not.toBeInTheDocument()
  })
})

