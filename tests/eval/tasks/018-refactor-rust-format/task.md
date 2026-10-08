# 任务
提取 Rust 格式化函数

将 summary.rs 和 detail.rs 的金额格式化提取到 format.rs 的 format_cents；两个调用方不得保留重复实现，输出保持一致。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将 summary.rs 和 detail.rs 的金额格式化提取到 format.rs 的 format_cents；两个调用方不得保留重复实现，输出保持一致。
- 仅允许修改或新增：src/format.rs、src/summary.rs、src/detail.rs、src/lib.rs
- 验证脚本退出码为 0
