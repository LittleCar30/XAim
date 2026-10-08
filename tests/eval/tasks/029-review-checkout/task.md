# 任务
审查 checkout.py 的路径安全

仅创建 REVIEW.md，审查 checkout.py，记录缺陷的 P1 优先级、准确文件行号、触发示例、影响和可执行修复建议；不修改生产代码。

# 验证
bash ../verify.sh

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅创建 REVIEW.md，审查 checkout.py，记录缺陷的 P1 优先级、准确文件行号、触发示例、影响和可执行修复建议；不修改生产代码。
- 仅允许修改或新增：REVIEW.md
- 验证脚本退出码为 0
