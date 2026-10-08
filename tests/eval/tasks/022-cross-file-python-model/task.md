# 任务
共享 Python User 模型

新增 models.py 的 User dataclass；service.py 与 serializer.py 必须从 models 导入并接受该类型，序列化输出保持一致。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 新增 models.py 的 User dataclass；service.py 与 serializer.py 必须从 models 导入并接受该类型，序列化输出保持一致。
- 仅允许修改或新增：models.py、service.py、serializer.py
- 验证脚本退出码为 0
