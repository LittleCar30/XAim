# 任务
修复 Rust 索引越界

src/lib.rs 的 total 应正确求和，空切片返回 0，不得 panic。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- src/lib.rs 的 total 应正确求和，空切片返回 0，不得 panic。
- 仅允许修改或新增：src/lib.rs
- 验证脚本退出码为 0
