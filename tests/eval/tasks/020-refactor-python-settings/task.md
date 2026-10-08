# 任务
封装 Python 设置对象

将 app.py 与 worker.py 对 TIMEOUT 的直接全局读取改为从 settings.Settings.timeout 读取；值保持 30，不能复制第二份常量。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将 app.py 与 worker.py 对 TIMEOUT 的直接全局读取改为从 settings.Settings.timeout 读取；值保持 30，不能复制第二份常量。
- 仅允许修改或新增：settings.py、app.py、worker.py
- 验证脚本退出码为 0
