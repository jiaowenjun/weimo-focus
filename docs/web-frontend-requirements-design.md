# Time Tracker Web 前端需求分析与设计

## 文档状态

- 状态：Implemented（Web-0 至 Web-4 初始切片）
- 日期：2026-09-01
- 适用目录：`weimo-time/frontend/web`
- 前置条件：backend HTTP API v1 与 HTTP-only CLI 契约保持稳定
- 实施边界：本文同时记录 Web 产品、界面、技术方案和当前初始切片的实现边界；backend API v1 未因 Web 实现而改变

## 1. 结论摘要

`weimo-time/frontend/web` 设计为单机、单用户、同源访问的时间记录工作台。第一版只有一个主工作区，围绕一条最短路径展开：填写记录 -> 预览后端派生结果 -> 保存或保存并同步 -> 查看状态 -> 重试待同步事件。

当前初始切片已经落地 API 接缝、首屏状态、事件预览/创建、重复确认、待同步队列、历史筛选分页、单条/批量同步入口、响应式布局和运行时验收；真实 Calendar 同步仍只通过 backend 触发。

Web 只通过 `http://127.0.0.1:9123` 的 `/health` 和 `/api/v1` 接口工作。SQLite、Calendar、marker、数量计算、结束时间计算、重复判断和同步恢复全部留在 Rust backend；浏览器不读取数据库、不调用 `osascript`，也不复制后端领域规则。

推荐技术栈为 React 19 + TypeScript 6 + Vite 8 + TanStack Query + Zod + 原生 CSS 变量 + `lucide-react`。当前初始切片使用 React 本地表单状态，不引入 React Hook Form、路由框架、全局状态库、UI 组件大包或服务端渲染。

## 2. 背景与问题

Web 目录基于 Vite/React 脚手架，当前已实现单页工作台初始切片。backend 提供以下稳定能力：

- `/health`：数据库健康和当前平台 Calendar 支持情况。
- `/api/v1/bootstrap`：后端当天、历史日历/标题建议和待同步数量。
- `/api/v1/events/preview`：无副作用地校验客户端提交的时间区间，并返回规范化时间、数量和重复状态。
- `/api/v1/events`：创建事件，可选择仅保存或保存并同步。
- `/api/v1/events`：按状态、日期、日历分页查询事件。
- `/api/v1/events/{id}/sync`：单条同步或重试。
- `/api/v1/sync`：串行同步全部 pending 事件。

Calendar 调用可能持续数秒，失败后事件仍然已经写入 SQLite。Web 必须把“保存结果”和“同步结果”分开表达，不能用一个成功/失败布尔值掩盖这两个阶段。

## 2.1 方案审计

### 保留的优点

1. backend 继续独占 SQLite、Calendar、marker、派生字段和恢复逻辑，Web 只穿过 HTTP 接缝，职责清晰。
2. `201 + sync.failed` 的提交语义被明确建模，能够避免把“已保存但未同步”误报为“未保存”。
3. 单页工作台适合单机工具的高频短流程，表单、预览、队列和历史记录集中在同一上下文中。
4. 预览、重复确认、错误码映射、刷新恢复和无障碍要求已经覆盖主要业务风险。
5. 将真实 Calendar 排除在自动化测试之外，并把浏览器验证放在 API/组件契约之后，验证边界合理。

### 发现的问题与遗漏

| 级别 | 审计发现 | 影响 | 修订结论 |
| --- | --- | --- | --- |
| 高 | `/health` 位于 `/api/v1` 之外，原方案只写了 `/api` 代理 | 首屏健康检查在开发环境会跨源失败 | Vite 同时代理 `/health` 和 `/api`；客户端默认使用相对路径 |
| 高 | create/sync 是有副作用的 POST，网络超时后浏览器可能拿不到事件 ID | 自动重试可能产生重复事件，且不能声称“已保存”或“未保存” | 不自动重试 create；超时进入“结果未知”状态，先刷新列表并让用户确认；按 ID 的重试只用于已知 ID |
| 高 | UI 同时承诺“待同步”和“最近记录”，但单次列表查询未必同时满足两者 | pending 可能被分页裁掉，队列不完整 | 使用独立的 pending 查询和 recent 查询；两者都由 backend 过滤/分页 |
| 中 | `health.calendar_supported` 只能说明平台支持，不能证明权限或目标日历可用 | 顶部显示“Calendar 可用”会过度承诺 | 顶部只显示“平台支持/不支持”；真实权限和缺失日历在同步结果中反馈 |
| 中 | React Query、RHF、Zod、图标库和测试栈一次性引入，对单页工具偏重 | lockfile、升级和首次实现成本增加 | 保留 TanStack Query 解决远程缓存；移除 RHF；Zod 和图标按实现阶段实际需要加入；不用全局状态库 |
| 中 | 方案未定义 transport/protocol 错误与业务错误的分层 | 网络断开、响应漂移和 backend 拒绝会混在一起 | API 接缝定义 `transport_error`、`protocol_error` 和稳定业务 code 三类错误 |
| 中 | browser unload/AbortController 不能撤销已进入 backend 的 Calendar 调用 | “取消同步”会产生误导 | 同步不提供取消按钮；页面离开后重新加载以 backend 状态为准 |
| 低 | 未决定第一阶段是否由 backend 托管 dist | 实现可能把静态托管、代理和发布一起耦合 | 本轮只实现 Vite 开发/预览；backend 静态托管作为交付阶段单独决策 |

