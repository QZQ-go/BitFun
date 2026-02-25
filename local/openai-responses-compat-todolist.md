# BitFun OpenAI Responses 兼容开发 TODOLIST（高可用 + 可溯源）

> 目标：让 BitFun 在**不破坏现有 OpenAI-Compatible（`/chat/completions`）用户**的前提下，新增对 OpenAI 最新 **`/responses`** 范式的完整兼容。

---

## 0. 文档使用说明

- 本文档是**可执行清单**，按顺序逐项推进。
- 每项任务都绑定了“改动依据（Trace IDs）”，避免“为什么改”不可追溯。
- 建议按小步提交（每个阶段一个 PR 或至少一个 commit）。

**状态约定**

- `[ ]` 未开始
- `[x]` 已完成
- `BLOCKED:` 阻塞项（写明原因和下一步）

---

## 1. 问题定义（本次开发的边界）

### 1.1 现象

当前 BitFun 的 OpenAI 流程强绑定 Chat Completions 语义，导致对仅支持或优先支持 `/responses` 的网关/模型兼容不足。

### 1.2 目标

1. 新增 `openai_responses` 格式（或等价命名）
2. 后端支持 `/responses` 请求体与流式事件解析
3. 前端可配置该格式，并提供清晰默认值/提示
4. 现有 `openai`（chat/completions）行为保持兼容

### 1.3 非目标（本轮不做）

- 不重构整个 AI 架构
- 不移除旧 `openai` 格式
- 不引入破坏性配置迁移

---

## 2. 溯源依据（Traceability Matrix）

## A. BitFun 现状证据

- **BTF-001**: `src/crates/core/src/infrastructure/ai/client.rs:241-307`
  - OpenAI 请求体固定为 `model/messages/stream/tools/tool_choice`（chat 风格）
- **BTF-002**: `src/crates/core/src/infrastructure/ai/client.rs:452-474`
  - 请求 URL 直接使用 `config.base_url`，未按格式自动拼接 endpoint
- **BTF-003**: `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/stream_handler/openai.rs:13-19,97-106`
  - 只接受 `object == "chat.completion.chunk"`
- **BTF-004**: `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/types/openai.rs:31-45,79-89`
  - SSE schema 强依赖 `choices[].delta` 结构
- **BTF-005**: `src/web-ui/src/shared/types/chat.ts:14`
  - `ApiFormat` 仅 `'openai' | 'anthropic'`
- **BTF-006**: `src/web-ui/src/infrastructure/config/services/modelConfigs.ts:37,48,59,75,86`
  - 多个模板默认是 `/chat/completions`
- **BTF-007**: `src/web-ui/src/infrastructure/config/components/AIModelConfig.tsx:190`
  - 自定义默认 URL 也是 `/chat/completions`

## B. 对照实现（opencode-dev）

- **OCD-001**: `packages/console/app/src/routes/zen/util/provider/openai.ts:17`
  - OpenAI helper 走 `/responses`
- **OCD-002**: `packages/console/app/src/routes/zen/util/provider/openai-compatible.ts:26`
  - OpenAI-Compatible helper 走 `/chat/completions`
- **OCD-003**: `packages/opencode/src/provider/sdk/copilot/responses/openai-responses-language-model.ts:399,779`
  - `/responses` 专用模型实现（请求 + 流）
- **OCD-004**: `packages/opencode/test/session/llm.test.ts:365,434`
  - 明确断言 OpenAI 请求路径是 `/responses`

## C. 官方规范依据

- **DOC-001**: OpenAI migration guide（建议新项目优先 Responses）
  - <https://developers.openai.com/api/docs/guides/migrate-to-responses/>
- **DOC-002**: Streaming Responses（事件类型为 `response.*`）
  - <https://developers.openai.com/api/docs/guides/streaming-responses>
- **DOC-003**: Conversation state（`previous_response_id` / `store`）
  - <https://developers.openai.com/api/docs/guides/conversation-state>
- **DOC-004**: Function calling（`function_call` / `function_call_output`）
  - <https://developers.openai.com/api/docs/guides/function-calling>

---

## 3. 开发总策略（先决策，后编码）

### 决策 D1（兼容优先）

