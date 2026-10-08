# 任务
提取 Go 标准化函数

将 user.go 和 team.go 的 strings.TrimSpace + strings.ToLower 逻辑提取到 normalize.go 的 Normalize；行为保持一致。

# 验证
go test ./...

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将 user.go 和 team.go 的 strings.TrimSpace + strings.ToLower 逻辑提取到 normalize.go 的 Normalize；行为保持一致。
- 仅允许修改或新增：normalize.go、user.go、team.go
- 验证脚本退出码为 0
