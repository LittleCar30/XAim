# 任务
修复空切片访问

first.go 的 First 对 nil 或空切片返回 (0, false)，非空输入返回 (首元素, true)，首元素为 0 也应返回 true。

# 验证
go test ./...

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- first.go 的 First 对 nil 或空切片返回 (0, false)，非空输入返回 (首元素, true)，首元素为 0 也应返回 true。
- 仅允许修改或新增：first.go
- 验证脚本退出码为 0
