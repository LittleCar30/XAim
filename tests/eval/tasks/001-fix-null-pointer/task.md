# 任务
修复登录空指针

修复 src/auth.ts 的 login；null 或 undefined 用户返回 null，有效用户返回 name，空字符串必须保留。只修改 src/auth.ts。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 修复 src/auth.ts 的 login；null 或 undefined 用户返回 null，有效用户返回 name，空字符串必须保留。只修改 src/auth.ts。
- 仅允许修改或新增：src/auth.ts
- 验证脚本退出码为 0
