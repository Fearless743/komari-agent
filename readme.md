# komari-agent

Komari Agent 的 Rust 重写版本。目标是与 [`../komari-agent`](../komari-agent)（Go 实现）达到
100% 功能对等；在完成对等验证之前，Go 版本保留作为回退实现。

当前已包含 workspace、CLI、配置加载、系统采集、协议 v1、DNS/自动发现、自动更新，以及完整的
v1 WebSocket 上报链路。协议 v2 已按用户确认裁剪（后端无 `/api/clients/v2/rpc` 路由），
详见 [`crates/agent-protocol/README.md`](crates/agent-protocol/README.md)。

- 安装脚本：[`install.sh`](install.sh)（Linux/macOS/FreeBSD）、[`install.ps1`](install.ps1)（Windows）
- 交叉编译：[`build_all.sh`](build_all.sh)、[`build_all.ps1`](build_all.ps1)
- 容器：[`Dockerfile`](Dockerfile)
- 与 Go 的差异登记：[`docs/parity-deviations.md`](docs/parity-deviations.md)

## 快速安装

### Linux / macOS / FreeBSD

从仓库获取脚本后以 root 运行（脚本会检测架构、下载 Release 资产并注册系统服务）：

```bash
curl -fsSLO https://raw.githubusercontent.com/fearless743/komari-agent/main/install.sh
chmod +x install.sh
sudo ./install.sh --endpoint https://panel.example.com --token YOUR_TOKEN
```

也可以克隆仓库后执行 `sudo ./install.sh ...`。脚本默认行为：

| 项目 | 默认值 |
| --- | --- |
| 服务名 | `komari-agent` |
| 安装目录 | Linux/FreeBSD：`/opt/komari`；macOS：`/usr/local/komari`（不可写时 `~/.komari`） |
| 二进制 | `<安装目录>/komari-agent` |
| 配置文件 | `<安装目录>/config.json`（`0600`，内含 endpoint/token） |
| systemd unit | `/etc/systemd/system/komari-agent.service` |
| 日志 | 由 journald/系统服务管理器收集 |

脚本支持 systemd、OpenRC（Alpine/Gentoo）、procd（OpenWrt）、launchd（macOS）、
upstart（CentOS 6）和 FreeBSD rc.d。NixOS 需要按脚本输出的片段手工声明服务。

安装完成后：

```bash
systemctl status komari-agent
journalctl -u komari-agent -f
```

### Windows

以**管理员**身份打开 PowerShell：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 --endpoint https://panel.example.com --token YOUR_TOKEN
```

默认安装到 `%ProgramFiles%\Komari`，通过 [nssm](https://nssm.cc) 注册为 Windows 服务
（缺失时自动下载 nssm 2.24）。`config.json` 使用无 BOM UTF-8，并通过 `icacls` 限制为
`SYSTEM` 与 `Administrators`；服务日志写入 `<安装目录>\komari-agent.log`（10 MiB 轮转）。

### Docker

```bash
docker build -t komari-agent .
docker run --rm komari-agent --version
```

发布工作流会推送 `ghcr.io/fearless743/komari-agent:<tag>` 与 `:latest` 多架构镜像
（linux/amd64、linux/arm64）。完整示例见 [Docker 部署](#docker-部署)。

## 安装脚本参数

Unix 与 Windows 脚本使用同一套选项：

| 选项 | 说明 |
| --- | --- |
| `--endpoint URL` | 面板地址，写入 `config.json` |
| `--token TOKEN` | 面板 token，写入 `config.json`（不会打印） |
| `--interval SECONDS` | 上报间隔，写入 `config.json` |
| `--install-dir DIR` | 安装目录 |
| `--install-service-name NAME` | 服务名（默认 `komari-agent`） |
| `--service NAME` | `--install-service-name` 的别名 |
| `--install-ghproxy URL` | GitHub 代理前缀，例如 `https://ghproxy.net` |
| `--install-version VERSION` | 指定 Release tag，例如 `v0.2.0`；默认最新版 |
| `--config FILE` | 使用已有配置文件，不再生成 `<安装目录>/config.json` |
| `--uninstall` | 停止并删除服务与二进制 |
| `--purge` | 与 `--uninstall` 一起使用时，同时删除 `config.json` 与 `auto-discovery.json` |
| `--dry-run` | 只打印计划，不修改系统 |
| `-h`, `--help` | 显示帮助 |

