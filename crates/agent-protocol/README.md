# agent-protocol

Komari Agent 的线协议模型 crate。**纯函数、无 I/O**：不建立连接、不读取系统信息、不访问时钟，
只负责把 Go 版本实际发送/接收的字节编码、解码出来，供 `agent-client`、`agent-monitor` 使用。

- v1：裸 JSON。上报发到 `/api/clients/report`，基础信息发到 `/api/clients/uploadBasicInfo`。
- v2：同一个 v1 负载外套 JSON-RPC 2.0 信封；HTTP POST 回退路径可选 gzip。

字段定义来自 Go 实现：

- `komari-agent/monitoring/monitoring.go`（`GenerateReport` 的真实 map 负载）
- `komari-agent/monitoring/unit/*.go`（各指标的实际 Go 类型）
- `komari-agent/server/basicInfo.go`、`server/websocket.go`
- `komari-agent/protocol/v2/jsonrpc.go`、`protocol/transport/compress.go`
- 服务端 `komari/common/modules.go`（`common.Report`、`common.ClientInfo`）
- 服务端 `komari/utils/rpc/*`（请求/响应/错误码）

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
| `gpu` | `gpu` | `Option<GpuDetailReportV1>` | 仅 `--enable-gpu` 时发送 | `*GPUDetailReport`，`omitempty` | 见下 |
| `gpu.count` | `gpu.count` | `i64` | `int` | `int` | |
| `gpu.average_usage` | `gpu.average_usage` | `f64` | `float64` | `float64` | |
| `gpu.detailed_info[]` | `gpu.detailed_info` | `Vec<GpuDeviceInfoV1>` | `[]map` | `[]GPUDeviceInfo` | |
| `gpu.detailed_info[].name` | `.name` | `String` | `string` | `string` | |
| `gpu.detailed_info[].memory_total` / `memory_used` | 同名 | `i64` | `int64` | `int64` | 字节 |
| `gpu.detailed_info[].utilization` | `.utilization` | `f64` | `float64` | `float64` | % |
| `gpu.detailed_info[].temperature` | `.temperature` | `i64` | `int` | `int` | °C |
| `uptime` | `uptime` | `u64` | `uint64` | `int64` | 秒 |
| `process` | `process` | `i64` | `int` | `int` | 进程数 |
| `message` | `message` | `String` | `string`（无 omitempty） | `string`（无 omitempty） | 总是出现，空值为 `""` |
| `method` | `method` | `Option<String>` | 不发送 | `string`，`omitempty` | 服务端填充 |
| `updated_at` | `updated_at` | `Option<String>` | 不发送 | `time.Time`（RFC 3339） | 保持字符串原样，避免重新格式化 |
| 其它未知键 | `extra` | `BTreeMap<String, Value>` | map 动态字段 | `encoding/json` 忽略未知键 | Rust 保留并原样回写 |

`omitempty` 语义：

- `ReportV1` 中只有 `uuid`、`gpu`、`method`、`updated_at` 会省略；其余字段即使为零值也会输出，
  与 `common.Report` 的 struct tag 一致。
- `cpu` 的四个字段都按 Go 的 `omitempty` 处理，因此零值不输出；`message` 无 `omitempty`，
  即使为空也输出 `""`。
- 缺失字段解码时一律取零值（`#[serde(default)]`），因为 Go 的 `encoding/json` 不会拒绝缺字段的
  report。

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

## v2 JSON-RPC

常量与 Go `protocol/v2` 一致：

| 常量 | 值 |
| --- | --- |
| `JSONRPC_VERSION` / `RPC_VERSION` | `"2.0"` |
| `METHOD_AGENT_REPORT` | `"agent.report"` |
| `METHOD_AGENT_BASIC_INFO` | `"agent.basicInfo"` |

信封：

```json
{"jsonrpc":"2.0","method":"agent.report","params":{"report":{...}}}
{"jsonrpc":"2.0","method":"agent.basicInfo","params":{"info":{...}}}
```

- 无 `id` 即 notification（WebSocket 路径）；POST 回退路径使用
  `{"id":"report-<UnixNano>"}`。
- `params` / `id` / `result` / `error.data` 都是任意 JSON 值（Go 为 `interface{}`）；
  解码为 `serde_json::Value`，支持命名参数对象、位置参数数组与标量。
- `id` 为 `null` 或缺失都视为 notification，与 Go 的 `HasID()`（`ID != nil`）一致。
- `RpcRequest::validate()` 复刻 Go 的 `Validate()`：`jsonrpc` 必须为 `"2.0"`，`method` 不能为空，
  失败返回 `INVALID_REQUEST` 错误对象。
- 未知成员在 v2 信封中被忽略（Go `encoding/json` 的行为）；v1 负载中的未知字段仍会保留。

