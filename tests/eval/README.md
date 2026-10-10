# XAim 评测任务

30 个独立任务，每个任务可整目录复制。初始代码保留待解决问题；`expected.diff` 仅供评测侧验证参考解，不能提交给 XAim。

| 编号 | 类别 | 数量 |
| --- | --- | --- |
| 001–010 | Bug 修复，TS / Python / Rust / Go | 10 |
| 011–015 | 补测试，使用变异检查 | 5 |
| 016–020 | 重构，行为与共享模块检查 | 5 |
| 021–025 | 跨文件，类型与调用契约检查 | 5 |
| 026–028 | 本地依赖升级与 API 迁移 | 3 |
| 029–030 | 代码审查，结构化结论检查 | 2 |

```text
tasks/<id>/
  repo/          初始代码，只将它复制到 XAim 工作目录
  task.md        任务、验证、期望
  verify.sh      验证入口，支持指定候选目录
  verify.mjs     独立验证器，无需依赖父目录
  task.json      允许修改范围、命令、变异配置
  check.mjs      可选的评测侧契约检查
  expected.diff 可应用的参考补丁，路径相对于 repo/
```

## 环境

- Node.js 22+、npm、Git、Bash（Windows 使用 Git Bash）。
- Python 3.10+，`pip`、`setuptools`、`wheel`（027 构建本地 Python 包）。
- Rust / Cargo stable，支持 Cargo.lock v4；Go 1.20+。
- 可通过 `EVAL_PYTHON`、`EVAL_GO` 指定解释器或编译器路径。
- TS 验证自动执行 `npm ci`，首次需要下载锁定的 TypeScript 5.5.4；Python 构建工具需提前安装。Rust、Go 和三个升级任务的目标依赖均不需要外部服务。升级任务使用随任务提供的真实 v1/v2 包，验证版本、锁文件（适用时）、API 迁移与运行结果。

安装上述工具后，在项目根目录运行 `node tests/eval/run.mjs reference`。非标准安装路径可通过 `EVAL_GO`、`EVAL_PYTHON` 指定。验收摘要见 [VALIDATION.md](VALIDATION.md)。

## 单任务

保留评测目录的 `repo/` 作为基线，将它复制到独立工作目录。将该工作目录作为 XAim 项目，只提交 `task.md` 内容。不要把参考补丁、验收器或其他任务交给 XAim。

```bash
bash tests/eval/tasks/001-fix-null-pointer/verify.sh /path/to/work-repo
```

退出码 0 为通过，非零为失败；检查测试结果、文件修改范围和任务特定契约。补测试任务必须使每个预设变异触发断言失败；编译失败不能当作识别变异。审查任务采用明确的文件行号、触发示例、影响和建议检查，是结构化自动判定，不代表完整人工审查质量。

省略参数时验证初始 `repo/`，预期失败。必须保留原始 `repo/`，不能直接在基线上完成任务，否则无法判断修改范围。

参考补丁可在工作目录执行 `git apply /absolute/path/to/expected.diff`，不需要初始化 Git 仓库。

## 批量评测

### 验证已有候选目录

在 XAim 项目根目录运行 JavaScript 验证器：

```bash
node tests/eval/run.mjs baseline
node tests/eval/run.mjs reference
node tests/eval/run.mjs candidate /path/to/candidates
```

### 自动执行 XAim

Rust 运行器直接调用 XAim 的真实编排器：复制每个 `repo/` 到临时目录，执行 `task.json` 的依赖准备命令，创建独立内存数据库与 Git 基线，读取 `task.md`，执行默认工具循环（不支持工具的适配器才走兼容补丁流程），自动确认应用，然后运行原始任务目录的 `verify.sh` 检查候选。初始代码不会改动。统计包装器透传工具调用与检查点能力。

原生工具循环与桌面共用流式优先交互，统计包装器透传正文增量、请求诊断与最终 usage。逻辑调用包含流式兼容探测/回退，实际 HTTP 尝试另由诊断统计；`requestParameters` 记录每个传输分支的有效参数，不能只凭首轮偏好元数据判断是否回退。正文增量持久化为 `modelText` 事件；推理正文和未完成工具参数不展示。缺少流式 usage 时运行预算采用保守预留，评测 token 只统计返回 usage，不能当作完整账单。

已冻结的非流式 30 项成绩保持原样，不能用流式协议回归测试声称在线成功率提高；新的在线评测需单独冻结源码与传输参数。

```bash
npm run eval -- --config /path/to/config.json
# 等价的 Cargo 命令
cargo run --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval -- --config /path/to/config.json
# 不调用模型，验收整条评测链路
npm run eval -- --mode reference
# 检查初始代码
npm run eval -- --mode baseline
```

`--config` 使用 XAim 的 `Config` JSON 格式，示例见 `config.example.json`。请填写已安装或服务支持的模型；示例不会自动下载模型。云端密钥可在配置中填写，也可通过 `XAIM_EVAL_API_KEY` 覆盖。运行器复用桌面配置构建函数，实际使用 `model.name`、当前模型 profile 的推理参数和输出/上下文限制；与桌面一样，fast/strong 均使用当前模型。