### 审计后的取舍

- Web-1 使用 React 本地状态 + 一个 `trackerApi` 深模块 + TanStack Query；表单字段数量很少，不引入 React Hook Form。
- Zod 只用于 API 响应运行时校验；如果增加依赖会明显阻碍首个切片，则先使用等价的窄 guard，并在 Web-1 完成前补回 schema。
- 不做 demo/fake 数据作为默认体验。浏览器验证使用临时 SQLite backend；自动化使用 fake API adapter。
- create 请求出现 transport timeout 时不自动重新 POST；重新读取事件列表，提示用户依据事件 ID/最近记录确认后再操作。
- “待同步”与“最近记录”是两组 query，分别使用 `status=pending` 和默认 `status=synced`；用户明确选择“待同步”或“全部状态”时，最近记录才切换到对应过滤条件。不在前端把一个分页结果重新解释成另一种集合。

## 3. 目标、非目标与原则

### 3.1 目标

1. 让用户在 30 秒内完成一条当天时间记录。
2. 在提交前显示 backend 计算出的结束时间、数量和重复提示。
3. 清楚区分 `pending`、同步失败和 `synced`。
4. 提供待同步队列、单条重试、全部同步和结果汇总。
5. 在后端未启动、Calendar 不支持、权限失败或数据库忙时给出可执行提示。
6. 在桌面宽屏和窄屏浏览器中保持可读、可键盘操作和不丢状态。

### 3.2 非目标

- 不编辑或删除已创建事件；当前 API 没有对应能力。
- 不支持历史日期、跨午夜事件或自定义时长；这些由 backend 第一阶段规则决定。
- 不创建、删除、移动或枚举 macOS Calendar。
- 不提供登录、多人协作、公网访问、云端数据库或跨设备同步。
- 不提供复杂报表、统计图、拖拽日历视图或实时异步任务中心。
- 不在浏览器本地持久化事件副本，不做 CSV 导入/导出。

### 3.3 设计原则

- **后端是真相**：表单只能做即时输入提示，最终校验和派生结果以 preview/create 响应为准。
- **保存先于同步**：任何 Calendar 失败都必须仍然显示事件已保存，并可重试。
- **深模块接缝**：Web 通过一个本地 `trackerApi` 模块访问后端，页面不直接拼接 URL、解析错误码或推导领域字段。
- **状态可恢复**：刷新页面后从 backend 重新读取，不依赖浏览器内存判断最终状态。
- **局部、密集、可扫描**：这是工作工具，不使用营销式 hero、嵌套卡片或装饰性插画。

## 4. 用户与核心流程

### 4.1 主要用户

使用 macOS Calendar 记录学习或工作时段的单机用户。用户通常知道自己的日历、事件标题和题目/任务编号范围，希望快速录入并确认是否已经同步。

### 4.2 P0 核心流程

```text
打开工作台
  -> GET /health 与 GET /api/v1/bootstrap
  -> 表单填充后端默认值
  -> 用户修改日历、标题、开始时间、起始/结束编号
  -> POST /api/v1/events/preview
  -> 显示结束时间、数量和重复状态
  -> 用户选择“仅保存”或“保存并同步”
  -> POST /api/v1/events
  -> 显示已保存结果和同步结果
  -> 失败时进入 pending 队列，支持单条/全部重试
```

### 4.3 P1 辅助流程

