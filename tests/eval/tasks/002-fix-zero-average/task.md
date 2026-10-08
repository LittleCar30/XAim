# 任务
修复空数组平均值

src/average.ts 的 average([]) 应返回 0；非空数组仍返回算术平均值，包含负数和小数。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- src/average.ts 的 average([]) 应返回 0；非空数组仍返回算术平均值，包含负数和小数。
- 仅允许修改或新增：src/average.ts
- 验证脚本退出码为 0
