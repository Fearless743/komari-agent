# 与 Go 实现的已知差异登记

本文件登记 Rust 重写版相对 Go 版（`komari-agent`）**有意保留**的行为差异，供后续功能对等验收逐项确认。
每条差异都包含原因、影响与迁移方式，便于部署方评估。

## D-001 配置优先级顺序不同

- **Rust**：命令行 flag > 环境变量 > JSON 配置文件 > 默认值
- **Go**：JSON 配置文件 > 环境变量 > 命令行 flag > 默认值

原因：Rust 版按本项目任务规范实现，要求命令行显式参数具有最高优先级。

影响与迁移方式：

1. **unit 文件写死 `--token`、运行时用 `AGENT_TOKEN` 轮换**
   - Go：环境变量在 flag 之后加载，`AGENT_TOKEN` 会覆盖 unit 文件里的 `--token`。
   - Rust：`--token` 优先级更高，环境变量不会生效，token 轮换会失效。
   - 迁移：从 unit 文件中删除 `--token`，改为只通过 `AGENT_TOKEN`（或配置文件）提供 token。
2. **依赖 JSON 文件覆盖命令行参数**
   - Go：只要配置文件中写了某个键，就会覆盖命令行传入的同名参数。
   - Rust：命令行参数优先，配置文件无法覆盖显式 flag。
   - 迁移：删除冲突的命令行参数，或把期望生效的值改到环境变量/命令行中。

状态：有意差异，等待与 Go 对等验收时最终确认（若届时要求与 Go 完全一致，仅需调整
`agent-config` 的覆盖顺序及对应测试）。

## D-002 未知命令行参数的处理不同

- **Rust**：未知参数交由 clap 报错，退出码 `2`。
- **Go**：通过 `ParseErrorsWhitelist.UnknownFlags` 静默忽略未知参数。

原因：严格校验可以尽早暴露拼写错误，避免部署脚本中的失效参数被悄悄忽略。

影响与迁移方式：删除部署脚本中已失效或拼错的参数；确需兼容旧脚本时可在验收阶段改为忽略策略。

状态：有意差异，待对等验收确认。

## D-003 布尔参数接受的字面量不同

- **Go**：pflag 的布尔 flag 使用 `strconv.ParseBool`，接受
  `1`、`t`、`T`、`TRUE`、`true`、`True`、`0`、`f`、`F`、`FALSE`、`false`、`False`。
- **Rust**：clap 的布尔解析器只接受小写 `true` / `false`。支持的形式为
  `--flag`（等价于 `--flag=true`）、`--flag=true`、`--flag=false`；
  `--flag=1`、`--flag=TRUE`、`--flag=t` 等会报 `invalid value` 并以退出码 `2` 退出。

影响：旧部署脚本如果写 `--disable-auto-update=1`、`--ignore-unsafe-cert=TRUE` 等，
在 Go 版可以正常工作，在 Rust 版会直接失败而不是启用该选项。

迁移建议：把等号后的值统一改为小写 `true` / `false`，例如
`--disable-auto-update=1` → `--disable-auto-update=true`，
`--ignore-unsafe-cert=TRUE` → `--ignore-unsafe-cert=true`；
仅表示启用时可直接省略取值（`--flag`）。

说明：该限制只作用于命令行 flag 取值；环境变量布尔值仍按 Go 语义处理
（`true`/`1` 启用，其他值保持原值）。

状态：**用户确认的有意差异**（A01 决策：保持 Rust 现状，只接受 `true`/`false`，不扩展解析器）。

## D-004 v2 上报信封中的大整数不经过 float64 舍入

- **Go**：`protocol/v2.BuildReportPayload` / `BuildReportRequest` 先把 v1 report 字节
  `json.Unmarshal` 到 `interface{}`，所有数字都变成 `float64`，再 `json.Marshal`。因此超过
  2^53 的整数会被舍入，例如 `"totalDown":9223372036854775807` 实际发出
  `9223372036854776000`，`9007199254740993` 实际发出 `9007199254740992`。
- **Rust**：`report_notification` / `report_request` 直接把 `ReportV1` 序列化到
  `params.report`，整数保持精确。

原因：Rust 的 serde 模型已经是强类型，没有再走 `interface{}` 的必要；保留精确值可以避免
流量统计、uptime 等字段在 v1 与 v2 路径上出现不同的数值。

影响：v2 路径上超过 2^53 的整数与 Go 发出的值不同。字段名、结构、JSON 整数类型不变，服务端
`common.Report` 用 `int64` 解析两者都能成功；只有当服务端或第三方对 v2 上报值做逐字节比对时
才会观察到差异。当前真实指标（内存、磁盘、月流量、uptime）远小于 2^53，功能性影响可忽略。

状态：有意差异，实现更精确；待与 Go 对等验收确认是否要求逐字节复刻 Go 的舍入行为。
