# 任务
修复端口解析崩溃

src/lib.rs 的 port 对合法 u16 数字返回其值；无效、负数、溢出或空输入返回默认端口 8080。合法 0 必须保留。

# 验证
cargo test --offline --locked

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- src/lib.rs 的 port 对合法 u16 数字返回其值；无效、负数、溢出或空输入返回默认端口 8080。合法 0 必须保留。
- 仅允许修改或新增：src/lib.rs
- 验证脚本退出码为 0