错误码与 `komari/utils/rpc/errors.go` 完全一致：

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `PARSE_ERROR` | -32700 | JSON 解析失败 |
| `INVALID_REQUEST` | -32600 | 不是合法 Request |
| `METHOD_NOT_FOUND` | -32601 | 方法不存在 |
| `INVALID_PARAMS` | -32602 | 参数非法 |
| `INTERNAL_ERROR` | -32603 | 内部错误 |
| `UNAUTHENTICATED` | -32040 | 未认证 |
| `PERMISSION_DENIED` | -32041 | 无权限 |
| `NOT_FOUND` | -32044 | 资源不存在 |
| `ALREADY_EXISTS` | -32045 | 资源已存在 |
| `CANCELLED` | -32010 | 已取消 |
| `DEADLINE_EXCEEDED` | -32011 | 超时 |
| `ABORTED` | -32021 | 并发冲突/事务中断 |
| `OUT_OF_RANGE` | -32022 | 越界 |
| `UNIMPLEMENTED` | -32050 | 未实现 |
| `UNAVAILABLE` | -32051 | 依赖不可用 |
| `DATA_LOSS` | -32052 | 数据丢失 |

## gzip

与 Go `protocol/transport/compress.go` 的 `GzipBytes` 对齐：

- 默认压缩级别 6（Go `DefaultCompression`，即 `gzip.NewWriter`）。
- gzip header 固定 `mtime = 0`、`OS = 255`（未知），与 Go 写出的前 10 字节完全一致。
- 解压使用多 member 模式，与 Go `gzip.Reader` 默认的 multistream 行为一致。
- 空输入返回 `ProtocolError::EmptyInput`；损坏的 header、DEFLATE 流、CRC 或 ISIZE 返回
  `ProtocolError::Decompress`。
- `compress_with_level` 只接受 `0..=9`；Go 的 `-2`（HuffmanOnly）在 `flate2` 中没有对应模式，
  且 agent 从不使用，因此不提供。

## v1 与 v2 的差异

| 维度 | v1 | v2 |
| --- | --- | --- |
| 信封 | 无，直接是 report / basic info 对象 | JSON-RPC 2.0 |
| 端点 | `/api/clients/report`、`/api/clients/uploadBasicInfo` | `/api/clients/v2/rpc` |
| WebSocket 帧 | 裸 JSON 文本帧 | JSON-RPC notification 文本帧 |
| HTTP 压缩 | 不压缩 | `Content-Encoding: gzip`（可禁用） |
| 响应 | `{"status":"success"}` 或错误 | JSON-RPC Response（含 `id`/`result`/`error`） |
| 基础信息 | 直接 map | `params.info` |

## 与 Go 的已知差异

1. **v2 大整数精度（D-004）**：Go 的 `v2.BuildReportPayload` 先把 v1 JSON 反序列化到
   `interface{}`，所有数字都变成 `float64`，因此超过 2^53 的整数会被舍入（例如
   `9223372036854775807` → `9223372036854776000`）。本 crate 直接把 `ReportV1` 序列化进
   `params.report`，保留精确整数。字段名、类型和结构不变；服务端按 `int64` 解析两者都能接受。
   详见根目录 `docs/parity-deviations.md`。
2. **键顺序**：Go 的 v1 report 是 map，`encoding/json` 按字母序输出；Rust 结构体按声明顺序输出。
   JSON 语义等价，测试按 `serde_json::Value` 比较。
3. **未知字段**：v1 模型保留未知字段（与 Go agent 的 map 透传一致）；v2 信封忽略未知成员
   （与 Go `encoding/json` 一致）。
4. **时间字段**：`updated_at` 用 `String` 原样保留 Go 的 RFC 3339 文本，不解析为时间类型，
   避免时区/纳秒格式被改写。

## 用法

```rust
use agent_protocol::{decode_v1, encode_v1, ReportV1, report_notification, compress};

let report = decode_v1::<ReportV1>(r#"{"cpu":{"usage":1.5}}"#)?;
let v1_bytes = encode_v1(&report)?;
let v2_body = report_notification(&report)?;
let gzipped = compress(v2_body.as_bytes())?;

let response = agent_protocol::RpcResponse::from_json(r#"{"jsonrpc":"2.0","id":1,"result":"ok"}"#)?;
assert_eq!(response.result.unwrap(), "ok");
# Ok::<(), agent_protocol::ProtocolError>(())
```

## 测试

`cargo test -p agent-protocol` 覆盖：

- 由 Go `encoding/json` 实际产出的 v1 report / basic info / v2 信封 golden JSON 往返；
- 未知字段透传、`omitempty` 省略、缺失字段默认值；
- `int64` / `uint64` 大整数与浮点精度；
- Go `compress/gzip` 固定字节的解压互操作、gzip 往返、多 member、空输入与损坏数据；
- 全部 JSON-RPC 错误码与请求校验。