其余未知选项会原样传给 agent，例如：

```bash
sudo ./install.sh --endpoint https://panel.example.com --token TOKEN \
  --custom-dns 8.8.8.8 --gpu --disable-auto-update
```

安全与幂等行为：

- token 只写入 `config.json`，不会出现在安装日志、unit/nssm 参数或 `ps` 中；
- 启动日志与 `--help` 输出中的 `token`、`cf_access_client_secret`、`auto_discovery_key`
  会被脱敏为 `******`；
- 重复运行会停止旧服务、替换二进制，并保留配置文件中未被新参数覆盖的键；
- 下载失败不会删除旧二进制（先下载到临时文件，成功后才替换）；
- 可用 `--dry-run` 预演参数解析、下载 URL 和服务路径。

## 配置

字段名、JSON 键名、环境变量名与默认值对齐 Go 的 `cmd/flags/flag.go`、`cmd/root.go`。
优先级（从高到低）：

1. 命令行参数
2. 环境变量（`AGENT_*`，另有 `HOST_PROC`）
3. JSON 配置文件（`--config` 或 `AGENT_CONFIG_FILE`）
4. 内置默认值

配置文件为可选；指定但不存在时进程以退出码 `1` 退出并打印明确错误。JSON 中未出现的键不会
覆盖更高优先级的值。

### 示例

```json
{
  "endpoint": "https://panel.example.com",
  "token": "your-token",
  "interval": 1.0,
  "ignore_unsafe_cert": false,
  "max_retries": 3,
  "reconnect_interval": 5,
  "info_report_interval": 5,
  "protocol_version": 1
}
```

```bash
./komari-agent --config ./config.json
AGENT_TOKEN=your-token AGENT_ENDPOINT=https://panel.example.com ./komari-agent
```

### 配置项与默认值

| JSON 键 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `auto_discovery_key` | `AGENT_AUTO_DISCOVERY_KEY` | `""` | 自动发现密钥；设置后可省略 endpoint/token |
| `disable_auto_update` | `AGENT_DISABLE_AUTO_UPDATE` | `false` | 关闭 6 小时自动更新 |
| `memory_mode_available` | `AGENT_MEMORY_MODE_AVAILABLE` | `false` | 已弃用，仅为兼容保留 |
| `token` | `AGENT_TOKEN` | `""` | 面板 token，日志脱敏 |
| `endpoint` | `AGENT_ENDPOINT` | `""` | 面板地址 |
| `interval` | `AGENT_INTERVAL` | `1.0` | 上报间隔（秒） |
| `ignore_unsafe_cert` | `AGENT_IGNORE_UNSAFE_CERT` | `false` | 忽略 TLS 证书错误 |
| `max_retries` | `AGENT_MAX_RETRIES` | `3` | WebSocket 重连次数 |
| `reconnect_interval` | `AGENT_RECONNECT_INTERVAL` | `5` | 重连间隔（秒） |
| `info_report_interval` | `AGENT_INFO_REPORT_INTERVAL` | `5` | 基础信息上传间隔（分钟） |
| `include_nics` | `AGENT_INCLUDE_NICS` | `""` | 包含的网卡，逗号分隔 |
| `exclude_nics` | `AGENT_EXCLUDE_NICS` | `""` | 排除的网卡，逗号分隔 |
| `include_mountpoints` | `AGENT_INCLUDE_MOUNTPOINTS` | `""` | 包含的挂载点，分号分隔 |
| `month_rotate` | `AGENT_MONTH_ROTATE` | `0` | 月流量重置日；`0` 关闭 |
| `cf_access_client_id` | `AGENT_CF_ACCESS_CLIENT_ID` | `""` | Cloudflare Access Client ID |
| `cf_access_client_secret` | `AGENT_CF_ACCESS_CLIENT_SECRET` | `""` | Cloudflare Access Secret，日志脱敏 |
| `memory_include_cache` | `AGENT_MEMORY_INCLUDE_CACHE` | `false` | 内存统计包含 cache/buffer |
| `memory_report_raw_used` | `AGENT_MEMORY_REPORT_RAW_USED` | `false` | CLI 对应 `--memory-exclude-bcf` |
| `custom_dns` | `AGENT_CUSTOM_DNS` | `""` | 自定义 DNS 服务器 |
| `enable_gpu` | `AGENT_ENABLE_GPU` | `false` | 详细 GPU 采集，CLI 对应 `--gpu` |
| `custom_ipv4` | `AGENT_CUSTOM_IPV4` | `""` | 固定上报 IPv4 |
| `custom_ipv6` | `AGENT_CUSTOM_IPV6` | `""` | 固定上报 IPv6 |
| `get_ip_addr_from_nic` | `AGENT_GET_IP_ADDR_FROM_NIC` | `false` | 从网卡获取 IP |
| `host_proc` | `HOST_PROC` | `""` | `/proc` 根目录；无对应 CLI 参数 |
| `protocol_version` | `AGENT_PROTOCOL_VERSION` | `1` | 仅支持 v1；`>=2` 会降级并告警 |
| `disable_compression` | `AGENT_DISABLE_COMPRESSION` | `false` | 关闭 WebSocket 压缩（v2 gzip 未实现） |
| `prefer_ip_version` | `AGENT_PREFER_IP_VERSION` | `""` | `4` 或 `6`，空为自动 |

