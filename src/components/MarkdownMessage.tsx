import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

/** Shared rendering for saved replies and streaming text; raw HTML stays disabled. */
export default function MarkdownMessage({ content }: { content: string }) {
  return <div className="markdown-message">
    <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={{
      a: ({ children, href }) => href
        ? <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>
        : <span>{children}</span>,
      table: ({ children }) => <div className="markdown-table"><table>{children}</table></div>,
      img: ({ alt }) => <span className="markdown-image">{alt ? `图片：${alt}` : '图片'}</span>,
    }}>{content}</ReactMarkdown>
  </div>
}
