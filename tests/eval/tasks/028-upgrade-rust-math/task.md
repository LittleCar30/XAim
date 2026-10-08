# 任务
升级本地 eval-math 到 v2

将 Cargo.toml 中 eval-math 从 vendor/math-v1 升级到 vendor/math-v2，版本从 1.0.0 改为 2.0.0；同步 Cargo.lock。v2 的 double 改名为 twice，更新 src/lib.rs。不得修改 vendor 包。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将 Cargo.toml 中 eval-math 从 vendor/math-v1 升级到 vendor/math-v2，版本从 1.0.0 改为 2.0.0；同步 Cargo.lock。v2 的 double 改名为 twice，更新 src/lib.rs。不得修改 vendor 包。
- 仅允许修改或新增：Cargo.toml、src/lib.rs、Cargo.lock
- 验证脚本退出码为 0
