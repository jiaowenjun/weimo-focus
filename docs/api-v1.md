# Time Tracker HTTP API v1

backend 默认只监听 `http://127.0.0.1:9123`。除 `/health` 外，业务路由使用 `/api/v1`。第一阶段不启用 CORS，Web 开发环境必须使用同源代理。

## 公共事件

`EventView` 包含 `id`、日历、日期、开始/结束时间、标题、起止编号、派生数量、`sync_status`、尝试次数和可选的最近同步错误。它不包含 `sync_marker`、CSV metadata、数据库来源或后端内部步骤。

事件状态只有：

- `pending`：已经写入 SQLite，尚未确认 Calendar。
- `synced`：Calendar 返回 `created` 或 `existing`，且 SQLite 已更新。

## 路由

| 方法 | 路径 | 成功状态 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/health` | `200` | 数据库健康和当前平台是否支持 Calendar |
| `GET` | `/api/v1/bootstrap` | `200` | backend 当前日期、历史建议和 pending 数量 |
| `GET` | `/api/v1/events` | `200` | 支持 `status`、`event_date`、`calendar_name`、`limit`、`offset` |
| `POST` | `/api/v1/events/preview` | `200` | 校验事件日期/时间、数量和重复状态；无副作用 |
| `POST` | `/api/v1/events` | `201` | 创建 pending 事件，可选择立即同步 |
| `POST` | `/api/v1/events/{id}/sync` | `200` | 重试单条事件；已同步事件不再次调用 Calendar |
| `POST` | `/api/v1/sync` | `200` | 串行同步当前全部 pending 事件 |

第一阶段没有异步任务和 `run_id` 路由。

## 预览与创建

预览请求：

```json
{
  "calendar_name": "学习",
  "event_date": "2026-08-30",
  "start_time": "21:00",
  "end_time": "21:30",
  "title": "资料分析",
  "first_item": 205,
  "last_item": 218
}
```

创建请求在同一组字段上增加：

```json
{
  "action": "save_and_sync",
  "allow_duplicate": false
}
```

`action` 是 `save` 或 `save_and_sync`。`allow_duplicate` 默认应为 `false`；客户端只有在预览或 `409 duplicate_event` 后取得用户明确确认，才能发送 `true`。

backend 的 `event_date` 接受任意合法日期；`start_time` 和 `end_time` 接受 `8:30`/`08:30`/`08:30:00` 冒号输入，响应统一为 `HH:MM:SS`。backend 不计算默认时间或事件时长，只要求结束时间晚于开始时间；固定 30 分钟和开始时间按 10 分钟对齐由 CLI/Web 前端负责。

创建成功一律返回 `201`：

- `save`：`sync.status=not_requested`，事件为 pending。
- Calendar 成功：`sync.status=succeeded`，事件为 synced，`action` 是 `created` 或 `existing`。
- Calendar 失败：`sync.status=failed`，事件仍是已经保存的 pending；`sync.error` 提供 code/message。
- 保存提交后发生数据库连接或状态回写故障：仍返回 `201` 和 pending event ID，`sync.error.code=sync_state_unknown`；客户端必须按 ID 重试确认。

Calendar 业务失败不会被伪装成数据库写入失败。单条/批量重试也在 `200` body 中报告每条同步结果。

## Error Envelope

请求、查找或数据库失败使用非 2xx：

```json
{
  "error": {
    "code": "validation_error",
    "message": "结束时间必须晚于开始时间",
    "fields": {
      "end_time": "结束时间必须晚于开始时间"
    }
  }
}
```

稳定 code：

- `invalid_json`、`invalid_query`
- `validation_error`、`duplicate_event`、`not_found`
- `database_busy`、`internal_error`

同步结果中的稳定 code：

- `calendar_missing`
- `calendar_ambiguous`
- `calendar_permission_denied`
- `calendar_unavailable`
- `calendar_platform_unsupported`
- `sync_state_unknown`：事件已经保存，但 backend 无法确认本次同步或状态回写结果