- 保留现有 `openai` = OpenAI-Compatible Chat（`/chat/completions`）
- 新增 `openai_responses` = OpenAI Responses（`/responses`）

> 依据：BTF-001/BTF-003 + OCD-001/OCD-002 + DOC-001

### 决策 D2（URL 策略）

- 支持两种 `base_url` 输入：
  1) 传完整 endpoint（如 `.../chat/completions` 或 `.../responses`）
  2) 传 API 根路径（如 `.../v1`），由系统按 format 自动拼接 endpoint

> 依据：BTF-002 + 你的实际场景（`/v1`）

### 决策 D3（实现方式）

- **新增 Responses 专用 builder + stream handler**，不在旧 parser 上硬补丁

> 依据：BTF-003/BTF-004 + OCD-003 + DOC-002

---

## 4. 分阶段 TODOLIST（可直接执行）

## Phase 0：基线与复现

- [ ] **T0-1 建立复现用配置样本**
  - 产出：`local/samples/openai-responses-repro.json`（或等价路径）
  - 内容：`base_url=https://api.wecodemaster.com/v1` + 目标模型 + provider format
  - 验收：能稳定复现“当前不兼容”
  - 依据：BTF-001/BTF-002

- [ ] **T0-2 基线记录**
  - 记录当前可用路径（chat/completions）和失败路径（responses）日志
  - 产出：`local/logs/baseline-*.md`
  - 验收：后续可与修复后行为对比

---

## Phase 1：配置契约扩展（后端 + 前端类型）

- [x] **T1-1 扩展格式枚举/字符串约定**
  - 目标：支持 `openai_responses`
  - 相关文件（至少）：
    - `src/web-ui/src/shared/types/chat.ts`
    - `src/crates/core/src/service/config/types.rs`（若有 provider 校验/默认逻辑）
    - `src/crates/core/src/util/types/config.rs`
  - 验收：新格式可被配置层读写，不影响旧配置
  - 依据：BTF-005

- [x] **T1-2 前端配置 UI 支持新格式**
  - 相关文件（至少）：
    - `src/web-ui/src/infrastructure/config/components/AIModelConfig.tsx`
    - `src/web-ui/src/features/onboarding/components/steps/ModelConfigStep.tsx`
    - `src/web-ui/src/infrastructure/config/services/modelConfigs.ts`
    - 对应 i18n 文案文件
  - 验收：可在 UI 中选择 `openai_responses`，并能保存
  - 依据：BTF-006/BTF-007

---

## Phase 2：URL 归一化与路由分发

- [x] **T2-1 新增 endpoint 解析函数**
  - 规则：
    - 若 `base_url` 已以 `/chat/completions` 或 `/responses` 结尾，直接使用
    - 否则按 `format` 自动拼接：
      - `openai` -> `/chat/completions`
      - `openai_responses` -> `/responses`
  - 相关文件：`src/crates/core/src/infrastructure/ai/client.rs`
  - 验收：根路径与完整路径都可正确请求
  - 依据：BTF-002 + OCD-001/OCD-002

- [x] **T2-2 发送分支拆分**
  - 目标：`openai` 与 `openai_responses` 分别走独立函数
  - 验收：旧 `openai` 行为不回归；新分支可触发
  - 依据：BTF-001/BTF-003

---

## Phase 3：`/responses` 请求体构建

- [x] **T3-1 新增 Responses message/input 转换器**
  - 建议新增模块（示例命名）：
    - `.../providers/openai/responses_message_converter.rs`
  - 能力：`Message` -> `input` items；tools -> responses tools
  - 依据：DOC-001/DOC-004 + OCD-003

- [x] **T3-2 新增 `build_openai_responses_request_body`**
  - 目标字段：`model/input/stream/tools/tool_choice/...`
  - 保留 `custom_request_body` 覆盖能力
  - 验收：发出的 body 结构符合 Responses 规范
  - 依据：DOC-001 + OCD-003

---

## Phase 4：`/responses` 流式事件解析（核心）

- [x] **T4-1 新增 Responses stream handler**
  - 建议新增：
    - `.../ai_stream_handlers/src/stream_handler/openai_responses.rs`
    - `.../ai_stream_handlers/src/types/openai_responses.rs`
  - 关键事件最小支持：
    - `response.output_text.delta`
    - `response.function_call_arguments.delta`
    - `response.completed`
    - `error`
  - 依据：DOC-002 + OCD-003

