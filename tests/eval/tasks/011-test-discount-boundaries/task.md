# 任务
补充折扣边界测试

仅修改 tests/discount.test.mjs。覆盖折扣率 0、1、非法负数和大于 1 的数值；非法折扣率应抛 RangeError。新增测试必须能发现不拒绝非法折扣率的变异实现。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅修改 tests/discount.test.mjs。覆盖折扣率 0、1、非法负数和大于 1 的数值；非法折扣率应抛 RangeError。新增测试必须能发现不拒绝非法折扣率的变异实现。
- 仅允许修改或新增：tests/discount.test.mjs
- 验证脚本退出码为 0，且所有变异实现均被测试识别