配置文件路径也可通过 `AGENT_CONFIG_FILE` 指定。完整 CLI 参数见 `komari-agent --help`，
包括 `--auto-discovery`、`--include-nics`、`--exclude-nics`、`--include-mountpoint`、
`--month-rotate`、`--cf-access-client-id`、`--cf-access-client-secret`、
`--memory-include-cache`、`--memory-exclude-bcf`、`--custom-dns`、`--gpu`、
`--custom-ipv4`、`--custom-ipv6`、`--get-ip-addr-from-nic`、`--protocol-version`、
`--disable-compression`、`--prefer-ip-version`。

### 兼容性说明

- **配置优先级与 Go 相反**：Rust 为 `flag > 环境变量 > JSON > 默认值`，Go 的实际顺序为
  `JSON > 环境变量 > flag > 默认值`。迁移时请从 unit 文件移除 `--token`，改用
  `AGENT_TOKEN` 或 `config.json`；依赖 JSON 覆盖命令行参数的部署需要删除冲突 flag。
  该差异登记为 D-001。
- Go 会静默忽略未知参数；Rust 默认报错并以退出码 `2` 退出（D-002）。
- 布尔参数只接受 `--flag`、`--flag=true`、`--flag=false`；`--flag=1`、`--flag=TRUE`
  会报错（D-003）。环境变量布尔值仍遵循 Go 的 `true`/`1` 语义。
- 位置参数与 Go 一致：接受但忽略。
- `--autoUpdate` 与 `--memory-mode-available` 已弃用，仍会被接受并打印警告后忽略。
- 协议版本仅支持 v1，显式 `2` 或更大值会降级为 `1` 并告警（D-004）。
- `check-mem`、`list-disk` 子命令已接入采集实现；`HOST_PROC` 同样作用于这两个命令。

## 服务管理

### systemd

安装脚本生成的 unit（`/etc/systemd/system/komari-agent.service`）等价于：

```ini
[Unit]
Description=Komari Agent Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/komari/komari-agent --config /opt/komari/config.json
WorkingDirectory=/opt/komari
Restart=always
RestartSec=5
User=root

[Install]
WantedBy=multi-user.target
```

`User=root` 与 Go 版一致，用于读取完整的 `/proc`、磁盘和 GPU 信息。若改为专用用户，
需要自行评估采集范围。

常用操作：

```bash
systemctl restart komari-agent
systemctl status komari-agent
journalctl -u komari-agent -n 100 --no-pager
```

### 轮换 token

直接编辑 `config.json` 后重启，或使用环境变量覆盖（环境变量优先级高于 JSON）：

```bash
sudo install -m 600 /dev/null /etc/komari/agent.env
echo 'AGENT_TOKEN=new-token' | sudo tee /etc/komari/agent.env >/dev/null
sudo systemctl edit komari-agent
```

在 drop-in 中加入：