- [x] **T4-2 映射到 BitFun 统一结构**
  - 输出到 `UnifiedResponse`（text/tool_call/usage/finish_reason）
  - 验收：上层 agent 不需要感知格式差异
  - 依据：BTF-004

- [x] **T4-3 保证旧 OpenAI parser 不回归**
  - 仅在 `openai_responses` 走新 parser
  - 验收：`openai` 模式仍按 chat chunk 解析

---

## Phase 5：测试与验证

- [x] **T5-1 Rust 单测（至少）**
  - URL 归一化测试
  - responses body 构建测试
  - responses SSE 事件解析测试（文本、tool 参数增量、completed usage）
  - 建议命令：
    - `cargo test -p ai_stream_handlers`
    - `cargo test -p bitfun-core`

- [ ] **T5-2 前端类型/构建验证**
  - 建议命令：
    - `cd src/web-ui && npm run type-check`
    - `npm run build:web`

- [ ] **T5-3 集成手测（你的场景）**
  - 配置 `openai_responses + https://api.wecodemaster.com/v1`
  - 验收点：
    1. 能正常出字（流式）
    2. tool call 可往返
    3. usage 可见/不报错

---

## Phase 6：文档与提交准备

- [ ] **T6-1 更新配置说明文档**
  - 明确两种格式差异：
    - `openai` = chat/completions compatible
    - `openai_responses` = responses
  - 明确 `base_url` 支持根路径和完整 endpoint

- [ ] **T6-2 产出迁移指引**
  - 老配置如何迁移到新格式
  - 常见错误（endpoint 配错、provider key 缺失）

- [ ] **T6-3 PR 描述模板**
  - 问题、方案、兼容性、测试证据、回滚方案

---

## 5. 验收标准（DoD）

- [ ] 旧 `openai`（chat/completions）不受影响
- [ ] 新 `openai_responses` 可用且流式稳定
- [ ] 支持 `base_url` 根路径自动拼接
- [ ] 单元测试通过（core + stream handlers）
- [ ] Web UI 类型检查与构建通过
- [ ] 文档覆盖配置方式与迁移步骤

---

## 6. 风险清单与回滚策略

### 风险 R1：错误合并 parser 导致老格式回归

- 规避：新建 responses parser，不修改旧 parser 主逻辑
- 回滚：仅回滚 `openai_responses` 分支文件

### 风险 R2：base_url 自动拼接误判

- 规避：增加 URL 归一化单测（根路径/完整路径/尾斜杠）
- 回滚：临时切回“仅完整 endpoint”策略

### 风险 R3：tool-call 增量参数拼接失败

- 规避：对 `function_call_arguments.delta` 做增量缓存 + JSON 完整性校验
- 回滚：先支持文本流，工具流降级为非流/完整包解析（短期）

---

## 7. 执行记录模板（每次开发后补充）

```md
### 执行记录 - YYYY-MM-DD HH:mm

- 完成任务：T?-?
- 实际改动文件：
  - path/a
  - path/b
- 使用依据（Trace IDs）：BTF-xxx, OCD-xxx, DOC-xxx
- 验证结果：
  - 命令1：pass/fail
  - 命令2：pass/fail
- 遗留问题：
  - ...
```

### 执行记录 - 2026-02-24 17:10

- 完成任务：T1-1, T1-2
- 实际改动文件：
  - `src/web-ui/src/shared/types/chat.ts`
  - `src/web-ui/src/infrastructure/config/services/modelConfigs.ts`
  - `src/web-ui/src/infrastructure/config/components/AIModelConfig.tsx`
  - `src/web-ui/src/features/onboarding/store/onboardingStore.ts`
  - `src/web-ui/src/features/onboarding/components/steps/ModelConfigStep.tsx`
  - `src/web-ui/src/locales/en-US/settings/ai-model.json`
  - `src/web-ui/src/locales/zh-CN/settings/ai-model.json`
  - `src/web-ui/src/locales/en-US/onboarding.json`
  - `src/web-ui/src/locales/zh-CN/onboarding.json`
