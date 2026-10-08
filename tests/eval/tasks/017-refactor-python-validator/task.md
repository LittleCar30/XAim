# 任务
提取 Python 验证器

把 user.py 与 order.py 中重复的非空字符串检查提取到 validators.py 的 require_text；原函数行为保持一致，空白字符串必须被拒绝。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 把 user.py 与 order.py 中重复的非空字符串检查提取到 validators.py 的 require_text；原函数行为保持一致，空白字符串必须被拒绝。
- 仅允许修改或新增：validators.py、user.py、order.py
- 验证脚本退出码为 0
