import { invoke } from '@tauri-apps/api/core';
import type { Project, Task, TaskStep, Patch, FileNode, ModelConfig, Config, StorageInfo, RuntimeSettings, RuntimeOverrides, ExecutionSettings, ExecutionMode, GitNetwork, PermissionRequest, PermissionDecision } from '../types';

/**
 * Tauri IPC 封装层：前端统一经由 `ipc` 对象调用后端命令，
 * 命令名与 src-tauri/src/commands/*（project.rs / task.rs / model.rs / config.rs）
 * 的 #[tauri::command] 一一对应。
 */
export const ipc = {
  pickProjectFolder: () => invoke<string | null>('pick_project_folder'),
  openProject: (path: string) => invoke<Project>('open_project', { path }),
  closeProject: () => invoke<void>('close_project'),
  listProjects: () => invoke<Project[]>('list_projects'),
  getFileTree: (projectId: number) => invoke<FileNode>('get_file_tree', { projectId }),
  readFile: (projectId: number, path: string, start?: number, end?: number) =>
    invoke<string>('read_file', { projectId, path, start, end }),

  runTask: (projectId: number, prompt: string, allowVerification?: boolean, parentTaskId?: number, allowCommands?: boolean) =>
    invoke<number>('run_task', { projectId, prompt, ...(allowVerification !== undefined ? { allowVerification } : {}), ...(parentTaskId !== undefined ? { parentTaskId } : {}), ...(allowCommands !== undefined ? { allowCommands } : {}) }),
  cancelTask: (taskId: number) => invoke<void>('cancel_task', { taskId }),
  getExecutionSettings: (projectId: number) => invoke<ExecutionSettings>('get_execution_settings', { projectId }),
  setExecutionSettings: (projectId: number, mode: ExecutionMode, gitNetwork: GitNetwork) => invoke<ExecutionSettings>('set_execution_settings', { projectId, mode, gitNetwork }),
  revokeExecutionGrant: (projectId: number, scopeId: string) => invoke<ExecutionSettings>('revoke_execution_grant', { projectId, scopeId }),
  getPendingPermission: () => invoke<PermissionRequest | null>('get_pending_permission'),
  resolveExecutionPermission: (taskId: number, requestId: string, decision: PermissionDecision) => invoke<void>('resolve_execution_permission', { taskId, requestId, decision }),
  getTask: (taskId: number) => invoke<Task>('get_task', { taskId }),
  listTasks: (projectId: number) => invoke<Task[]>('list_tasks', { projectId }),
  listTaskSteps: (taskId: number) => invoke<TaskStep[]>('list_task_steps', { taskId }),
  listConversationTasks: (taskId: number) => invoke<Task[]>('list_conversation_tasks', { taskId }),
  readTaskFile: (taskId: number, path: string) => invoke<string>('read_task_file', { taskId, path }),
  applyPatch: (patchId: number) => invoke<void>('apply_patch', { patchId }),
  rejectPatch: (patchId: number) => invoke<void>('reject_patch', { patchId }),
  rollbackTask: (taskId: number) => invoke<void>('rollback_task', { taskId }),
  listPatches: (taskId: number) => invoke<Patch[]>('list_patches', { taskId }),

  getModelConfig: () => invoke<ModelConfig>('get_model_config'),
  setModelConfig: (config: ModelConfig) => invoke<void>('set_model_config', { config }),
  testModelConnection: (config: ModelConfig, reasoningEffort?: string, thinking?: boolean, supportsReasoning?: boolean) =>
    invoke<boolean>('test_model_connection', { config, ...(reasoningEffort ? { reasoningEffort } : {}), ...(thinking !== undefined ? { thinking } : {}), ...(supportsReasoning !== undefined ? { supportsReasoning } : {}) }),
  listModels: (provider: string) => invoke<string[]>('list_models', { provider }),
  /** 列出受支持的模型提供方标识（后端 list_providers）。 */
  listProviders: () => invoke<string[]>('list_providers'),

  getConfig: () => invoke<Config>('get_config'),
  setConfig: (config: Config) => invoke<void>('set_config', { config }),
  getStorageInfo: () => invoke<StorageInfo>('get_storage_info'),
  openDataFolder: (kind: 'home' | 'workspaces' | 'backups') => invoke<void>('open_data_folder', { kind }),
  pickStorageFolder: () => invoke<string | null>('pick_storage_folder'),
  moveDataDirectory: (path: string) => invoke<void>('move_data_directory', { path }),
  cancelDataMove: () => invoke<void>('cancel_data_move'),
  createDataBackup: () => invoke<string>('create_data_backup'),
  restoreDataBackup: () => invoke<string | null>('restore_data_backup'),
  setWorkspaceRetention: (days: number) => invoke<void>('set_workspace_retention', { days }),
  cleanupWorkspaces: () => invoke<number>('cleanup_workspaces'),
  openTaskWorkspace: (taskId: number) => invoke<void>('open_task_workspace', { taskId }),
  exportTaskRecord: (taskId: number) => invoke<string | null>('export_task_record', { taskId }),
  getPreferences: () => invoke<Record<string, string>>('get_preferences'),
  setPreference: (key: string, value: string) => invoke<void>('set_preference', { key, value }),
  getRuntimeSettings: (projectId: number | null) => invoke<RuntimeSettings>('get_runtime_settings', { projectId }),
  setRuntimeSettings: (projectId: number | null, settings: RuntimeOverrides) => invoke<void>('set_runtime_settings', { projectId, settings }),
};