- 使用历史日历和标题建议，但始终允许输入新值。
- 通过状态筛选、日期筛选和日历筛选浏览历史记录。
- 刷新 bootstrap、健康状态和事件列表。
- 后端重启或另一个客户端写入后，页面重新聚合状态。

## 5. 功能需求

需求编号使用 `WEB-F`，优先级 `P0/P1/P2`。

| 编号 | 优先级 | 需求 | 验收要点 |
| --- | --- | --- | --- |
| WEB-F-01 | P0 | 启动检查 | 页面同时加载 health/bootstrap；任一失败显示原因和重试按钮 |
| WEB-F-02 | P0 | 新建表单 | 支持日历、日期、开始时间、标题、起始、结束；日期显示后端当天且只读 |
| WEB-F-03 | P0 | 历史建议 | 日历和标题展示 bootstrap 建议，可输入建议之外的新值 |
| WEB-F-04 | P0 | 后端预览 | 输入完整且变更后调用 preview；显示规范化时间、结束时间、数量和重复状态 |
| WEB-F-05 | P0 | 重复确认 | preview 或 `409 duplicate_event` 后，只有明确确认才允许 `allow_duplicate=true` |
| WEB-F-06 | P0 | 创建动作 | 支持 `save` 和 `save_and_sync`；不能把保存成功但同步失败显示为创建失败 |
| WEB-F-07 | P0 | 状态表达 | `pending`、`synced`、同步失败和 `sync_state_unknown` 结果有不同颜色、文案和操作 |
| WEB-F-08 | P0 | 单条重试 | pending 行可调用 `/events/{id}/sync`，成功后更新该行及 pending 计数 |
| WEB-F-09 | P0 | 全部同步 | 顶部操作调用 `/api/v1/sync`，显示已尝试/成功/失败汇总和逐条结果 |
| WEB-F-10 | P0 | 错误处理 | 显示稳定 code 对应的用户文案；字段错误定位到具体控件 |
| WEB-F-11 | P0 | 刷新恢复 | 刷新、重新打开或请求超时后，从 backend 重新读取，不丢失已保存事件 |
| WEB-F-12 | P1 | 历史列表 | 支持分页、按状态/日期/日历过滤，默认展示最近事件 |
| WEB-F-13 | P1 | 键盘效率 | 表单可用 Tab 完成；Enter 提交预览；Dialog 可用 Esc 关闭且焦点可恢复 |
| WEB-F-14 | P1 | 响应式 | 1024px 以上双列；窄屏单列，操作按钮不溢出、不遮挡内容 |
| WEB-F-15 | P1 | 可观测性 | 页面显示最近一次刷新时间和服务状态，不显示 marker、数据库路径或内部步骤 |
| WEB-F-16 | P2 | UI 偏好 | 仅在有真实需求时增加主题或列表密度偏好；不持久化业务事件数据 |

## 6. 字段与表单行为

### 6.1 表单字段

| 字段 | 控件 | 默认值与交互 | Web 侧规则 |
| --- | --- | --- | --- |
| `calendar_name` | 可输入建议框 | bootstrap 最近建议优先 | 去除首尾空白后不能为空；允许新值 |
| `event_date` | 只读日期输入/文本 | `bootstrap.server_date` | 默认使用今天；不允许客户端修改；不做浏览器时区转换 |
| `start_time` | `input type=time` 或文本时间输入 | 当前时间减 30 分钟后按 10 分钟向下对齐 | 前端生成 `end_time = start_time + 30 分钟`，提交前端计算结果 |
| `title` | 可输入建议框 | bootstrap 最近标题优先 | 去除首尾空白后不能为空；允许新值 |
| `first_item` | 数字输入 | recent suggestion 的 `next_first_item`，无值时留空 | 整数；不在前端推导历史编号 |
| `last_item` | 数字输入 | `first_item + 1` 作为交互默认值 | 整数；仅做“不能小于起始”的即时提示 |
| `end_time` | 只读预览字段 | 前端按固定 30 分钟计算并由 preview 回显 | 不允许手工覆盖 |
| `quantity` | 只读预览字段 | preview 返回 | 不允许手工覆盖 |
| `allow_duplicate` | 内部提交参数 | 默认 `false` | 只在重复确认后切换为 `true` |

开始时间可以接受 backend 文档规定的 `H:MM`、`HH:MM` 或 `HH:MM:SS`。Web 使用 10 分钟步进并在提交 preview 前计算结束时间；backend 只校验客户端提交的日期和时间区间。

### 6.2 预览触发策略

