# agent-protocol

Komari Agent 的 **v1** 线协议模型 crate。纯函数、无 I/O：不建立连接、不读取系统信息、不访问时钟，
只负责把 Go 版本实际发送/接收的 JSON 编解码出来，供 `agent-client`、`agent-monitor` 使用。

v1 是裸 JSON，没有信封、没有压缩：

- 上报：`POST /api/clients/report`（或 WebSocket 文本帧）
- 基础信息：`POST /api/clients/uploadBasicInfo`

字段定义来自 Go 实现：

- `komari-agent/monitoring/monitoring.go`（`GenerateReport` 的真实 map 负载）
- `komari-agent/monitoring/unit/*.go`（各指标的实际 Go 类型）
- `komari-agent/server/basicInfo.go`、`server/websocket.go`
- 服务端 `komari/common/modules.go`（`common.Report`、`common.ClientInfo`）

## 有意不实现 v2

本 crate **只实现 v1**。Go agent 的 `protocol/v2`（JSON-RPC 2.0、`/api/clients/v2/rpc`、gzip）
已按用户确认整体删除：

- 当前 Go 后端没有 `/api/clients/v2/rpc` 路由，v2 在本项目中是无效路径；
- 保留它只会让后续任务误以为需要实现 v2，增加无效代码与依赖（`flate2`）。

**A05 及后续任务不要恢复 v2**，除非后端先提供对应路由并重新确认需求。

## v1 `ReportV1` 字段对照表

Go 的 v1 上报是 `map[string]interface{}`（`monitoring.GenerateReport`），服务端用
`common.Report` 解码。Rust 用结构体建模，并把未识别的键放进 `extra`，保持第三方字段透传。

| JSON key | Rust 字段 | Rust 类型 | Go agent 实际类型 | 服务端 `common.Report` 类型 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `uuid` | `uuid` | `Option<String>` | 不发送 | `string`，`omitempty` | 服务端从 token 推导 |
| `cpu.usage` | `cpu.usage` | `f64` | `float64`，最小 0.001 | `float64`，`omitempty` | |
| `cpu.name` | `cpu.name` | `String` | 不发送 | `string`，`omitempty` | 服务端可选字段 |
| `cpu.cores` | `cpu.cores` | `i64` | 不发送 | `int`，`omitempty` | 服务端可选字段 |
| `cpu.arch` | `cpu.arch` | `String` | 不发送 | `string`，`omitempty` | 服务端可选字段 |
| `ram.total` / `ram.used` | `ram.total` / `ram.used` | `u64` | `uint64` | `int64` | 字节 |
| `swap.total` / `swap.used` | `swap.total` / `swap.used` | `u64` | `uint64` | `int64` | 字节 |
| `load.load1` / `load5` / `load15` | `load.load1` … | `f64` | `float64` | `float64` | |
| `disk.total` / `disk.used` | `disk.total` / `disk.used` | `u64` | `uint64` | `int64` | 字节 |
| `network.up` / `down` | `network.up` / `down` | `u64` | `uint64` | `int64` | 字节/秒 |
| `network.totalUp` / `totalDown` | `network.total_up` / `total_down` | `u64` | `uint64` | `int64` | camelCase 键名 |
| `connections.tcp` / `udp` | `connections.tcp` / `udp` | `i64` | `int` | `int` | |
| `gpu` | `gpu` | `Option<GpuReportV1>` | 仅 `--enable-gpu` 时发送 | `*GPUDetailReport`，`omitempty` | 有两种形状，见下 |
| `gpu.count` | `gpu.count` | `Option<i64>` | `int` | `int` | 仅详细模式 |
| `gpu.average_usage` | `gpu.average_usage` | `Option<f64>` | `float64` | `float64` | 仅详细模式；保留显式 0 |
| `gpu.detailed_info[]` | `gpu.detailed_info` | `Option<Vec<GpuDeviceInfoV1>>` | `[]map` | `[]GPUDeviceInfo` | 仅详细模式 |
| `gpu.models[]` | `gpu.models` | `Option<Vec<String>>` | `[]string` | 无对应字段 | 详细探测失败、型号探测成功时的回退形状 |
| `gpu.detailed_info[].name` | `.name` | `String` | `string` | `string` | |
| `gpu.detailed_info[].memory_total` / `memory_used` | 同名 | `u64` | `uint64` | `int64` | 字节 |
| `gpu.detailed_info[].utilization` | `.utilization` | `f64` | `float64` | `float64` | % |
| `gpu.detailed_info[].temperature` | `.temperature` | `u64` | `uint64` | `int` | °C |
| `uptime` | `uptime` | `u64` | `uint64` | `int64` | 秒 |
| `process` | `process` | `i64` | `int` | `int` | 进程数 |
| `message` | `message` | `String` | `string`（无 omitempty） | `string`（无 omitempty） | 总是出现，空值为 `""` |
| `method` | `method` | `Option<String>` | 不发送 | `string`，`omitempty` | 服务端填充 |
| `updated_at` | `updated_at` | `Option<String>` | 不发送 | `time.Time`（RFC 3339） | 保持字符串原样，避免重新格式化 |
| 其它未知键 | `extra` | `BTreeMap<String, Value>` | map 动态字段 | `encoding/json` 忽略未知键 | Rust 保留并原样回写 |

GPU 的两种 Go 实际输出（均由 `monitoring.GenerateReport` 产生）：

- 详细探测成功：
  `{"gpu":{"count":2,"average_usage":37.5,"detailed_info":[{"name":...,"memory_total":...,"memory_used":...,"utilization":...,"temperature":...}]}}`
