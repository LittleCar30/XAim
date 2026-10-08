# 任务
跨文件共享 API 客户端

新增 src/api.ts 的 request 函数；client.ts 必须导入并调用它，保留请求参数和返回 Promise 行为。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 新增 src/api.ts 的 request 函数；client.ts 必须导入并调用它，保留请求参数和返回 Promise 行为。
- 仅允许修改或新增：src/api.ts、src/client.ts
- 验证脚本退出码为 0
