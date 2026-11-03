# komari-agent-rs

Komari Agent 的 Rust 增量重写版本。目标是与 [`../komari-agent`](../komari-agent)（Go 实现）达到
100% 功能对等；在完成对等验证之前，Go 版本保留作为回退实现。

当前阶段已包含 **workspace 骨架、CLI、配置加载、日志与版本信息、DNS/自动发现、自动更新**；
采集、协议与 WebSocket 传输仍在后续任务中。

## 目录结构

```
Cargo.toml                 # workspace（edition 2024 / resolver 3）+ 统一依赖版本
crates/
  agent-cli/               # 唯一可执行 crate，二进制名 komari-agent
  agent-config/            # 配置结构、默认值、环境变量、JSON 文件与 CLI 覆盖
  agent-monitor/           # 系统指标采集（后续任务）
  agent-protocol/          # 上报协议 v1/v2（后续任务）
  agent-client/            # HTTP / WebSocket 传输（后续任务）
  agent-discovery/         # 自定义 DNS 解析与自动发现注册
  agent-update/            # GitHub Release 版本检查、下载校验与原子替换
```

后续任务通常只修改各自 crate，不需要再改根 `Cargo.toml`：常用依赖已统一预声明在
`[workspace.dependencies]`，个别缺失依赖仍可在该 crate 的 `Cargo.toml` 中直接添加。

## 环境要求

- Rust 1.95 或更高（`edition = "2024"`，`resolver = "3"`）

## 构建与测试

```bash
cargo fmt --all
cargo build
cargo test
cargo run -p agent-cli -- --help
```

单独验证发现与更新 crate：

```bash
cargo test -p agent-discovery -p agent-update
cargo clippy -p agent-discovery -p agent-update --all-targets -- -D warnings
```

发布构建：

```bash
cargo build --release
# 产物：target/release/komari-agent
```

## 版本信息

默认版本取 `CARGO_PKG_VERSION`（当前为 `0.0.1`），可在编译期覆盖：

```bash
KOMARI_AGENT_VERSION=1.2.3 \
KOMARI_AGENT_VERSION_HASH=$(git rev-parse --short HEAD) \
cargo build --release
```

```console
$ komari-agent --version
komari-agent 1.2.3 (a1b2c3d)
```

启动日志同样会输出 `Komari Agent <version>`。

## 配置

字段名、JSON 键名、环境变量名与默认值均对齐 Go 的
`cmd/flags/flag.go`、`cmd/root.go`。

优先级（从高到低）：

1. 命令行参数
2. 环境变量（`AGENT_*`，另有 `HOST_PROC`）
3. JSON 配置文件（`--config` 或 `AGENT_CONFIG_FILE`）
4. 内置默认值

配置文件为可选：未指定时使用默认值；指定但不存在时进程会以退出码 `1` 退出并打印明确错误。
JSON 中未出现的键不会覆盖更高优先级的值。

示例：

```json
{
  "token": "your-token",
  "endpoint": "https://panel.example.com",
  "interval": 1.0,
  "ignore_unsafe_cert": false,
  "max_retries": 3,
  "reconnect_interval": 5,
  "info_report_interval": 5,
  "protocol_version": 2
}
```

```bash
./komari-agent --config ./config.json
AGENT_TOKEN=your-token AGENT_ENDPOINT=https://panel.example.com ./komari-agent
```

完整参数见 `komari-agent --help`。启动时打印的配置中，`token`、
`cf_access_client_secret` 与 `auto_discovery_key` 会被脱敏为 `******`。

兼容性说明：

- **配置优先级与 Go 相反**：Rust 为 `flag > 环境变量 > JSON > 默认值`，Go 的实际顺序为
  `JSON > 环境变量 > flag > 默认值`（见 Go `cmd/root.go` 中先 `loadFromEnv()` 再
  `json.Unmarshal`）。这会带来两类迁移问题：
  - unit 文件写死 `--token`、运行时用 `AGENT_TOKEN` 轮换时，Go 会使用环境变量，而 Rust 仍使用
    flag；请从 unit 文件中移除 `--token`，改用环境变量或配置文件提供。
  - 依赖 JSON 配置文件覆盖命令行参数的部署，在 Rust 中需要删除冲突的 flag 或将值改由
    环境变量/命令行提供。
  该差异已登记在 [`docs/parity-deviations.md`](docs/parity-deviations.md)（D-001），待与 Go 对等验收确认。
- Go 版本会静默忽略未知参数；Rust 版本默认对未知参数报错（退出码 `2`），同样登记为 D-002。
- 位置参数与 Go 一致：接受但忽略。
- 布尔参数仅支持 `--flag`、`--flag=true`、`--flag=false` 三种形式（短参数为 `-u`、`-u=false`）：
  - `--flag` 等价于 `--flag=true`；
  - 紧跟其后的 token 不会被当作布尔值吞掉，而是作为位置参数被忽略；
  - 等号后的取值只接受小写 `true`/`false`，`--flag=1`、`--flag=TRUE`、`--flag=t` 等会以退出码 `2`
    报错（环境变量仍遵循 Go 的 `true`/`1` 语义）。该差异已登记在
    [`docs/parity-deviations.md`](docs/parity-deviations.md)（D-003）。
- `--autoUpdate` 与 `--memory-mode-available` 已弃用，仍会被接受并打印警告后忽略。
- `check-mem`、`list-disk` 子命令依赖采集实现，将在后续任务中补齐。

## 自动发现与自动更新

- `agent-discovery`
  - `dns`：自定义 DNS 服务器（`custom_dns`）、A/AAAA 查询、超时/NXDOMAIN/截断错误分类、
    IPv4/IPv6 优先排序、内置服务器与系统解析器回退。
  - `autodiscovery`：读取/保存 `auto-discovery.json`，使用 `auto_discovery_key` 向面板注册，
    支持 Cloudflare Access 头、IDN endpoint 与可注入 HTTP 传输。
- `agent-update`
  - 查询 GitHub Releases、SemVer 比较、按 `GOOS`/`GOARCH` 选择 asset、下载大小与可选
    SHA-256 校验、临时文件 + rename 原子替换并保留权限。
  - `disable_auto_update` 会短路；更新成功后由调用方以退出码 42 重启（见 `RESTART_EXIT_CODE`）。

详细的行为对照、测试矩阵与已知限制见
[`docs/agent-discovery-update.md`](docs/agent-discovery-update.md)。
