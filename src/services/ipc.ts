import { invoke } from '@tauri-apps/api/core';
import type { Project, Task, TaskStep, Patch, FileNode, ModelConfig, Config } from '../types';

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

  runTask: (projectId: number, prompt: string, allowVerification?: boolean) =>
    invoke<number>('run_task', { projectId, prompt, ...(allowVerification !== undefined ? { allowVerification } : {}) }),
  cancelTask: (taskId: number) => invoke<void>('cancel_task', { taskId }),
  getTask: (taskId: number) => invoke<Task>('get_task', { taskId }),
  listTasks: (projectId: number) => invoke<Task[]>('list_tasks', { projectId }),
  listTaskSteps: (taskId: number) => invoke<TaskStep[]>('list_task_steps', { taskId }),
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
};