- 必填字段未齐全时不请求 preview，只显示本地字段提示。
- 完整输入首次达到可预览条件时请求一次。
- 任一影响事件内容的字段变化后，使用 300ms 防抖重新 preview。
- 同一输入快照只允许一个 preview 请求生效；过期响应不能覆盖较新的结果。
- preview 失败时清除旧的派生结果，避免用户误提交过期的结束时间/数量。
- preview 是无副作用请求，不能因为打开页面或改变筛选器而创建事件。

### 6.3 重复确认

当 `duplicate_status` 有值或创建返回 `409 duplicate_event` 时，打开确认对话框：

- 显示本次记录摘要和“已有同内容记录”的状态。
- 主操作文案明确为“仍然保存”或“仍然保存并同步”。
- 用户取消后保留表单，不改变输入。
- 确认后仅对下一次 create 请求设置 `allow_duplicate=true`，请求完成后立即恢复 `false`。

## 7. 界面与信息架构

### 7.1 单页工作台

第一版不需要客户端路由。根路径 `/` 展示一个工作台，便于本地工具快速启动和恢复上下文。

```text
┌─────────────────────────────────────────────────────────────────────┐
│ Time Tracker        Backend ●  Calendar ●       待同步 2   刷新  ↻   │
├───────────────────────────────┬─────────────────────────────────────┤
│ 新建记录                        │ 待同步 / 最近记录                    │
│ 日历      [学习          ▾]    │ [状态] [日期] [日历]                  │
│ 日期      [2026-08-30]        │ ┌─────────────────────────────────┐ │
│ 开始时间  [21:00]             │ │ 资料分析   21:00-21:30  pending │ │
│ 事件      [资料分析      ▾]    │ │ 205-218 · 数量 14       重试  │ │
│ 起始      [205]               │ └─────────────────────────────────┘ │
│ 结束      [218]               │                                     │
│                               │ [同步全部]                           │
│ 后端预览                      │                                     │
│ 21:00-21:30 · 数量 14         │                                     │
│                               │                                     │
│ [预览] [仅保存] [保存并同步]  │                                     │
└───────────────────────────────┴─────────────────────────────────────┘
```

页面不使用嵌套卡片。左侧是连续的表单和预览区，右侧是带分隔线的队列/列表区；重复事件确认和服务错误使用 Dialog/Alert，而不是在页面中再套一层卡片。

### 7.2 顶部状态栏

- 产品名：`Time Tracker`。
- backend 状态：`正常`、`未连接` 或 `数据库忙`。
- Calendar 状态：`可用`、`当前平台不支持` 或 `同步可能失败`。
- pending 数量：来自 bootstrap/list 响应；点击后将列表筛选为 pending。
- 刷新按钮：使用刷新图标，带 tooltip 和无障碍名称。

状态栏只表达可行动的运行状态，不显示数据库绝对路径、marker 或 AppleScript 细节。

### 7.3 新建记录区

- 表单标签始终可见，不依赖 placeholder 作为唯一说明。
- 日历和标题采用允许自由输入的建议框，建议项来自 bootstrap。
- 日期使用只读视觉样式，并附带“由 backend 决定”状态提示。
- 预览区只在 preview 成功后出现；显示规范化时间、数量和重复提示。
- “仅保存”是次要操作，“保存并同步”是主操作；两者都在提交期间禁用。
- 提交后表单重置为下一条记录的合理默认值，但不清除右侧刚创建的记录。

### 7.4 事件列表区

每行至少显示：标题、日历、日期、开始/结束时间、编号范围、数量、同步状态、同步尝试次数和最近错误摘要。

状态显示建议：

| 状态 | 视觉 | 文案 | 可用操作 |
| --- | --- | --- | --- |
| `pending` 且无错误 | 琥珀色点/标签 | 待同步 | 重试 |
| `pending` 且有错误 | 红色点/标签 | 同步失败，已保存 | 重试、展开错误 |
| `synced` | 绿色点/标签 | 已同步 | 无同步操作 |
| create 结果为 `sync_state_unknown` | 红色警示条 | 已保存，无法确认同步状态 | 按 ID 重试确认；刷新后按 pending 事件处理 |

“同步全部”在没有 pending 时禁用。批量同步返回后，列表和顶部计数必须重新获取，不依赖本地逐条拼接推断。

`sync_state_unknown` 不是 `EventView.sync_status` 的值。它应在创建结果区域作为一次性高优先级警示展示；事件本身仍为 `pending`，重新加载列表后按普通 pending 事件显示。

