# 任务
修复默认容器共享

cache.py 的 add(item, bucket) 在未传 bucket 时每次创建独立列表；显式传入 bucket 时追加到原列表并返回它。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- cache.py 的 add(item, bucket) 在未传 bucket 时每次创建独立列表；显式传入 bucket 时追加到原列表并返回它。
- 仅允许修改或新增：cache.py
- 验证脚本退出码为 0
