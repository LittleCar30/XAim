# 任务
升级本地 rules 包到 v2

requirements.txt 从 ./vendor/rules-v1 升级到 ./vendor/rules-v2。v2 把 validate 改为 is_valid；更新 app.py，保持正数为 True、零或负数为 False。不得修改 vendor 包。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- requirements.txt 从 ./vendor/rules-v1 升级到 ./vendor/rules-v2。v2 把 validate 改为 is_valid；更新 app.py，保持正数为 True、零或负数为 False。不得修改 vendor 包。
- 仅允许修改或新增：requirements.txt、app.py
- 验证脚本退出码为 0