### 7.5 空、加载和错误状态

- 首次加载：表单骨架和列表骨架，不显示空列表文案。
- 无历史记录：显示“还没有记录”，同时保留可用的新建表单。
- backend 未启动：页面显示本地启动命令 `cargo run -p tracker-backend --locked -- --database ./data/tracker.sqlite3` 和重试按钮。
- Calendar 不支持：允许“仅保存”，禁用“保存并同步”和同步操作，并说明该页面仍可作为本地记录器使用。
- 数据库忙：提示稍后重试；不要清空已填写表单。
- 未知错误：显示通用文案和 request 失败操作，不暴露堆栈或本机路径。

## 8. 前端状态模型

### 8.1 服务状态

```text
loading -> ready
loading -> unavailable
ready -> degraded(calendar_supported = false)
ready -> unavailable (health 失败)
unavailable -> loading (手动重试)
```

`health` 和 `bootstrap` 使用独立查询，但首屏以两者都成功为 ready。health 失败时保留表单草稿，不做自动 create。

### 8.2 创建状态

```text
idle
  -> previewing -> preview_ready
  -> preview_error
preview_ready
  -> saving
  -> duplicate_confirmation (duplicate_status 有值)
duplicate_confirmation
  -> preview_ready (取消)
  -> saving (明确允许重复)
saving
  -> saved_pending
  -> saved_synced
  -> saved_pending_with_error
```

`201` 始终表示事件已经创建。判断最终状态必须同时读取 `event.sync_status` 和 `sync.status`；`sync_state_unknown` 只是同步结果错误码，不是新的事件状态：

- `not_requested`：事件已保存为 pending。
- `succeeded`：事件已保存且同步成功。
- `failed`：事件已保存但 Calendar 失败。
- `already_synced`：重试接口发现事件已是 synced。

### 8.3 同步状态

同步是同步 HTTP 请求，不设计假的百分比进度。调用期间显示“正在验证 Calendar…”和已等待时长；不提供取消按钮，因为浏览器中止请求不等于停止 backend 已经开始的外部副作用。

```text
not_running -> syncing_one | syncing_all
syncing_one -> synced | pending_error | unknown
syncing_all -> summary
summary -> idle (重新查询列表和 bootstrap)
```

超过 3 秒时可以显示“Calendar 响应较慢，仍在等待”，超过 30 秒显示“仍在等待 backend 返回”。已知事件 ID 的单条同步超时后保留该 ID，并提示刷新/重试确认；create 超时通常还没有事件 ID，客户端必须先重新读取 pending/recent 列表，不能自动再次 POST，也不能直接断言保存失败。

## 9. API 与数据接缝设计

### 9.1 `trackerApi` 深模块

页面只依赖以下小接口，所有 URL、HTTP 方法、JSON 编解码、稳定错误码和响应校验隐藏在 `src/api`：

```text
getHealth() -> HealthView
getBootstrap() -> BootstrapView
previewEvent(input) -> EventPreview
createEvent(command) -> CreateEventResult
listEvents(filter) -> EventPage
syncEvent(id) -> SyncEventResult
syncPending() -> BatchSyncResult
```

这是 Web 的外部接缝。生产适配器是 `fetch`；测试适配器是 fixture/fake API。页面和组件不得导入 backend Rust 类型，也不得在 JSX 中直接调用 `fetch`。

API 错误分为三层：

1. `transport_error`：无法连接、网络断开或客户端等待超时；不对服务端写入结果做假设。
2. `protocol_error`：HTTP 成功但 JSON 不符合 schema，或失败响应不是 error envelope；停止当前动作并提示刷新。
3. 业务错误：`validation_error`、`duplicate_event`、`database_busy`、Calendar 错误和其他 API v1 稳定 code；按错误码决定字段聚焦、重试或人工处理。

### 9.2 API 映射