```ini
[Service]
EnvironmentFile=/etc/komari/agent.env
```

然后 `sudo systemctl restart komari-agent`。不要把 token 写进 `ExecStart`，否则会出现在
`ps` 与服务定义中。

### 其他 init 系统

- **OpenRC**：`/etc/init.d/komari-agent`，`rc-service komari-agent {start,stop,restart}`。
- **procd（OpenWrt）**：`/etc/init.d/komari-agent`，`/etc/init.d/komari-agent {start,stop,restart}`。
- **launchd（macOS）**：`/Library/LaunchDaemons/com.komari.komari-agent.plist` 或
  `~/Library/LaunchAgents/...`，日志写入 `/var/log/komari-agent.log` 或
  `~/Library/Logs/komari-agent.log`。
- **FreeBSD rc.d**：`/usr/local/etc/rc.d/komari-agent`，
  `service komari-agent {start,stop,restart}`。
- **NixOS**：脚本只打印 `systemd.services.komari-agent` 片段，需加入 NixOS 配置后
  `sudo nixos-rebuild switch`。

## Docker 部署

镜像以 UID/GID `10001` 非 root 运行，`/etc/komari` 为配置卷，`/var/lib/komari` 为数据卷。
`/app` 归运行用户所有，用于自动发现文件和自更新。

### 使用配置文件

```bash
mkdir -p komari/config komari/data
cat > komari/config/config.json <<'JSON'
{
  "endpoint": "https://panel.example.com",
  "token": "your-token",
  "interval": 1.0
}
JSON
sudo chown -R 10001:10001 komari/config komari/data
sudo chmod 600 komari/config/config.json

docker run -d --name komari-agent --restart unless-stopped \
  -v "$PWD/komari/config:/etc/komari:ro" \
  -v "$PWD/komari/data:/var/lib/komari" \
  ghcr.io/fearless743/komari-agent:latest \
  --config /etc/komari/config.json
```

`docker-compose.yml` 示例：

```yaml
services:
  komari-agent:
    image: ghcr.io/fearless743/komari-agent:latest
    restart: unless-stopped
    command: ["--config", "/etc/komari/config.json"]
    volumes:
      - ./komari/config:/etc/komari:ro
      - komari-data:/var/lib/komari

volumes:
  komari-data:
```

### 仅使用环境变量

镜像默认 `CMD` 是 `--help`；用环境变量提供凭据时，传一个非敏感参数即可启动：

```bash
docker run -d --name komari-agent --restart unless-stopped \
  -e AGENT_TOKEN=YOUR_TOKEN \
  ghcr.io/fearless743/komari-agent:latest \
  --endpoint https://panel.example.com
```

token 只在环境变量中，不会出现在容器命令行。也可以把 `AGENT_*` 写进 compose 的
`environment`。

### 说明

- 自动发现文件 `auto-discovery.json` 固定写在可执行文件同目录（镜像内为 `/app`）。
  容器重建后该文件不会保留；长期部署建议显式配置 token，或确保自动发现注册可重复执行。
- 自更新需要可写 `/app`。若以只读根文件系统运行，请设置 `disable_auto_update` 并自行管理镜像升级。
- 镜像内不包含默认配置；未挂载配置且未提供 `AGENT_*` 时，agent 会以“缺少凭据”退出。

## 升级与卸载

### 脚本升级

```bash
sudo ./install.sh --install-version v0.3.0   # 指定版本
sudo ./install.sh                            # 最新版
```

脚本会停止旧服务、下载新二进制、保留 `config.json` 中未被覆盖的键，然后重启服务。
Windows 重新运行 `install.ps1` 即可。

### 自更新

`agent-update` 在启动时检查一次，之后每 6 小时检查 GitHub Releases，按
`komari-agent-{os}-{arch}[.exe]` 选择资产、校验大小（可选 SHA-256）并原子替换二进制。
更新成功后进程以退出码 `42` 退出，由 systemd 的 `Restart=always` 或 nssm 的
`AppExit Default Restart` 拉起新版本。`disable_auto_update=true` 可关闭。

### 卸载

```bash
sudo ./install.sh --uninstall          # 删除服务与二进制，保留配置
sudo ./install.sh --uninstall --purge  # 同时删除 config.json / auto-discovery.json
```

