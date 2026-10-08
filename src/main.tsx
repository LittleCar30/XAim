import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import App from './App'
import { useConfigStore } from '@/stores/config'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { useUiStore } from '@/stores/ui'
import '@/styles/globals.css'
import { isPreview } from '@/lib/preview-mode'

// 仅开发构建暴露调试句柄：浏览器 dev（无 Tauri 后端）下可注入假数据/检查状态；
// import.meta.env.DEV 被静态替换，生产构建中此分支被剪除。
if (import.meta.env.DEV) {
  const devWindow = window as unknown as Record<string, unknown>
  devWindow.__xaim = {
    useProjectStore,
    useTaskStore,
    useConfigStore,
    useUiStore,
  }
}

async function mount() {
  if (import.meta.env.DEV && isPreview) {
    const { setupPreview } = await import('@/dev/preview')
    setupPreview()
  }
  createRoot(document.getElementById('root') as HTMLElement).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}

void mount()
