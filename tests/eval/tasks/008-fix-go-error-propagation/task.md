# 任务
修复忽略数字解析错误

parse.go 的 ParseCount 必须将 strconv.Atoi 的错误传给调用方，合法非负整数成功；负数返回错误。

# 验证
go test ./...

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- parse.go 的 ParseCount 必须将 strconv.Atoi 的错误传给调用方，合法非负整数成功；负数返回错误。
- 仅允许修改或新增：parse.go
- 验证脚本退出码为 0