Windows：

```powershell
.\install.ps1 --uninstall --purge
```

## 故障排查

| 现象 | 排查方式 |
| --- | --- |
| 服务启动后立即退出 | `journalctl -u komari-agent -n 100`；检查 `config.json` 是否存在且为合法 JSON |
| `missing credentials` | 设置 `endpoint` + `token`，或配置 `auto_discovery_key` |
| 配置文件读取失败（退出码 `1`） | `--config`/`AGENT_CONFIG_FILE` 指向的文件不存在或无权限 |
| 未知参数（退出码 `2`） | 删除拼写错误的 flag；布尔值使用 `true`/`false` |
| 连接失败后反复重启 | 检查 `endpoint`、网络、TLS；必要时临时设置 `ignore_unsafe_cert=true` |
| token 轮换不生效 | unit 中不要写 `--token`；环境变量优先于 JSON |
| Windows 服务无法启动 | `nssm status komari-agent`；查看 `<安装目录>\komari-agent.log` |
| 看不到 token | 这是预期行为：日志、安装摘要和 unit 都会脱敏或只引用配置文件 |
| 需要更多日志 | 设置环境变量 `RUST_LOG=debug` 后重启服务 |
| 安装脚本无法识别 init | 使用 systemd/OpenRC/procd/launchd/upstart/FreeBSD rc.d；NixOS 需手工声明 |

`--dry-run` 可在不修改系统的前提下确认下载 URL、安装目录与服务路径：

```bash
sudo ./install.sh --dry-run --endpoint https://panel.example.com --token TOKEN
```

## 构建与发布

### 本地构建

```bash
cargo build --release
# 产物：target/release/komari-agent
```

版本在编译期注入：

```bash
KOMARI_AGENT_VERSION=1.2.3 \
KOMARI_AGENT_VERSION_HASH=$(git rev-parse --short HEAD) \
cargo build --release
```

```console
$ komari-agent --version
komari-agent 1.2.3 (a1b2c3d)
```

### 交叉编译矩阵

```bash
./build_all.sh          # Unix
.\build_all.ps1         # Windows PowerShell
```

产物写入 `build/`，命名与 Go 版、自更新选择逻辑一致：

```
komari-agent-linux-amd64     komari-agent-windows-amd64.exe
komari-agent-linux-arm64     komari-agent-windows-arm64.exe
komari-agent-linux-386       komari-agent-windows-386.exe
komari-agent-linux-arm       komari-agent-darwin-amd64
komari-agent-darwin-arm64    komari-agent-freebsd-amd64
komari-agent-freebsd-arm64   komari-agent-freebsd-386
komari-agent-freebsd-arm
```

| 变量 | 作用 |
| --- | --- |
| `KOMARI_AGENT_VERSION` | 版本号，默认最新 git tag，否则 `dev` |
| `KOMARI_AGENT_VERSION_HASH` | 版本哈希，默认短 git SHA |
| `KOMARI_BUILD_TARGETS` | 逗号分隔过滤，例如 `linux-amd64,darwin-arm64` |
| `KOMARI_BUILD_TOOL` | `auto`（默认）、`cargo`、`zigbuild` 或 `cross` |
| `KOMARI_INSTALL_TARGETS` | `1`（默认）时自动执行 `rustup target add` |
| `KOMARI_BUILD_DRY_RUN` | `1` 时只打印构建命令 |

前置条件：

- 每个目标需要对应的 rustup target；
- 交叉链接需要平台工具链；安装 `cargo-zigbuild` + `ziglang` 后 `auto` 会自动使用
  `cargo zigbuild`；
- macOS 目标需要 macOS 主机或 osxcross；
- Linux 静态产物使用 musl target；`agent-cli` 依赖的 `aws-lc-sys` 需要 C 工具链，
  使用 zigbuild 时会自动使用 zig cc。

单个目标失败不会中断其余目标，脚本最后以非零退出码汇总失败列表。

### CI 与 Release

- [`.github/workflows/build.yml`](.github/workflows/build.yml)：main push / PR 上运行
  `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test --workspace`，随后构建
  13 个矩阵目标并上传 artifact。
