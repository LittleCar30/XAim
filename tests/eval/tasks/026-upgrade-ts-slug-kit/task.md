# 任务
升级本地 slug-kit 到 v2

将 package.json 中 slug-kit 从 vendor/slug-v1 升级到 vendor/slug-v2，同步 package-lock.json。v2 把 slug 改为 slugify，更新 src/app.ts；输出保持小写连字符形式。vendor 包不得修改。

# 验证
npm test

评测侧使用 bash verify.sh /path/to/work-repo 检查全部通过标准。

# 期望
- 将 package.json 中 slug-kit 从 vendor/slug-v1 升级到 vendor/slug-v2，同步 package-lock.json。v2 把 slug 改为 slugify，更新 src/app.ts；输出保持小写连字符形式。vendor 包不得修改。
- 仅允许修改或新增：src/app.ts、package.json、package-lock.json
- 验证脚本退出码为 0
