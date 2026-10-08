# 任务
补充 Rust 范围边界测试

仅修改 tests/range.rs。覆盖相等端点、反向范围，均应返回空 Vec。测试必须发现两种边界变异，不能改生产代码。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅修改 tests/range.rs。覆盖相等端点、反向范围，均应返回空 Vec。测试必须发现两种边界变异，不能改生产代码。
- 仅允许修改或新增：tests/range.rs
- 验证脚本退出码为 0，且所有变异实现均被测试识别