- 使用依据（Trace IDs）：BTF-005, BTF-006, BTF-007, DOC-001
- 验证结果：
  - `cd src/web-ui && npm run type-check`：fail（仓库存在大量既有 TS 错误，非本次改动引入）
  - `LSP diagnostics（改动文件）`：本次新增逻辑无新增类型错误；存在既有 Biome 规则错误（a11y/exhaustive-deps）
- 遗留问题：
  - 仓库前端当前不处于可全量 type-check 通过状态，Phase 2 前建议先隔离既有错误或在 CI 中加增量检查

### 执行记录 - 2026-02-24 17:35

- 完成任务：T2-1, T2-2
- 实际改动文件：
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `local/openai-responses-compat-todolist.md`
- 使用依据（Trace IDs）：BTF-001, BTF-002, BTF-003, OCD-001, OCD-002
- 验证结果：
  - `lsp_diagnostics(client.rs)`：fail（环境缺少 `rust-analyzer`）
  - `cargo fmt --all`：fail（环境缺少 `cargo`）
  - `which cargo; which rustc; which rust-analyzer`：not found（确认当前会话无法执行 Rust 编译/测试）
- 遗留问题：
  - `openai_responses` 当前仅完成 URL 路由与分支拆分，`/responses` 请求体与事件解析将在 Phase 3/4 完成

### 执行记录 - 2026-02-24 18:06

- 完成任务：T3-1, T3-2
- 实际改动文件：
  - `src/crates/core/src/infrastructure/ai/providers/openai/mod.rs`
  - `src/crates/core/src/infrastructure/ai/providers/openai/responses_message_converter.rs`
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `local/openai-responses-compat-todolist.md`
- 使用依据（Trace IDs）：DOC-001, DOC-004, OCD-003, BTF-001, BTF-003
- 验证结果：
  - `lsp_diagnostics(client.rs/mod.rs/responses_message_converter.rs)`：fail（环境缺少 `rust-analyzer`）
  - `which cargo; which rustc; which rust-analyzer`：not found
  - `cargo test -p bitfun-core ...`：fail（环境缺少 `cargo`）
  - `cargo fmt --all`：fail（环境缺少 `cargo`）
- 遗留问题：
  - 当前会话环境缺少 Rust 工具链，无法执行编译/测试；Phase 4 的 `/responses` 流式事件解析尚未开始

### 执行记录 - 2026-02-24 18:45

