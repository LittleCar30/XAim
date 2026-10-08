# 任务
拆分 Go 配置模块

新增 config.go 的 Config 结构和 LoadConfig；server.go 从该模块读取端口，默认端口 8080，测试通过环境变量覆盖。

# 验证
go test ./...

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 新增 config.go 的 Config 结构和 LoadConfig；server.go 从该模块读取端口，默认端口 8080，测试通过环境变量覆盖。
- 仅允许修改或新增：config.go、server.go
- 验证脚本退出码为 0