| 页面动作 | 请求 | 成功后的缓存动作 |
| --- | --- | --- |
| 首屏服务检查 | `GET /health` | 更新 health 查询 |
| 首屏默认值 | `GET /api/v1/bootstrap` | 更新 bootstrap 查询和表单默认值 |
| 预览 | `POST /api/v1/events/preview` | 仅更新当前 preview 状态，不写事件列表 |
| 仅保存 | `POST /api/v1/events`，`action=save` | 失效 bootstrap/events，显示 pending |
| 保存并同步 | `POST /api/v1/events`，`action=save_and_sync` | 失效 bootstrap/events/health，显示同步结果 |
| 单条重试 | `POST /api/v1/events/{id}/sync` | 失效对应列表、bootstrap |
| 同步全部 | `POST /api/v1/sync` | 失效 events/bootstrap，显示批量汇总 |
| 待同步队列 | `GET /api/v1/events?status=pending&limit=50&offset=0` | 独立 pending query；用于顶部计数之外的逐条操作 |
| 最近记录 | 默认 `GET /api/v1/events?status=synced&limit=20&offset=0` | 独立 recent query；默认避免与 pending 队列重复，筛选器可切换状态 |
| 列表过滤/分页 | `GET /api/v1/events?...` | 以完整过滤条件作为 query key，不做前端过滤代替后端过滤 |

### 9.3 DTO 与运行时校验

TypeScript 类型和 Zod schema 必须覆盖 API v1 公共字段：`EventInput`、`EventPreview`、`EventView`、`CreateEventResult`、`SyncEventResult`、`BatchSyncResult`、`EventPage`、`BootstrapView`、`HealthView` 和 error envelope。

Zod 只负责防止响应漂移和基本输入形状错误，不实现 backend 的数量、marker 或重复算法；固定 30 分钟和 10 分钟开始时间粒度属于前端表单规则。响应 shape 不符合 schema 时归类为 `protocol_error`，与业务 `validation_error` 分开显示。

### 9.4 错误映射

| code | 用户文案方向 | 交互 |
| --- | --- | --- |
| `validation_error` | 修正标出的字段 | 聚焦第一个错误字段 |
| `duplicate_event` | 已有相同记录 | 打开重复确认，不自动重试 |
| `not_found` | 记录已不存在或已被移除 | 刷新列表 |
| `database_busy` | 数据库正被另一个 backend 使用 | 保留表单，稍后重试 |
| `calendar_missing` | 目标日历不存在 | 允许仅保存，提示检查 Calendar |
| `calendar_ambiguous` | 找到多个同名日历 | 提示用户处理日历命名，不自动选择 |
| `calendar_permission_denied` | Calendar 权限不足 | 给出 macOS 权限处理方向 |
| `calendar_unavailable` | Calendar 暂时不可用 | 事件已保存，可重试 |
| `calendar_platform_unsupported` | 当前平台不支持 Calendar | 禁用同步，保留仅保存 |
| `sync_state_unknown` | 已保存，但无法确认同步状态 | 按事件 ID 重试确认，不创建新记录 |
| `invalid_json` / `invalid_query` | 请求格式错误 | 记录为协议错误并允许刷新 |
| `internal_error` | 后端暂时无法完成请求 | 不清空表单，提供重试 |

## 10. 技术栈与包边界

### 10.1 推荐技术栈

| 层 | 选择 | 选择理由 |
| --- | --- | --- |
| UI runtime | React 19 + React DOM | 当前脚手架已使用，生态和类型支持成熟 |
| 语言 | TypeScript 6 | 当前 package 已锁定，启用严格未使用检查 |
| 构建 | Vite 8 + `@vitejs/plugin-react` | 当前 package 已存在，适合独立本地静态应用 |
| 服务端状态 | TanStack Query | 缓存、请求去重、失效和 mutation 状态适合本地 HTTP 工作台 |
| 表单 | React 本地状态/`useReducer` | 只有六个输入字段，显式状态机比引入表单框架更容易审计 |
| 运行时契约 | Zod（Web-1 可先用窄 guard） | 在 HTTP 接缝校验 JSON，尽早发现字段漂移；依赖成本过高时先保持等价 guard |
| 图标 | `lucide-react` | 只为工具按钮使用图标，保持按钮含义可识别 |
| 样式 | 原生 CSS + CSS variables | 独立 package，不引入 Tailwind 配置和公共 UI workspace 耦合 |
| 测试 | Vitest + fake fetch 契约 fixture；浏览器 smoke 验收 | 当前切片覆盖 HTTP 接缝和关键浏览器流程；Testing Library/MSW 在组件拆分或 mock 场景增多时再引入，不依赖真实 Calendar |
| Lint/typecheck | Oxlint + `tsc -b` | 延续当前脚手架脚本，作为构建门禁 |

第一版不引入 React Hook Form、Redux/Zustand、React Router、UI 组件大包、GraphQL、WebSocket、Tauri 或 SSR。只有当页面数量、跨页状态或桌面打包需求真实出现时，才重新评估这些依赖。

