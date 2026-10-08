// Monaco 按需配置：与 @monaco-editor/react 一起被 DiffView 懒加载（动态 import），
// 不进入首屏包体。loader 指向本地 monaco-editor 包（含 worker），桌面端完全离线可用。
// 注意：monaco-editor 固定在 0.52.2（无 exports 限制，支持经典 esm 深路径 worker 导入）。
import { loader } from '@monaco-editor/react'
import * as monaco from 'monaco-editor'
import cssWorker from 'monaco-editor/esm/vs/language/css/css.worker?worker'
import htmlWorker from 'monaco-editor/esm/vs/language/html/html.worker?worker'
import jsonWorker from 'monaco-editor/esm/vs/language/json/json.worker?worker'
import tsWorker from 'monaco-editor/esm/vs/language/typescript/ts.worker?worker'
import editorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker'

// DiffEditor 的补丁计算与语言服务都走 worker；未命中的语言回落到基础 editor worker。
;(self as unknown as { MonacoEnvironment: monaco.Environment }).MonacoEnvironment = {
  getWorker: (_workerId, label) => {
    if (label === 'typescript' || label === 'javascript') return new tsWorker()
    if (label === 'json') return new jsonWorker()
    if (label === 'css' || label === 'scss' || label === 'less') return new cssWorker()
    if (label === 'html') return new htmlWorker()
    return new editorWorker()
  },
}

loader.config({ monaco })
