# 任务
修复分页尾部数据丢失

pagination.py 的 pages(items, size) 应包含最后不足 size 的一页；空输入返回空列表。size <= 0 抛出 ValueError。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- pagination.py 的 pages(items, size) 应包含最后不足 size 的一页；空输入返回空列表。size <= 0 抛出 ValueError。
- 仅允许修改或新增：pagination.py
- 验证脚本退出码为 0