### 10.2 建议目录

```text
weimo-time/frontend/web/
├── src/
│   ├── api/
│   │   ├── client.ts          # fetch adapter、超时和 error envelope
│   │   ├── contract.ts        # TypeScript 类型与 Zod schema
│   │   └── tracker-api.ts     # 页面使用的深模块接口
│   ├── features/
│   │   ├── app-shell/         # 顶部状态、全局加载/错误
│   │   ├── event-entry/       # 表单、preview、重复确认
│   │   ├── event-list/        # 过滤、分页、行状态
│   │   └── sync/              # 单条/批量 mutation 与汇总
│   ├── components/            # 无 tracker 业务语义的基础控件
│   ├── api.test.ts            # HTTP 接缝和错误分类契约测试
│   ├── App.tsx
│   ├── App.css
│   ├── index.css
│   └── main.tsx
├── package.json
└── vite.config.ts
```

`features` 内部可以有多个 React 组件，但对 `App` 的公开接口保持为少数页面级模块。基础控件不能反向依赖 `api` 或 backend 领域名词。

### 10.3 开发与部署接缝

- 开发时 Vite 同时将 `/health` 和 `/api` 代理到 `http://127.0.0.1:9123`；不启用浏览器 CORS。联调临时 backend 时可用 `TRACKER_BACKEND_URL=http://127.0.0.1:9124` 覆盖代理目标。
- API 默认使用相对路径；只有显式配置时才允许覆盖 base URL，不能默认为公网地址。
- 正式运行优先由 backend 同源托管 `dist`，这样页面和 `/api/v1` 使用同一 origin。
- Web 是 `weimo-time` 的独立 pnpm package，不加入 Weimo 根前端 workspace，不接入生产 Home/Timu/Biji 导航。
- 生产构建使用 `pnpm install --frozen-lockfile`、`pnpm build` 和 `pnpm lint`；不把 SQLite、Calendar 脚本或 backend 二进制打进前端 bundle。

## 11. 性能、可访问性与安全

### 11.1 性能

- 首屏只加载工作台需要的代码；第一版不为不存在的路由做复杂分包。
- 列表默认 `limit=50`，分页由 backend 完成；不在前端拉取全部事件再过滤。
- preview 使用防抖和请求序列号，避免快速输入造成过多请求或旧响应覆盖新响应。
- 同步请求允许最长 60 秒；等待期间显示明确状态，不轮询不存在的异步 run。
- mutation 完成后只失效相关 query，不整页硬刷新。

### 11.2 可访问性

- 所有输入有可见 label、错误关联和键盘顺序。
- 状态变化通过 `aria-live="polite"` 宣布；错误通过 `role="alert"` 告知。
- Dialog 打开时焦点进入标题/主操作，关闭后返回触发按钮；Esc 关闭非破坏性 Dialog。
- 颜色不是状态的唯一信号，同时使用文本、图标和可读标签。
- 小屏下按钮保持稳定尺寸，长标题和错误信息换行，不覆盖相邻内容。

### 11.3 安全与隐私

- 只访问本机 backend；不收集、不上传事件内容。
- 不把事件、marker、错误堆栈或数据库路径写入 localStorage、URL 或分析服务。
- 不允许用户通过 Web 输入数据库路径、文件路径或命令参数。
- 不把 `sync_marker`、metadata、SQL 字段和内部日志步骤渲染到页面。
- 若后续开放远程访问，必须另立认证、CORS、CSRF 和权限设计；不能沿用本方案的无认证单机假设。

## 12. 测试与验收设计

### 12.1 Contract fixtures

当前初始切片已落实 `src/api.test.ts`，使用 fake `fetch` 覆盖 7 个契约测试；测试不访问用户真实 SQLite 或 Calendar。组件数量增加后，再将重复确认、query 失效和可访问焦点行为迁移到 Testing Library/MSW fixture。

为每个公共 API 响应准备最小 fixture：

- bootstrap 有默认时间、建议和 pending 数量。
- preview 返回 `quantity`、规范化 `end_time`、无重复/重复 pending/重复 synced。
- create 返回 `not_requested`、`succeeded(created)`、`succeeded(existing)`、`failed` 和 `sync_state_unknown`。
- list 返回空页、单页、分页和过滤结果。
- sync 返回单条成功/失败和批量部分成功。
- error envelope 覆盖所有稳定 code 及字段错误。

### 12.2 UI 行为测试