- 完成任务：T4-1, T4-2, T4-3
- 实际改动文件：
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/types/openai_responses.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/stream_handler/openai_responses.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/types/mod.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/stream_handler/mod.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/lib.rs`
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `local/openai-responses-compat-todolist.md`
- 使用依据（Trace IDs）：DOC-002, DOC-004, BTF-003, BTF-004, OCD-003
- 验证结果：
  - `~/.cargo/bin/rustfmt --edition 2021 <modified files>`：pass
  - `~/.cargo/bin/cargo test -p ai_stream_handlers`：pass（12 passed）
  - `~/.cargo/bin/cargo test -p bitfun-core normalize_openai_endpoint -- --nocapture`：pass（5 passed）
  - `~/.cargo/bin/cargo test -p bitfun-core build_openai_responses_request_body -- --nocapture`：pass（2 passed）
  - `~/.cargo/bin/cargo build -p bitfun-core`：pass
  - `lsp_diagnostics(<modified rust files>)`：fail（会话 LSP 环境未识别 rust-analyzer，可见工具报错；已通过 Rust 编译与测试补充验证）
- 遗留问题：
  - 全仓 `cargo fmt --all` 因仓库中既有文件存在 trailing whitespace 失败（与本次改动无关）

### 执行记录 - 2026-02-25 09:55

- 完成任务：T5-1（完成），T5-2（部分验证），T5-3（前置验证）
- 实际改动文件：
  - `src/crates/core/src/service/system/command.rs`
  - `local/openai-responses-compat-todolist.md`
- 使用依据（Trace IDs）：BTF-002, BTF-003, BTF-004, DOC-002
- 验证结果：
  - `cargo test -p ai_stream_handlers`：pass（13 passed）
  - `cargo test -p bitfun-core`：pass（55 unit + 1 doctest）
  - `cargo build -p bitfun-core`：pass
  - `cd src/web-ui && npm run type-check`：fail（仓库既有大量 TS 错误，非本轮 openai_responses 测试项新增）
  - `npm run build:web`：pass（存在既有 warnings，不阻塞构建）
  - `npm run e2e:test:l0`：fail（环境链路：`tauri-driver is not supported on this platform` / `ECONNREFUSED localhost:4444`）
  - `cargo build -p bitfun-desktop`：pass（已完成 E2E 前置构建）
- 补充修复：
  - 修复 `check_command` 的 doctest 示例作用域，避免 `cargo test -p bitfun-core` 在 doc-tests 阶段失败
- 遗留问题：
  - T5-2 尚未完成：`type-check` 受仓库存量类型错误阻塞
  - T5-3 尚未完成：需要在可用 E2E/Driver 平台及真实 provider 环境完成手工集成验证

### 执行记录 - 2026-02-25 10:02

- 完成任务：文档回填（本次测试推进结果）、提交与推送准备
- 实际改动文件：
  - `local/openai-responses-compat-todolist.md`
  - `src/crates/core/src/service/system/command.rs`
- 验证结果：
  - `git status / git diff / git log`：已复核
  - 分支：`feat/openai-responses-compat`（跟踪 `origin/feat/openai-responses-compat`）
- 遗留问题：
  - T5-2、T5-3 仍按上一条记录继续跟进

### 执行记录 - 2026-02-25 10:55

- 完成任务：用户反馈问题修复（OpenAI 文案精简 + Responses 兼容网关 `thinking` 参数报错修复）
- 实际改动文件：
  - `src/web-ui/src/locales/zh-CN/settings/ai-model.json`
  - `src/web-ui/src/locales/en-US/settings/ai-model.json`
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `local/openai-responses-compat-todolist.md`
- 修复说明：
  - OpenAI 提供商描述去除“优先 Responses”括号提示，避免界面冗余
  - `openai` / `openai_responses` 请求体不再默认发送 `thinking: { type: disabled }`
  - 仅在 `enable_thinking_process = true` 时发送 `thinking: { type: enabled }`，提升 OpenAI-Compatible 网关兼容性（规避 `Unsupported parameter: thinking`）
  - 新增/更新单元测试，覆盖 thinking 字段的条件注入行为
- 验证结果：
  - `lsp_diagnostics(client.rs + 2 个 locale 文件)`：pass
  - `cargo test -p bitfun-core build_openai_ -- --nocapture`：pass（5 passed）
  - `cargo build -p bitfun-core`：pass
  - `cd src/web-ui && npm run type-check`：fail（仓库既有大量 TS 错误，非本次改动引入）
  - `npm run build:web`：pass（存在既有 warning，不阻塞构建）
- 遗留问题：
  - 若第三方网关在开启 thinking 后依然不兼容，可在该配置中关闭“启用思考”或通过自定义请求体精确控制字段

### 执行记录 - 2026-02-25 11:20

- 完成任务：自定义模型连接失败问题修复（`openai_responses` 命中 Codex+ChatGPT 账号限制时报 400）
- 实际改动文件：
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `src/web-ui/src/locales/en-US/settings/ai-model.json`
  - `src/web-ui/src/locales/zh-CN/settings/ai-model.json`
  - `local/openai-responses-compat-todolist.md`
- 修复说明：
  - 新增错误识别逻辑：当上游返回 `model is not supported when using Codex with a ChatGPT account`（或同类 Codex 限制语义）时，识别为可回退场景
  - 在 `openai_responses` 流式调用失败后自动 fallback 到 `openai`（`/chat/completions`）重试，提升第三方 OpenAI-Compatible 网关在受限账户模式下的可用性
  - 新增单测覆盖：
    - 命中特征错误的识别
    - 非相关错误不误判
- 验证结果：
  - `lsp_diagnostics(src/crates/core/src/infrastructure/ai/client.rs)`：pass
  - `cargo test -p bitfun-core codex_chatgpt_account_error_detector`：pass（2 passed）
  - `cargo check -p bitfun-core`：pass
- 遗留问题：
  - 该 fallback 是兼容兜底；若供应商后端强制走特定账户通道，仍建议优先使用其明确支持的 endpoint/model 组合
  - 用户在对话中暴露了明文 API Key，需立即旋转/作废旧 Key

---

### 执行记录 - 2026-02-25 11:52

- 完成任务：Responses 兼容回归修复（`function_call_arguments.delta` 缺失 `call_id`）+ 文档回填
- 实际改动文件：
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/types/openai_responses.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/stream_handler/openai_responses.rs`
  - `docs/rfcs/openai-responses-compat.md`
  - `local/openai-responses-compat-todolist.md`
