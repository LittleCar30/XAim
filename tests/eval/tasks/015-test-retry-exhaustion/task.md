# 任务
补充重试次数测试

仅修改 tests/retry.test.mjs。检查 attempts=0 不调用操作且返回 false；重试耗尽时调用次数等于 attempts 并返回 false。测试必须发现多调用一次的变异。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅修改 tests/retry.test.mjs。检查 attempts=0 不调用操作且返回 false；重试耗尽时调用次数等于 attempts 并返回 false。测试必须发现多调用一次的变异。
- 仅允许修改或新增：tests/retry.test.mjs
- 验证脚本退出码为 0，且所有变异实现均被测试识别
