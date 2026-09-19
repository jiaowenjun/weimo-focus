# Calendar 调用耗时实验（2026-08-31）

## 实验范围

- 主机：macOS 26.6.2，arm64。
- 目标日历：`学习`，实验时包含 202 个事件。
- 路径：使用已经存在的 marker，验证 `existing` 结果；不创建事件、不删除事件。
- marker：`weimo-time-tracker-rust-v1:67101b21e973b3a6eb3fa0fa`。
- 另用刚创建的 marker `weimo-time-tracker-rust-v1:8390072d02d9abcee7f7026a` 做位置和日期对照。
- Calendar 进程未被强制退出，因此本实验不代表冷启动；每个子方案串行运行，单次 `osascript` 超时为 60 秒。

实验程序是 [calendar_latency.rs](../backend/src/bin/calendar_latency.rs)，运行前用只读 `whose` 查询确认 marker 存在；marker 不存在时拒绝执行可能创建事件的生产脚本。

示例命令：

```bash
cargo run -p tracker-backend --bin calendar_latency --locked -- \
  --marker weimo-time-tracker-rust-v1:67101b21e973b3a6eb3fa0fa \
  --runs 3 --timeout-seconds 60
```

## 修复前基线

以下是本机串行测量的代表性结果；时间为 wall-clock elapsed，包含 `osascript` 进程和 Calendar Apple Event 通信。

| 方案 | 次数 | min | p50 | avg | max |
| --- | ---: | ---: | ---: | ---: | ---: |
| `open -ga Calendar` | 10 | 69 ms | 76 ms | 77 ms | 95 ms |
| 生产脚本（修复前），旧 marker | 3 | 35.09 s | 37.39 s | 37.77 s | 40.83 s |
| 生产脚本（修复前），新 marker | 2 | 44.00 s | 44.68 s | 44.34 s | 44.68 s |
| Rust adapter（`open` + 修复前脚本） | 3 | 42.70 s | 43.31 s | 43.12 s | 43.35 s |
| 逐事件 loop，命中后返回 | 3 | 43.28 s | 43.40 s | 43.52 s | 43.87 s |
| `whose description contains marker` | 3 | 9.00 s | 9.39 s | 9.36 s | 9.68 s |
| 批量读取全部 description 后匹配 | 3 | 9.02 s | 9.05 s | 9.08 s | 9.16 s |
| 日期 + marker `whose` | 3 | 9.93 s | 10.18 s | 10.10 s | 10.20 s |
| `open` + `whose` | 3 | 9.50 s | 9.52 s | 9.54 s | 9.59 s |

新 marker 的优化查询结果也一致：`whose` 平均 9.40 秒，批量 description 平均 8.81 秒；因此不是事件日期或 marker 新旧位置导致差异。

## 修复后复测

生产脚本已改为单次 `whose description contains marker` 查询，并用已有 marker 对“学习”日历执行只读复测：

| 方案 | 次数 | min | p50 | avg | max |
| --- | ---: | ---: | ---: | ---: | ---: |
| 生产脚本（修复后），no open | 3 | 8.021 s | 8.030 s | 8.141 s | 8.371 s |
| Rust adapter（`open` + 修复后脚本） | 3 | 8.035 s | 8.129 s | 8.180 s | 8.377 s |

两组复测均返回 `existing`，marker 预检也返回 `1`。复测结束后的只读事件总数为 `203`；原始实验样本为 `202`，期间 Calendar 可能发生了外部变化，因此不把跨时点总数差异归因于本修复。复测命令不会创建或删除事件。

## 结论

1. **主因是逐事件 Apple Event 访问**。修复前脚本对 `every event` 逐条读取 `description`；202 个事件时耗时 35–45 秒。批量 description 或 `whose` 将访问合并到一次 Calendar 查询，平均约 9 秒，降低约 75–80%，约 4–5 倍加速。
2. **启动 Calendar 不是主因**。`open -ga Calendar` 只有约 77 ms；`open + whose` 比 `whose` 仅多约 0.18 秒。可以保留启动/就绪步骤，不应把它当作 40 秒耗时的解释。
3. **日期约束没有收益**。日期 + marker 查询平均 10.10 秒，略慢于不加日期的 `whose`；而且限制日期会破坏“事件被移动后仍能按 marker 恢复”的幂等语义。
4. **Rust/`spawn_blocking` 不是已证实的主因**。adapter 与生产脚本都处于 40 秒级，但 Calendar 本身波动很大；`spawn_blocking` 的调度开销不可能解释数十秒差异。
5. 本轮全部是 `existing` 只读路径，未测量新事件创建成本；也没有强制退出 Calendar 做冷启动实验，以避免干扰用户会话。

## 实施结果

生产脚本已将逐事件 loop 替换为单次查询：

```applescript
set matchingEvents to (every event of targetCalendar whose description contains marker)
if (count of matchingEvents) is greater than 0 then
    return "existing"
end if
```

该方案保持全日历 marker 搜索和崩溃恢复语义；修复后生产脚本和 Rust adapter 均约 8.1 秒，相比修复前 35–45 秒降低约 77–82%。批量读取全部 description 的历史均值略低（约 8.8–9.1 秒），但对异常 description 值的容错未充分验证，因此生产实现采用稳定性更明确的 `whose` 查询，并保留本 harness 做回归基线。

如果 9 秒仍不可接受，下一阶段应单独评估 EventKit/持久化 helper 或 marker 索引缓存；这两种方案本轮没有实现或测量，不能直接宣称收益。