- 根因说明：
  - 用户环境 provider 返回 `response.function_call_arguments.delta` 事件仅含 `item_id`，不含 `call_id`
  - 旧实现中 `serde` 结构将 `call_id` 设为必填，导致流解析直接报错并中断
  - 本地 `local/test-openai-responses-config-chain.mjs` 使用宽松事件解析，不强依赖 `call_id` 必填，因此出现“脚本成功、应用失败”的差异
- 修复说明：
  - `Delta/Done` 事件结构支持 `call_id?: Option<String>` 与 `item_id?: Option<String>`
  - 新增 `item_id -> call_id` 关联解析（来自 `response.output_item.*`）
  - 当映射缺失时回退使用 `item_id` 作为 tool call 标识，避免硬失败
  - 新增回归测试覆盖 item-id-only 事件及 ID 解析优先级
- 验证结果：
  - `cargo test -p ai_stream_handlers`：pass（19 passed）
  - `cargo check -p bitfun-core`：pass（仅既有 warning）
  - `lsp_diagnostics(修改文件)`：pass
- 遗留问题：
  - 仍需用户在桌面端完成同配置实测确认（链路已启动供验证）

### 执行记录 - 2026-02-25 12:36

- 完成任务：连接失败修复（`builder error` + `Responses SSE stream closed before response completed`）+ 文档回填
- 实际改动文件：
  - `src/crates/core/src/util/types/config.rs`
  - `src/crates/core/src/infrastructure/ai/client.rs`
  - `src/crates/core/src/infrastructure/ai/ai_stream_handlers/src/stream_handler/openai_responses.rs`
  - `local/test-openai-responses-config-chain.mjs`
  - `docs/rfcs/openai-responses-compat.md`
  - `local/openai-responses-compat-todolist.md`
- 根因说明：
  - `builder error`：配置中 `api_key/base_url/model/format/custom_headers` 含空白或非法头字段时，`reqwest` builder 可能在发请求前失败。
  - `SSE stream closed before response completed`：部分 OpenAI-Compatible provider 会在已有有效增量输出后直接关闭 SSE，未发送 `response.completed`。
- 修复说明：
  - 在 `AIModelConfig -> AIConfig` 转换阶段统一 trim 关键字段，并清洗 `custom_headers/custom_headers_mode/custom_request_body`。
  - 在 AI client header 注入阶段增加 header 名/值合法性过滤，避免非法自定义头触发 builder 失败。
  - 在 Responses stream handler 中引入“有意义输出即成功结束”判定：若未见 `response.completed` 但已收到有效输出/tool-call 增量，SSE 提前关闭不再硬失败。
  - 同步更新本地链路脚本，保持与后端一致的输入清洗策略。
- 验证结果：
  - `cargo test -p ai_stream_handlers`：pass（22 passed）
  - `cargo test -p bitfun-core try_from_model_config_`：pass（3 passed）
  - `cargo test -p bitfun-core build_openai_`：pass（5 passed）
  - `cargo check -p bitfun-core`：pass（仅既有 warning）
  - `node local/test-openai-responses-config-chain.mjs`（脏输入：空格/换行）: pass
- 遗留问题：
  - 仍需在桌面端 UI 用同配置完成一次人工回归（验证 provider 实际回包时序）

## 8. 建议分支策略

- 建议分支名：`feat/openai-responses-compat`
- 建议 PR 拆分：
  1. 配置与类型
  2. 后端请求构建
  3. 流解析与工具调用
  4. UI + 文档

---

## 9. 当前建议起手顺序（最小风险）

1. 先做 Phase 1（类型/配置）
2. 再做 Phase 2（URL 归一化）
3. 再做 Phase 3（请求体）
4. 最后做 Phase 4（流解析）

> 原因：这样每一步都可独立验证，出现问题容易定位与回滚。