默认只运行评测侧的最终验证。增加 `--allow-verification` 可授权工具循环中的 `run_verify` 和自动最终检查；这些脚本以当前用户权限运行，不是 OS 沙箱。比较两次实验时必须保持此开关一致。依赖准备发生在评测候选目录；Agent 源码副本仍排除依赖，内部检查在独立验证副本中执行。清单和锁文件未变化时复用已安装的 Node 依赖，变化或缺失时明确报告 unavailable；评测侧原始验收仍独立执行。

任务超时先请求取消，最多给予 10 秒保存宽限时间，再从持久检查点导出诊断与阶段性差异。严格完成率仍将超时计为失败；`candidate_verify_status` 单独记录未完成候选的原始验收结果，不计入完成率。`artifacts/任务ID/` 保存 partial.diff、partial-changes.json 与候选验证日志，JSON 同时包含 checkpoint、diagnostics、事件时间戳及路径。运行报告兼容缺少这些字段的历史 JSON。

评测侧最终验收在应用后的源码副本上重新准备候选依赖，不继承 Agent 基线目录的 node_modules/.venv/构建产物。这样依赖升级不会被预安装旧版本的 dist-info 元数据污染；源码、允许文件范围、测试和版本断言保持原标准。

OpenAI 兼容工具请求诊断按任务/轮次记录各次尝试、HTTP 状态、响应头和正文阶段耗时、总耗时、请求 ID、重试/退避及取消。不会导出 API 地址、密钥、请求正文或私有思考。响应头等待包含网络、排队及模型处理时间，不能直接视为纯网络时延。请求和验证共享任务截止时间，重试尊重 Retry-After（秒或 HTTP 日期），剩余时间不足时停止。

审查评分接受文件冒号行号、中文“第 N 行”和英文 line N；错误行号仍失败。运行 `node --test tests/eval/review-validation.test.mjs` 检查等价格式、错误位置及生成器一致性。

支持 `--output <目录>`、`--bash <Bash路径>`、`--task-timeout <秒>`（默认 1800）、`--verify-timeout <秒>`（默认 600）。API `pub async fn run_eval(tasks_dir: &Path) -> EvalReport` 默认执行 agent 模式；环境变量 `XAIM_EVAL_CONFIG`、`XAIM_EVAL_MODE`、`XAIM_EVAL_OUTPUT`、`EVAL_BASH` 分别设置配置、模式、报告目录和 Bash。

Rust 报告默认位于 `reports/rust/<mode>.json` 和 `reports/rust/<mode>.md`，含成功率、分类结果、每任务阶段、日志、失败原因、耗时、模型调用次数和输入/输出 token。metadata 记录模型、工具协议、预算、超时、检查授权和平台，不记录密钥；可用 `--output` 为每次实验保留独立报告。token 从模型响应 `usage` 汇总，包含失败前已完成的模型调用；验证输出不计 token。供应方未返回 usage 时为 0，平均值分母为全部任务数，不能把缺失 usage 当作实际消耗为零。临时目录在每个任务完成后清理。

每完成一个任务即更新报告，保留中断前已完成任务的日志。`metadata.complete=false` 表示批次尚未完成，`expected_tasks` 是本批任务总数；此时成功率只针对已记录项。完整批次结束后才标记完成。

Windows 默认使用 Git Bash，验证超时时结束其进程 Job（含子进程）；Unix 使用独立进程组。保留每条命令 stdout/stderr 的最后 1 MiB，超时前的输出仍写入失败日志。实际模型候选评估使用 XAim 原有验证命令，因此除 `EVAL_GO` 外，Go 还应可在 `PATH` 中找到。

缺失 `repo/`、`task.md`、`verify.sh` 或损坏的任务元数据计失败；一个任务失败不影响其他任务。全部 agent/reference 通过时命令退出 0；baseline 全部未达标时退出 0；空评测集、目录错误、报告写入失败或其他未通过情况退出 1。agent 缺少模型配置时记录任务执行错误，不以验证通过代替任务执行。

### JavaScript 报告

候选目录结构为 `/path/to/candidates/<task-id>/repo/`。分母固定为全部 30 个任务，缺失任务也计失败。成功率 = 通过数 / 30；每类分别统计。报告默认写入 `tests/eval/reports/<mode>.json`，可通过 `--output /path/to/report.json` 指定。

`baseline` 应为 0/30，全部未达标时命令退出 0；`reference` 应为 30/30。Rust `agent` 模式直接测量 XAim 完成任务的成功率；JavaScript `candidate` 模式测量已生成候选目录的成功率。参考解成绩只用于证明任务和验证器可运行，不代表 XAim 的准确率。

基线与参考模式复制到 `.runs/`，不改动任务原件。候选模式安装依赖、生成测试产物，源码不由验证器修改。构建产物不计入文件修改；报告包含每个任务的日志、失败原因和耗时。

## 添加任务

复制任一任务目录，更新 `task.md`、`repo/`、`task.json`、必要的 `check.mjs` 与 `expected.diff`。每个任务必须能证明初始代码未达标、参考解通过，且独立复制后验证入口可运行。允许等价实现，不比较参考补丁文本。

`catalog/` 保存任务源码与参考解，`generate.mjs` 输出用于维护任务文件的补丁文本；修改目录与清单时应同步。原始任务、参考解与运行产物分开保存。
