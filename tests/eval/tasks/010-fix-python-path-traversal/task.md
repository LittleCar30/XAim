# 任务
阻止路径逃逸

paths.py 的 safe_path(root, user_path) 返回解析后的绝对 Path；任何逃逸根目录的路径（..、绝对路径、相邻前缀目录、指向外部的符号链接）都必须抛出 ValueError。根目录内的合法路径应正常返回。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- paths.py 的 safe_path(root, user_path) 返回解析后的绝对 Path；任何逃逸根目录的路径（..、绝对路径、相邻前缀目录、指向外部的符号链接）都必须抛出 ValueError。根目录内的合法路径应正常返回。
- 仅允许修改或新增：paths.py
- 验证脚本退出码为 0
