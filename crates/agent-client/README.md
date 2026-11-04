# agent-client

Komari Agent 的协议 v1 运行链路。对应 Go 的 `server/websocket.go`、
`server/basicInfo.go` 与 `cmd/root.go` 中相关的启动/更新接线。

## 模块

| 模块 | 作用 |
|------|------|
| `endpoint` | `{endpoint}/api/clients/report?token=` 与 `uploadBasicInfo` URL；`http→ws`/`https→wss`、IDN、尾斜杠、IPv6、路径前缀 |
| `ws` | `WsTransport`/`WsSender`/`WsReceiver` trait；生产实现基于 `tokio-tungstenite`，TCP 先经 `agent-discovery::DnsResolver` 解析，支持 `ignore_unsafe_cert` 与 CF Access 头 |
| `report` | 采集值 → `ReportV1` 的纯映射，含 0.001 CPU 下限、GPU 详细/型号回退形状 |
| `basic_info` | `BasicInfoV1` 构造与上传；首次失败后按 Go 语义删除 `kernel_version`/`cpu_physical_cores` 重试 |
| `provider` | `ReportProvider` trait 与 `MonitorReportProvider`，CPU 固定 1 秒窗口 |
| `ip` | A05 唯一的采集模块：IP-only（A03 不提供 IP），NIC/custom 优先级与 IFF_UP |
| `client` | 连接、首帧、心跳、读循环 drain、重连与重试上限 |
| `clock` | 可注入时钟（测试用极短间隔） |

## 循环语义

1. 数据 tick 为 `max(1, interval - 1)` 秒，与 Go 为 1 秒采样窗口预留时间一致；
2. 启动时先上传一次基础信息（WS 不可达也会上传），之后由独立任务按
   `info_report_interval` 周期上传；未连接时先连接，成功后立即发送第一帧；
3. 每 30 秒发送 Ping；读任务只消费服务端帧，收到 Close/读错误时通知主循环；
4. 发送或心跳失败会关闭连接，下个数据 tick 按 `max_retries + 1` 次尝试、
   `reconnect_interval` 间隔重连（最后一次失败后同样 sleep 再返回）；
5. 重试耗尽后 `run()` 返回 `RunOutcome::RetriesExhausted`。

v2、gzip 与 POST fallback 不存在（D-004）。

## 测试

- URL：http/https、IDN、尾斜杠/路径、IPv4/IPv6、非法 endpoint；
- 映射：全字段、CPU 下限、GPU 详细形状与型号回退、零值 section；
- 基础信息：启动首传（WS 不可达仍上传）、周期上传不阻塞上报/心跳、
  兼容字段回退、双失败、CF Access 头、字段形状；
- 循环：通道 mock 覆盖首帧、心跳、服务端关闭/读错误重连、发送/心跳失败重连、
  重试耗尽、最终失败 sleep、旧 disconnect generation；
- 真实 transport：本地 `tokio-tungstenite` mock server 完成握手、文本帧与 close；
  自签证书验证 `ignore_unsafe_cert` 开关；
- 指标收敛：A03 的 disk/network/connections/GPU/process/message 全部进入 `ReportV1`；
  `from_metrics` 逐字段构造无默认丢弃；
- IP-only：网卡过滤通配符、IFF_UP 无 carrier、NIC 优先于 custom。

所有测试均在本机完成，不访问真实面板或 GitHub API。

## 已知限制

- 指标采集全部来自 A03；A05 只保留 IP-only 模块（D-019）。非 Linux 的
  GPU/连接/进程平台路径见 A03 的 D-017（未在目标系统实测，见 agent-monitor README）；
- 未设置 `custom_ipv4/ipv6` 且未启用 `get_ip_addr_from_nic` 时，基础信息中的 IP 为空
  （Go 会调用外部 IP 查询服务，这里刻意避免引入外网依赖，见 D-020）；
- 重试耗尽后由 CLI 以退出码 1 退出，而不是像 Go root 命令那样立即进入下一轮
  `UpdateBasicInfo + EstablishWebSocketConnection`（见 D-018）；
- WebSocket 不走 HTTP(S)_PROXY，也不启用 permessage-deflate（见 D-021）；
  `month_rotate`/netstatic 月度流量由 A03 提供（见 D-022）。
