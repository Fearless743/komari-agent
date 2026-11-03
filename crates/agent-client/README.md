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
| `supplement` | A03 迁入前补齐磁盘/网络/连接/进程/GPU/IP；Linux 读取 `/proc`、`/sys` 与 `lspci`/`nvidia-smi` |
| `client` | 连接、首帧、心跳、读循环 drain、重连与重试上限 |
| `clock` | 可注入时钟（测试用极短间隔） |

## 循环语义

1. 数据 tick 为 `max(1, interval - 1)` 秒，与 Go 为 1 秒采样窗口预留时间一致；
2. 未连接时先连接，成功后立即发送第一帧；首个成功连接后立即上传基础信息；
3. 每 30 秒发送 Ping；读任务只消费服务端帧，收到 Close/读错误时通知主循环；
4. 发送或心跳失败会关闭连接，下个数据 tick 按 `max_retries + 1` 次尝试、
   `reconnect_interval` 间隔重连；
5. 重试耗尽后 `run()` 返回 `RunOutcome::RetriesExhausted`。

v2、gzip 与 POST fallback 不存在（D-004）。

## 测试

- URL：http/https、IDN、尾斜杠/路径、IPv4/IPv6、非法 endpoint；
- 映射：全字段、CPU 下限、GPU 详细形状与型号回退、零值 section；
- 基础信息：首传、兼容字段回退、双失败、字段形状；
- 循环：通道 mock 覆盖首帧、心跳、服务端关闭/读错误重连、重试耗尽、周期基础信息；
- 真实 transport：本地 `tokio-tungstenite` mock server 完成握手、文本帧与 close；
- 补充采集：网卡过滤通配符、虚拟文件系统排除、mount 转义。

所有测试均在本机完成，不访问真实面板或 GitHub API。

## 已知限制

- `supplement` 是为 A03 落地前的临时实现：GPU 详细探测仅支持 NVIDIA
  (`nvidia-smi`)，AMD `rocm-smi`、sysfs 设备树模型与 Windows/非 Linux 采集尚未覆盖；
- 未设置 `custom_ipv4/ipv6` 且未启用 `get_ip_addr_from_nic` 时，基础信息中的 IP 为空
  （Go 会调用外部 IP 查询服务，这里刻意避免引入外网依赖）；
- 重试耗尽后由 CLI 退出，而不是像 Go root 命令那样立即进入下一轮
  `UpdateBasicInfo + EstablishWebSocketConnection`（见 D-013）。
