# 任务
共享 TypeScript Result 类型

新增 src/result.ts 导出 Result<T>，并让 src/reader.ts 与 src/writer.ts 导入并返回该类型；运行时结果保持不变。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 新增 src/result.ts 导出 Result<T>，并让 src/reader.ts 与 src/writer.ts 导入并返回该类型；运行时结果保持不变。
- 仅允许修改或新增：src/result.ts、src/reader.ts、src/writer.ts
- 验证脚本退出码为 0
