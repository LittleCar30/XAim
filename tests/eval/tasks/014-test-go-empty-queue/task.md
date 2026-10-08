# 任务
补充 Go 队列测试

仅修改 queue_test.go。检查空队列 Pop 返回 (0,false)、FIFO、耗尽后仍返回 (0,false)。测试必须发现空队列和 FIFO 变异。

# 验证
go test ./...

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 仅修改 queue_test.go。检查空队列 Pop 返回 (0,false)、FIFO、耗尽后仍返回 (0,false)。测试必须发现空队列和 FIFO 变异。
- 仅允许修改或新增：queue_test.go
- 验证脚本退出码为 0，且所有变异实现均被测试识别