- [`.github/workflows/release.yml`](.github/workflows/release.yml)：release published 时
  重复测试与矩阵构建，以 `komari-agent-{os}-{arch}[.exe]` 上传到 Release，并构建推送
  GHCR 多架构镜像。
- Linux/FreeBSD 目标在 ubuntu runner 上使用 `cargo zigbuild`；Windows/macOS 目标使用
  原生 runner。

Docker 构建可注入版本：

```bash
docker build \
  --build-arg KOMARI_AGENT_VERSION=1.2.3 \
  --build-arg KOMARI_AGENT_VERSION_HASH=a1b2c3d \
  -t komari-agent .
```

## 开发

### 环境要求

- Rust 1.95 或更高（`edition = "2024"`，`resolver = "3"`）

### 目录结构

```
Cargo.toml                 # workspace（edition 2024 / resolver 3）+ 统一依赖版本
crates/
  agent-cli/               # 唯一可执行 crate，二进制名 komari-agent
  agent-config/            # 配置结构、默认值、环境变量、JSON 文件与 CLI 覆盖
  agent-monitor/           # 系统采集（CPU/内存/磁盘/网络/月流量/连接/进程/GPU/OS/虚拟化）
  agent-protocol/          # 上报协议 v1（仅 v1，v2 已按确认裁剪）
  agent-client/            # v1 WebSocket 上报、基础信息上传与重连循环
  agent-discovery/         # 自定义 DNS 解析与自动发现注册
  agent-update/            # GitHub Release 版本检查、下载校验与原子替换
```

后续任务通常只修改各自 crate，不需要再改根 `Cargo.toml`：常用依赖已统一预声明在
`[workspace.dependencies]`，个别缺失依赖仍可在该 crate 的 `Cargo.toml` 中直接添加。

### 构建与测试

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

### 系统采集

`agent-monitor` 已实现 CPU、内存、Swap、负载、磁盘、网络、连接数、进程数、GPU、OS、
uptime 与虚拟化的采集：

- 所有 `/proc` 读取支持 `HOST_PROC` 注入；`HOST_SYS` 可注入 sysfs topology/DMI；
  未设置 `HOST_PROC` 时的本机回退与 `strict_proc` 行为见 crate README。
- 周期上报调用 `Collector::collect_report_metrics()`；A05 在调用前用
  `sample_cpu(1s)` 固定 CPU 窗口并覆盖报告中的 CPU 值。基础信息使用
  `collect_basic_info()`；`collect()` 仅用于诊断。
- 磁盘、网络、月流量、连接/进程数与 GPU 采集已接入 `collect_report_metrics()` 与
  `collect_basic_info()`（`disk_total`、`gpu_name`）。A05 只保留 IP-only 采集（D-019）。

详见 [`crates/agent-monitor/README.md`](crates/agent-monitor/README.md)。

### 自动发现与自动更新

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

### v1 运行链路

`agent-client` 实现了 Go `server/websocket.go` 与 `server/basicInfo.go` 的 v1 路径：

- 连接 `{endpoint}/api/clients/report?token=...`，自动完成 `http→ws` / `https→wss`、
  IDN、尾斜杠、IPv6 与 `prefer_ip_version` 处理；
- 每 `interval` 秒上报一帧，CPU 固定 1 秒采样窗口（`sample_cpu`），字段与
  `ReportV1` 严格一致；30 秒心跳 Ping，读循环只 drain 服务端帧；
- 断开后按 `max_retries`、`reconnect_interval` 重连，耗尽后返回并由 CLI 退出；
- 启动时先上传一次 `BasicInfoV1`（即使 WS 不可达也会上传），之后每
  `info_report_interval` 分钟由独立任务上传；失败时按 Go 语义去掉
  `kernel_version` / `cpu_physical_cores` 再试一次；
- `auto_discovery_key` 存在时读取/写入 `auto-discovery.json`；启动时先同步检查一次更新
  （`Updated` 映射为退出码 42），随后由 `agent-update::run_periodic` 每 6 小时检查。

WebSocket、时钟、HTTP 上传与指标源均为 trait，测试使用本地 mock server 或通道实现，
不会访问真实面板。架构与测试说明见
[`crates/agent-client/README.md`](crates/agent-client/README.md)。
