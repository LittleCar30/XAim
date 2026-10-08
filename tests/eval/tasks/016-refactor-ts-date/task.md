# 任务
提取日期格式化辅助函数

将重复的日期格式化逻辑提取到 src/date.ts 的 formatDate；app.ts 和 report.ts 必须共同调用该函数，输出保持一致。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将重复的日期格式化逻辑提取到 src/date.ts 的 formatDate；app.ts 和 report.ts 必须共同调用该函数，输出保持一致。
- 仅允许修改或新增：src/date.ts、src/app.ts、src/report.ts
- 验证脚本退出码为 0
