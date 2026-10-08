import { act, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DiffEditor } from '@monaco-editor/react'

import PatchDiff from './PatchDiff'
import { patch } from '@/test/fixtures'
import { useThemeStore } from '@/stores/theme'

vi.mock('@/lib/monaco', () => ({}))
vi.mock('@monaco-editor/react', () => ({
  DiffEditor: vi.fn(() => <div data-testid="diff-editor" />),
}))
const mockedEditor = vi.mocked(DiffEditor)

describe('PatchDiff', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useThemeStore.setState({ theme: 'light' })
  })

  it('reconstructs unified diff sides and passes readonly options to editor', () => {
    render(<PatchDiff patchId={patch.id} path={patch.filePath} diff={patch.diff} />)
    const props = mockedEditor.mock.calls[0][0]
    expect(props.original).toBe('old')
    expect(props.modified).toBe('new')
    expect(props.language).toBe('typescript')
    expect(props.options).toMatchObject({ readOnly: true, renderSideBySide: true })
    expect(props.originalModelPath).toBe('xaim-patch://9/original')
    expect(props.modifiedModelPath).toBe('xaim-patch://9/modified')
    expect(props.theme).toBe('vs')
  })

  it('renders insertions without introducing blank lines or a false leading gap', () => {
    render(<PatchDiff patchId={1} path="a.ts" diff={'@@ -1,2 +1,3 @@\n first\n+inserted\n last'} />)
    expect(mockedEditor.mock.calls[0][0].original).toBe('first\nlast')
    expect(mockedEditor.mock.calls[0][0].modified).toBe('first\ninserted\nlast')
  })

  it('updates an already mounted editor when the theme changes', () => {
    render(<PatchDiff patchId={patch.id} path={patch.filePath} diff={patch.diff} />)
    act(() => useThemeStore.setState({ theme: 'dark' }))
    expect(mockedEditor.mock.calls.at(-1)?.[0].theme).toBe('vs-dark')
    act(() => useThemeStore.setState({ theme: 'light' }))
    expect(mockedEditor.mock.calls.at(-1)?.[0].theme).toBe('vs')
  })

  it('shows binary state instead of mounting editor', () => {
    render(<PatchDiff patchId={1} path="data.bin" diff="Binary files a/data.bin and b/data.bin differ" />)
    expect(screen.getByText('二进制文件，无法渲染 Diff。')).toBeInTheDocument()
    expect(mockedEditor).not.toHaveBeenCalled()
  })

  it('caps oversized diff and falls back to plaintext for unknown extensions', () => {
    render(<PatchDiff patchId={1} path="data.unknown" diff={'+line\n'.repeat(5001)} />)
    expect(screen.getByText(/仅渲染前 5000 行/)).toBeInTheDocument()
    expect(mockedEditor.mock.calls[0][0]).toMatchObject({ height: 480, language: 'plaintext' })
  })
})