- 详细探测失败但型号探测成功：
  `{"gpu":{"models":["NVIDIA GeForce RTX 4090","NVIDIA GeForce RTX 3090"]}}`

`GpuReportV1` 的四个分支字段都是 `Option`，序列化时只输出存在的字段，因此不会把
`models` 回退形状错误地补成零值 `count/average_usage/detailed_info`，也能保留显式的
`average_usage: 0`。

`omitempty` 语义：

- `ReportV1` 中只有 `uuid`、`gpu`、`method`、`updated_at` 会省略；其余字段即使为零值也会输出，
  与 `common.Report` 的 struct tag 一致。
- `cpu` 的四个字段都按 Go 的 `omitempty` 处理，因此零值不输出；`message` 无 `omitempty`，
  即使为空也输出 `""`。
- 缺失字段解码时一律取零值（`#[serde(default)]`），因为 Go 的 `encoding/json` 不会拒绝缺字段的
  report。
- 任意字段为 `null` 时按零值处理（`Option` 字段则为 `None`），与 Go `encoding/json` 把 `null`
  解到非指针字段时保持零值的行为一致。
- 顶层以及 `cpu`/`ram`/`swap`/`load`/`disk`/`network`/`connections`/`gpu` 和
  `gpu.detailed_info[]` 中的未知字段都会保留在各自的 `extra` 中，与 Go agent 的 map 透传一致。

## v1 `BasicInfoV1` 字段对照表

Go agent 在 `server.uploadBasicInfo` 中构造 `map[string]interface{}`；服务端
`UploadBasicInfo` 按 map 接收并追加 `uuid` / `region`。

| JSON key | Rust 字段 | Rust 类型 | Go 实际类型 | 说明 |
| --- | --- | --- | --- | --- |
| `cpu_name` | `cpu_name` | `String` | `string` | |
| `cpu_cores` | `cpu_cores` | `i64` | `int` | 逻辑核心数 |
| `cpu_physical_cores` | `cpu_physical_cores` | `Option<i64>` | `int` | 旧面板（<= 1.2.0）会删除该键，`None` 即不发送 |
| `arch` | `arch` | `String` | `string` | `runtime.GOARCH` 风格值（如 `amd64`） |
| `os` | `os` | `String` | `string` | |
| `kernel_version` | `kernel_version` | `Option<String>` | `string` | 旧面板（<= 1.0.2）会删除该键 |
| `ipv4` / `ipv6` | `ipv4` / `ipv6` | `String` | `string` | agent 会显式发送空字符串 |
| `mem_total` / `swap_total` / `disk_total` | 同名 | `u64` | `uint64` | 字节；服务端存 `int64` |
| `gpu_name` | `gpu_name` | `String` | `string` | |
| `virtualization` | `virtualization` | `String` | `string` | |
| `version` | `version` | `String` | `string` | agent 版本 |
| `uuid` | `uuid` | `Option<String>` | 服务端追加 | agent 不发送 |
| `region` | `region` | `Option<String>` | 服务端追加（GeoIP） | agent 不发送 |
| 其它未知键 | `extra` | `BTreeMap<String, Value>` | map 动态字段 | 保留并原样回写 |

与 `ReportV1` 一样，`BasicInfoV1` 的非 `Option` 字段也接受 JSON `null` 并按零值处理
（`Option` 字段则为 `None`），与 Go `encoding/json` 的行为一致。

`BasicInfoV1::default()` 等价于 Go 的**首次完整上传**：`cpu_physical_cores` 为 `Some(0)`、
`kernel_version` 为 `Some("")`，两个键都会出现在 JSON 中。只有模拟旧面板回退时才把它们设为
`None`，此时序列化结果与 Go 的 `delete(data, ...)` 一致。

## 与 Go 的已知差异

1. **不实现 v2**：见上文“有意不实现 v2”。这是用户确认的有意裁剪。
2. **键顺序**：Go 的 v1 report 是 map，`encoding/json` 按字母序输出；Rust 结构体按声明顺序输出。
   JSON 语义等价，测试按 `serde_json::Value` 比较。
3. **未知字段**：v1 模型在顶层和各 section 中保留未知字段（与 Go agent 的 map 透传一致）。
4. **`null`**：Go 接受任意字段为 `null` 并保持零值；Rust 通过 `null_default` 反序列化助手复刻。
5. **时间字段**：`updated_at` 用 `String` 原样保留 Go 的 RFC 3339 文本，不解析为时间类型，
   避免时区/纳秒格式被改写。
6. **整数值浮点的文本格式**：Go `encoding/json` 把 `float64(0)` 写成 `0`、`float64(1)` 写成 `1`；
   `serde_json` 写成 `0.0`、`1.0`。JSON 数值语义相同，服务端解析结果一致，但逐字节比对会不同。

## 用法

```rust
use agent_protocol::{decode_v1, encode_v1, ReportV1};

let report = decode_v1::<ReportV1>(r#"{"cpu":{"usage":1.5}}"#)?;
let body = encode_v1(&report)?;              // POST body / WebSocket text frame
assert!(body.contains(r#""usage":1.5"#));
# Ok::<(), agent_protocol::ProtocolError>(())
```

## 测试

`cargo test -p agent-protocol` 覆盖：

- 由 Go `encoding/json` 实际产出的 v1 report / basic info / GPU 两种形状 golden JSON 往返；
- 顶层与各 section 的未知字段透传、`omitempty` 省略、缺失字段默认值；
- `null` 顶层 section、嵌套标量按零值处理；
- `int64` / `uint64` 大整数（含 GPU 显存/温度的 `uint64::MAX`）与浮点精度；
- 顶层字段的类型错误与非法 JSON。
