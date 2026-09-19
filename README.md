# Time Tracker

`weimo-focus`（自 weimo monorepo 的 `weimo-time` 目录独立）是单机 backend + HTTP-only CLI。当前 Cargo workspace 有两个成员：

- `tracker-backend`：SQLite、领域规则、Calendar 和 `/api/v1` 的唯一所有者。
- `record-event` CLI 包：提供 `record-event`（补录已经完成的事件）和 `start-event`（确认后倒计时 30 分钟，再录入结束编号并保存）。

两个 CLI 都只通过 HTTP 使用 backend，不依赖 SQLx、CSV 或 Calendar 脚本。

运行时数据保存在 `data/tracker.sqlite3`，当前 backend 不再读取或写入 CSV。数据库升级后只保留事件和同步状态，不保留旧 CSV 的迁移元数据。

## 启动

从仓库根目录启动 backend：

```bash
cargo run -p tracker-backend --locked -- \
  --database ./data/tracker.sqlite3
```

默认只监听 `127.0.0.1:9123`，不启用 CORS。backend 对 `<database>.lock` 取得独占锁，同一数据库不能同时运行第二个 backend。

另一个终端运行 CLI：

```bash
cargo run -p record-event --locked
```

要开始一个新的 30 分钟事件：

```bash
cargo run -p record-event --bin start-event --locked
```

`start-event` 会先展示日历、预计时间、事件和起始编号并等待确认；确认后按秒显示倒计时。倒计时结束后，它再次展示事件信息，提示结束编号，然后保存并同步到 Calendar。由于当前 API 的单个事件不能跨越日期，30 分钟事件会跨越午夜时，命令会在倒计时开始前退出。

两个 CLI 都固定使用 `学习` 日历（不提示输入日历），日期、开始时间、结束时间、数量、重复结论和 marker 均由客户端准备后提交给 backend。`record-event` 默认使用当前时间减 30 分钟后按 10 分钟向下对齐；`start-event` 使用用户确认开始倒计时的时间。两者都提交 30 分钟事件。backend 接受任意合法日期、开始时间和结束时间（要求结束晚于开始）。操作为保存并同步；Calendar 失败时事件保留为 pending，CLI 明确显示“已保存但尚未同步”并返回 `1`。

## API 契约

HTTP 路由、JSON shape、错误码和“已保存但未同步”语义见 [API v1](docs/api-v1.md)。普通客户端响应不暴露 Calendar marker、内部 metadata 或数据库来源字段。

## 验证

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo metadata --locked --no-deps --format-version 1
```

自动化测试只使用临时 SQLite 和 fake Calendar adapter，不访问用户真实 Calendar。

Calendar 耗时实验及本机“学习”日历的只读测量见 [Calendar latency experiment](docs/calendar-latency-2026-08-31.md)，实验入口为 `cargo run -p tracker-backend --bin calendar_latency --locked`。

生产 Calendar marker 查找使用单次 `whose description contains marker` 查询，保留全日历 marker 恢复语义；修复后本机复测生产脚本平均约 8.14 秒，Rust adapter 平均约 8.18 秒。
