# 任务
补充 Unicode slug 测试

仅修改 tests/test_slug.py。覆盖中文、连续空白、空字符串，保留 Unicode 并以单个连字符分隔。测试必须发现 ASCII 过滤和空白拆分错误。

# 验证
python -m unittest discover -s tests

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅修改 tests/test_slug.py。覆盖中文、连续空白、空字符串，保留 Unicode 并以单个连字符分隔。测试必须发现 ASCII 过滤和空白拆分错误。
- 仅允许修改或新增：tests/test_slug.py
- 验证脚本退出码为 0，且所有变异实现均被测试识别
