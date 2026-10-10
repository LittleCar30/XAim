import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import MarkdownMessage from './MarkdownMessage'

it('renders headings, GFM tables, task lists and code while preserving code whitespace', () => {
  const { container } = render(<MarkdownMessage content={'## 检查结果\n\n| 文件 | 状态 |\n| --- | --- |\n| main.ts | 通过 |\n\n- [x] 测试完成\n\n```ts\nconst value = 1\n  console.log(value)\n```'} />)
  expect(screen.getByRole('heading', { name: '检查结果', level: 2 })).toBeInTheDocument()
  expect(screen.getByRole('table')).toHaveTextContent('main.ts')
  expect(screen.getByRole('checkbox')).toBeChecked()
  expect(screen.getByRole('checkbox')).toBeDisabled()
  expect(container.querySelector('pre code')?.textContent).toBe('const value = 1\n  console.log(value)\n')
})

it('keeps raw HTML and unsafe links inert and does not fetch Markdown images', () => {
  const { container } = render(<MarkdownMessage content={'<script>alert(1)</script>\n\n<a href="https://example.test">HTML link</a>\n\n[unsafe](javascript:alert%281%29)\n\n[文档](https://example.test/docs)\n\n![架构图](https://example.test/image.png)'} />)
  expect(container.querySelector('script')).toBeNull()
  expect(container.querySelector('img')).toBeNull()
  expect(screen.queryByRole('link', { name: 'unsafe' })).not.toBeInTheDocument()
  expect(screen.queryByRole('link', { name: 'HTML link' })).not.toBeInTheDocument()
  expect(screen.getByRole('link', { name: '文档' })).toHaveAttribute('rel', 'noopener noreferrer')
  expect(screen.getByText('图片：架构图')).toBeInTheDocument()
})
