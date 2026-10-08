# 任务
修复查询参数编码

src/query.ts 的 query 必须将参数键和值按 URL 查询组件编码；参数之间以 & 连接，不编码分隔符。空对象返回空字符串。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- src/query.ts 的 query 必须将参数键和值按 URL 查询组件编码；参数之间以 & 连接，不编码分隔符。空对象返回空字符串。
- 仅允许修改或新增：src/query.ts
- 验证脚本退出码为 0
