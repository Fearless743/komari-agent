# agent-discovery 与 agent-update 行为说明

本文件记录 A06 两个 crate 与 Go 实现（`komari-agent`）的对应关系、验证方式和已知限制。
总体的差异登记见 [`parity-deviations.md`](parity-deviations.md)。

## agent-discovery

### DNS 解析

对应 Go：`dnsresolver/resolver.go`。

| Go 行为 | Rust 实现 |
| --- | --- |
| `custom_dns` 为空时使用 `net.DefaultResolver` | `DnsResolver` 直接使用 `tokio::net::lookup_host`（系统解析器） |
| 自定义 DNS 不可达时按 `DNSServers` 列表兜底 | `ResolverConfig::fallback_servers` 默认值与 Go 列表逐项一致 |
| 每个服务器 10s 拨号超时 | `ResolverConfig::timeout` 默认 10s，作用于每次查询/连接 |
| `LookupHost` 同时返回 A/AAAA | 并发查询 A 与 AAAA 后合并、去重 |
| `sortIPsByPreference` 固定优先 4/6 | `PreferIp::V4` / `PreferIp::V6`，稳定排序，保持族内原始顺序 |
| 未指定版本时检测本机 IPv4 | `PreferIp::Auto` 通过 `if-addrs` 查找非回环 IPv4；结果缓存 |
| `ConvertHostToASCII` / `ConvertIDNToASCII` | `agent_discovery::idna`，使用 `idna` crate |

DNS 错误分类（`DnsError`）：`Timeout`、`NxDomain`、`ServFail`、`Refused`、
`ResponseCode`、`Truncated`、`BadResponse`、`InvalidName`、`InvalidServer`、
`SystemLookup`、`AllServersFailed`。

实现细节：

- 自行构造/解析 DNS 报文，支持压缩指针、A/AAAA、NXDOMAIN、SERVFAIL/REFUSED。
- UDP 响应 `TC=1` 时自动用 TCP 重试；TCP 仍失败则返回 `Truncated`。
- 回退顺序：自定义服务器 → 内置/配置的 fallback 列表 → 系统解析器（可关闭）。
  Go 只回退到内置列表；系统解析器是任务要求的额外最后兜底。
- `NXDOMAIN` 视为权威结果，不再尝试其他 DNS 服务器。

### 自动发现

对应 Go：`cmd/autodiscovery.go`。

流程与 Go 一致：

1. 从可执行文件同目录的 `auto-discovery.json` 读取 `{uuid, token}`；
2. 文件不存在或损坏时向 `{endpoint}/api/clients/register?name={hostname}` 发送
   `POST {"key":"..."}`；
3. 校验 HTTP 状态必须为 `200`、`status` 必须为 `success`；
4. 以两空格缩进 JSON 保存 `{uuid, token}` 并返回给调用方。

请求头：

- `Content-Type: application/json`
- `Authorization: Bearer <auto_discovery_key>`
- `CF-Access-Client-Id` / `CF-Access-Client-Secret`（两者均非空时）

HTTP 传输可注入（`HttpClient` trait）。默认实现 `ReqwestHttpClient` 会先用
`DnsResolver` 解析 endpoint 主机名，再按 IP 优先顺序逐个尝试；TLS SNI 仍使用域名。
endpoint 中的国际化域名会先转换为 Punycode。

`auto-discovery.json` 的默认位置是当前可执行文件所在目录；测试通过
`AutoDiscoveryOptions::config_path` 注入临时路径，不会写入真实目录。

### 关于 “manifest”

Go 版 `komari-agent` 没有 manifest 接口或 manifest 文件：自动发现只持久化
`auto-discovery.json`，更新只访问 GitHub Releases API。Rust 版本保持同样范围，
未额外引入 manifest 协议。

## agent-update

对应 Go：`update/update.go`（以及 `go-github-selfupdate` 的默认行为）。

| Go 行为 | Rust 实现 |
| --- | --- |
| `parseVersion` 去掉单个 `v`/`V`，tolerant SemVer | `parse_version`，支持 `1`、`1.2`、预发布、构建元数据 |
| `latest.Compare(current) > 0` | `need_update` 使用 `cmp_precedence`，构建元数据不参与比较 |
| `go-github-selfupdate` 查询 GitHub latest | `Updater::check_latest` 查询 `GET /repos/{repo}/releases/latest` |
| 按 `GOOS`/`GOARCH` 选择 asset | `select_asset`，优先 `komari-agent-{os}-{arch}[.exe]` |
| 下载后替换当前二进制 | `replace_file_atomically`，临时文件 + 权限复制 + rename |
| 更新成功后 `os.Exit(42)` | 返回 `UpdateOutcome::Updated`，调用方使用 `RESTART_EXIT_CODE = 42` |
| 每 6 小时检查 | `CHECK_INTERVAL = 6h`，循环由 CLI/服务层负责 |

安全与校验：

- `disable_auto_update = true` 时直接返回 `UpdateOutcome::Disabled`，不发起任何请求。
- 下载后校验：非空、最大大小（默认 256 MiB）、GitHub asset `size`（非 0 时严格相等）、
  可选 `digest: sha256:...`。
- 拒绝明显是 HTML/XML/JSON 错误页的下载内容（`InvalidBinary`）。
- 只替换显式配置的 `target_path`；`check_and_update` 在没有目标路径时返回
  `TargetNotConfigured`，避免意外覆盖正在运行的进程。
- 原子替换会复制目标文件权限（Unix），写入后 `fsync`，再 `rename`；失败时清理临时文件。

## 测试

```bash
cargo test -p agent-discovery -p agent-update
```

- DNS：本地 mock UDP 服务器覆盖 A/AAAA、IPv4/IPv6 排序、NXDOMAIN、超时、截断、
  SERVFAIL 回退、系统解析器兜底、IP 字面量；纯函数覆盖报文解析与服务器地址规范化。
- 自动发现：手写本地 HTTP/1.1 服务器覆盖成功保存、401、非法 JSON、业务失败、超时、
  已有配置短路、损坏配置重建。
- 更新：注入 `UpdateHttp` 覆盖版本表、asset 选择、非 2xx、非法 JSON、缺 asset、
  下载传输失败、大小/校验和不匹配、HTML 拒绝、超大下载、成功替换与权限保留。
- 所有替换测试都在 `tempfile::tempdir()` 中进行，不会触碰测试进程二进制。

## 已知限制

1. **CNAME-only 响应**：自定义 DNS 查询只提取响应中的 A/AAAA 记录。递归服务器通常
   会返回 CNAME 链及其 A/AAAA；若服务器只返回 CNAME，当前实现会得到 `NoRecords`。
2. **HTTP 重定向**：自动发现首次请求使用自定义解析器；若面板重定向到另一主机，
   `reqwest` 使用系统解析器处理重定向目标。
3. **Windows 替换**：Windows 不允许 rename 覆盖运行中的 exe。实现会先把旧文件移开，
   再安装新文件；旧文件若仍被占用，删除会延迟到后续运行。
4. **更新来源**：仅支持 GitHub Releases API；`api_base` 可指向 GitHub Enterprise，
   但不支持任意自定义更新服务器。
5. **IDN 边界**：Rust 使用 `idna` crate，Go 使用 `golang.org/x/net/idna`；极端非法域名
   的接受/拒绝集合可能不同，常规 Punycode 转换一致。