- 首屏加载成功、backend 未启动、Calendar 不支持和重试。
- 输入变化触发防抖 preview，旧 preview 响应不会覆盖新输入。
- 重复确认取消/确认，确认只设置一次 `allow_duplicate`。
- “仅保存”显示 pending；“保存并同步”失败显示已保存且可重试。
- 单条重试更新一行和 pending 计数；同步全部显示汇总并刷新列表。
- 页面刷新后按 backend 状态恢复，不依赖上一次 mutation 的内存结果。
- 键盘 Tab、Enter、Esc、Dialog 焦点和 `aria-live` 行为。

### 12.3 门禁命令

在实现 Web 后，至少执行：

```bash
cd weimo-time/frontend/web
pnpm install --frozen-lockfile
pnpm lint
pnpm exec tsc -b
pnpm test
pnpm build
git diff --check -- weimo-time/frontend/web weimo-time/docs
```

当前门禁使用 fake fetch 契约测试，并辅以本地 backend 的浏览器 smoke 验收；两者都不访问真实 Calendar。根据仓库规则，真实设备验收仍由用户在目标设备上完成，不作为自动门禁。

## 13. 分阶段实施顺序

### Web-0：契约冻结

- 核对 `weimo-time/docs/api-v1.md`、backend HTTP contract tests 和 CLI DTO。
- 建立 response/error fixtures，先验证 `201 + sync.failed` 的语义。
- 确认开发代理和 backend 同源托管方式，不在前端自行开启 CORS。

### Web-1：API 深模块与运行壳

- 实现 `trackerApi`、Zod schema、TanStack Query provider 和服务状态栏。
- 用 fake API 完成 loading/error/retry 的测试。

### Web-2：新增事件工作流

- 实现表单、历史建议、preview、重复确认和两种创建动作。
- 先覆盖保存 pending 与同步失败，再覆盖同步成功。

### Web-3：事件列表与同步队列

- 实现过滤、分页、单条重试、全部同步和结果汇总。
- 验证刷新恢复、并发按钮禁用和 query 失效范围。

### Web-4：可访问性、响应式与交付

- 完成窄屏布局、键盘焦点、错误文案和无障碍属性。
- 通过 lint/typecheck/test/build 后，再进行人工浏览器验收。

每个阶段的实现都必须保持 `frontend/web` 只依赖 HTTP；如果需求需要修改 backend 字段或状态，应先更新 API v1 和双方 contract tests，再开始 Web 代码。

## 14. 设计决策与待确认项

### 已确定

1. 单页工作台优先于多路由应用。
2. TanStack Query 管理远程状态，表单状态局部管理。
3. preview 是提交前的校验和重复检查来源；固定 30 分钟的结束时间由前端计算后提交，数量仍由 backend 派生。
4. 同步不做假的百分比进度，也不提供误导性的取消按钮。
5. Web 不加入 Weimo 根前端 workspace 或生产发布链路。

### 实现前需要确认

- backend 是否在 Web 阶段提供静态文件托管，还是保留独立 Vite preview 入口。
- Web 当前仍只显示并提交当天日期；backend/API 已具备接收其他合法日期的能力，未来开放历史日期时只需调整 Web 表单权限与交互。
- 是否需要把 `sync_attempts`、最近错误详情默认展开；如果列表过密，可改为行内摘要 + Dialog 详情。
- 是否需要 LaunchAgent/一键启动 backend；这属于本地运行体验，不应塞进 React 页面逻辑。

## 15. 依据与相关文档

- `weimo-time/docs/api-v1.md`：HTTP v1 路由、JSON shape、状态和错误码。
- `weimo-time/backend/src/domain.rs`：事件输入、预览、状态和公共视图字段。
- `weimo-time/backend/src/service.rs`：bootstrap、重复检查、创建、列表和同步恢复语义。
- `weimo-time/backend/src/http.rs`：路由、状态码和 error envelope。
- `weimo-time/frontend/cli/src/types.rs`：已验证的客户端 DTO 形状。
- `weimo-time/frontend/web/package.json`：当前 React/Vite/TypeScript/Oxlint 基线。
- `docs/superpowers/specs/2026-08-30-time-tracker-rearchitecture-requirements.md`：阶段顺序和 Web 范围的历史设计依据。
- `docs/superpowers/specs/2026-08-30-time-tracker-rearchitecture-design.md`：后端拥有 SQLite/Calendar、Web 仅 HTTP 的架构决策。
