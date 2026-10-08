# 任务
拆分 Rust Error 类型

新增 error.rs 导出 AppError；reader.rs 与 writer.rs 返回 Result<T, AppError>，main/lib 声明模块，行为保持一致。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 新增 error.rs 导出 AppError；reader.rs 与 writer.rs 返回 Result<T, AppError>，main/lib 声明模块，行为保持一致。
- 仅允许修改或新增：src/error.rs、src/reader.rs、src/writer.rs、src/lib.rs
- 验证脚本退出码为 0
