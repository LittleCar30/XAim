import type { Config, FileNode, Patch, Project, Task } from '@/types'
import { useConfigStore } from '@/stores/config'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { useUiStore } from '@/stores/ui'
import { useConversationStore } from '@/stores/conversation'

export const project: Project = { id: 7, path: 'D:/repo', name: 'repo', createdAt: 100, lastOpenedAt: 200 }
export const task: Task = {
  id: 42, projectId: 7, prompt: 'fix login', status: 'done', intent: 'repair auth', plan: null,
  createdAt: 1000, finishedAt: 3000, durationMs: 2000, tokenInput: 1200, tokenOutput: 80, cost: 0.003,
}
export const config: Config = {
  model: { provider: 'openai', name: 'fast', apiKey: null, baseUrl: null, temperature: 0.2, maxTokens: 1024 },
  router: { fastModel: 'fast', strongModel: 'strong' },
}
export const patch: Patch = {
  id: 9, taskId: 42, filePath: 'src/auth.ts', applied: false, score: 0.9, createdAt: 1000,
  diff: '--- a/src/auth.ts\n+++ b/src/auth.ts\n@@ -1 +1 @@\n-old\n+new',
}
export const tree: FileNode = { name: 'repo', path: '.', isDir: true, size: null, children: [] }

export function resetStores() {
  useTaskStore.getState().reset()
  useConversationStore.getState().clear()
  useConversationStore.setState({ projectId: null })
  useProjectStore.setState({ currentProject: null, fileTree: null, recentProjects: [], loading: false })
  useConfigStore.setState({ config: null, modelConfig: null, loading: false })
  useUiStore.setState({ sidebarOpen: true, rightPanelOpen: false, previewPath: null, detailTab: 'context' })
}

