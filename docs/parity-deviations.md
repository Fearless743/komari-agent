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

## D-004 仅实现协议 v1，`protocol_version` 默认 1

- **Go**：`protocol_version` 默认 `2`，请求 v2 时会走 `/api/clients/v2/rpc`、JSON-RPC 与 gzip。
- **Rust**：只实现 v1。`protocol_version` 默认 `1`；显式配置 `2` 或更大值会被降级为 `1`，
  并通过 `tracing::warn!` 输出提示。

原因：用户确认当前 Go 后端没有 `/api/clients/v2/rpc` 路由，v2 在本项目是无效路径；保留默认 2
会让后续任务误以为需要实现 v2。

影响：从 Go 版迁移且显式写死 `protocol_version: 2` 的配置仍能启动，但实际使用 v1；
日志中会出现降级提示。v1 上报与基础信息接口不受影响。

状态：**用户确认的有意裁剪**，A05 及后续任务不得恢复 v2，除非后端先提供路由并重新确认需求。

## D-005 自动发现 endpoint 的尾斜杠处理更宽松

- **Rust**：构造注册 URL 时去掉 endpoint 路径末尾的全部 `/`，再拼接
  `/api/clients/register`。
- **Go**：只去掉一个尾随 `/`，然后再拼接。

影响：endpoint 以 `//` 结尾时，Rust 生成的 URL 不会出现重复斜杠；正常单个 `/` 时两者一致。

## D-006 更新流程不直接退出进程

- **Rust**：`agent-update` 在替换成功后返回 `UpdateOutcome::Updated`，由调用方决定是否以
  `RESTART_EXIT_CODE`（42）退出并交给服务管理器重启。
- **Go**：`CheckAndUpdate` 成功后直接 `os.Exit(42)`。

原因：库 crate 不应终止宿主进程，且测试过程绝不能退出。CLI 接线时需要显式使用 42。

## D-007 更新校验比 Go 更严格

- **Rust**：除大小/可选 SHA-256 外，还拒绝空下载和明显的 HTML/XML/JSON 错误页。
- **Go**：`go-github-selfupdate` 主要依赖下载结果和文件替换，未额外做内容类型嗅探。

影响：若发布资产本身以 `<` 或 `{` 开头（正常 ELF/Mach-O/PE 不会），会被拒绝。大小和
digest 校验失败时不会替换现有二进制，避免半安装状态。


## D-008 Rust 不会降级，Go 使用 Equals 判断

- **Go**：`CheckAndUpdate` 调用 `selfupdate.UpdateSelf` 后只用
  `latest.Version.Equals(currentSemVer)` 判断是否已是最新；如果远端“最新”版本低于当前版本，
  Go 仍可能下载并安装该版本。
- **Rust**：`need_update` 使用 `Version::cmp_precedence` 判断 `latest > current`，同版本或更低
  版本都返回 `UpdateOutcome::UpToDate`，不会降级。

原因：防止误配的 tag、回滚发布或 API 返回旧数据导致在线降级。

影响：需要主动回滚时不能依赖自动更新，必须手工安装目标版本。该差异为有意收紧。

## D-009 更新资产选择改为规范名白名单

- **Rust**：只接受精确的 `komari-agent-{os}-{arch}`（Unix）或
  `komari-agent-{os}-{arch}.exe`（Windows）。`.deb`、`.rpm`、`.msi`、`.dmg`、`.pkg`、
  `.apk`、`.AppImage`、`.sh`、`.run`、`.zst`、`.tar.gz`、`.zip` 等一律不选中。
- **Go**：`go-github-selfupdate` 按 `{os}[-_]{arch}` 后缀匹配，接受
  `.zip`/`.tar.gz`/`.tgz`/`.gzip`/`.gz`/`.tar.xz`/`.xz` 等压缩包，并会解压后替换；
  Windows 也接受不带 `.exe` 的后缀。

原因：A06 审核要求避免把安装包或压缩包误当作可执行文件替换；Komari 官方 Release
workflow 只上传裸二进制 `komari-agent-{os}-{arch}[.exe]`，因此正常发布路径不受影响。

影响：使用自建镜像发布压缩包或非规范文件名的部署无法自动更新，需要改为上传裸二进制
或手工更新。状态：有意的安全收紧。

## D-010 CPU 物理核心数的安全兜底不同

- **Go**：`cpu.Counts(false)` 只使用 sysfs topology，以及 `/proc/cpuinfo` 的
  `physical id` + `cpu cores`。两者都缺失时返回 `0`。
- **Rust**：在相同两步之后，还会依次尝试：
  1. `/proc/cpuinfo` 的 `physical id` + `core id` 去重；
  2. 仅按 `processor` 条目去重（ARM64/RISC-V/LoongArch 等常见）。

原因：避免 ARM/精简容器把物理核心数固定上报为 `0`，让基础信息更有参考价值。

影响：在缺少 topology 且 cpuinfo 无 `physical id` + `cpu cores` 的环境中，Go 上报
`cpu_physical_cores = 0`，Rust 上报去重后的核心数（通常等于逻辑核数）。依赖
`0 = 未知` 语义的消费方需要区分该差异。

状态：有意差异，待对等验收确认。

## D-011 `PreferIp::Auto` 在非 Unix 平台使用近似接口状态

- **Unix（Linux/macOS/FreeBSD 等）**：直接用 `getifaddrs` 检查 `IFF_UP`，跳过
  `IFF_LOOPBACK` 和回环地址，与 Go `preferIPv4First` 的 `net.FlagUp` 语义一致；
  不要求 `IFF_RUNNING`（网线未插但接口 UP 且配置 IPv4 时仍会优先 IPv4）。
- **非 Unix（如 Windows）**：使用 `if-addrs::Interface::is_oper_up()`，该值来自平台
  操作状态，不等同于 POSIX `IFF_UP`，只是近似。

原因：`if-addrs` 不暴露 POSIX `IFF_UP`，Unix 路径改用 libc `getifaddrs` 以精确对齐。
影响：Windows 上极端网络状态下 Auto 的 IPv4/IPv6 选择可能与 Go 不同；显式设置
`prefer_ip_version = "4"` 或 `"6"` 可完全避免该差异。

## D-012 自定义 DNS 失败后额外回退到系统解析器

- **Rust**：自定义 DNS → 内置/配置的 fallback 服务器列表 → 系统解析器（默认开启，可配置关闭）。
- **Go**：自定义 DNS → 内置 `DNSServers` 列表；自定义 DNS 为空时才使用系统解析器。

原因：A06 任务要求“支持系统默认解析作为回退”，提高容器 / 受限网络下的可用性。

影响：自定义 DNS 与所有内置服务器都失败时，Rust 仍可能通过 `/etc/hosts` 或系统 DNS
解析成功；Go 会直接失败。需要严格对齐时设置 `ResolverConfig::system_fallback = false`。

状态：任务要求的增强，不影响正常路径。

## D-013 月流量采样点跟随上报周期

- **Go**：`netstatic.StartOrContinue()` 启动后台 ticker，每 2 秒采样一次；增量先进入
  内存 `staticCache`，每 `save_interval`（默认 600 秒）聚合成一条记录并落盘。
- **Rust**：没有常驻线程；在每次 `collect_network()` / `collect_report_metrics()` 时
  采样。增量先进入内存 cache，只有达到配置的 `save_interval`（默认 600 秒）才聚合为
  一条记录并原子写盘；`Collector` 销毁时 flush 并保存。

原因：本 crate 是库，不应在构造 `Collector` 时隐式启动后台线程；上报循环本身就是最
自然的采样点。磁盘写入与记录数现在与上报频率解耦，不会再按 1 秒/条放大。

影响：没有上报时不会持续采样，停止上报前的最后一个 `save_interval` 窗口依赖
`Collector` 的 `Drop` 落盘；进程被强制杀死时最多丢失一个窗口的内存增量。需要秒级
分辨率时可缩短 `save_interval` 或后续增加独立采样任务。

状态：有意差异，待对等验收确认。

## D-014 Linux 连接数失败时不回退 gopsutil 的 /proc 枚举

- **Go**：优先直接读 `/proc/net/{tcp,tcp6,udp,udp6}`；失败时回退
  `gopsutil net.Connections`。在 Linux 上该回退改用 per-pid fd/inode 枚举去重，但
  `statsFromInodesWithContext` 仍读取 `<HostProc>/net/{tcp,tcp6,udp,udp6}`
  （v4.26.x `net_linux.go`），**不是 netlink**，也不覆盖 `/proc/net` 完全不可读的
  情况，并且需要读取 `/proc/<pid>/fd` 的权限。
- **Rust**：只实现 `/proc/net` 快路径；失败时上报 0 并写入 `message`。
- **非 Linux**：Go 的 `net.Connections` 有各平台实现；Rust 当前恒为 0（D-017）。

原因：避免引入 inode/文件描述符级枚举与各平台 socket API 的大量代码；任务要求覆盖
`/proc` fixture 与 `HOST_PROC`。

影响：Go 的回退同样依赖 `/proc/net`，因此不能覆盖 `/proc/net` 完全不可读的场景；
它额外处理 per-pid fd/inode 去重并需要 fd 权限。Rust 快路径失败时直接上报 0；
macOS/FreeBSD/Windows 同样为 0（D-017）。

状态：已知限制，待后续补齐。

## D-015 空 GPU 详细结果省略 `gpu`

- **Go**：详细探测成功但设备数为 0 时仍输出 `count:0`，并计算 `0/0 = NaN`，随后
  `json.Marshal` 失败导致整包为空。
- **Rust**：设备数为 0 时尝试 `models` 回退，仍为空则省略 `gpu`。

原因：避免向协议层输出 NaN 或空报告。

影响：无 GPU 的 `--enable-gpu` 部署不再出现 Go 的空包/NaN 路径；其它字段正常上报。

状态：有意修复。

## D-016 网络速率对计数器回绕钳零

- **Go**：`NetworkSpeed` 用 `uint64` 直接相减，计数器回绕、网卡消失或驱动重置时会得到
  接近 `2^64` 的巨大 `up`/`down`。
- **Rust**：`saturating_sub` 将回绕/重置段视为 0。

原因：任务明确要求重启/计数器回绕保护；巨大速率会污染面板图表与告警。

影响：回绕瞬间 Go 可能上报异常峰值，Rust 上报 0；累计 `totalUp`/`totalDown` 仍取
当前累计计数器，不受影响。该差异在 `net::tests::counter_wrap_and_interface_removal_clamp_to_zero`
中固定。

状态：有意修复。

## D-017 非 Linux 平台采集为可编译降级

- **Go**：Windows 通过 DXGI 枚举 GPU 名称；macOS/FreeBSD/Windows 均实现
  `net.Connections`；进程数在 macOS/FreeBSD 用 `ps`，Windows 用
  `K32EnumProcesses`。
- **Rust**：Windows `gpu_name` 返回 `Unknown`；非 Linux 连接数恒为 0；非 Linux 进程数
  使用 `sysinfo`；详细 GPU 探测仅 Linux。

原因：本任务要求非 Linux 提供可编译降级，不要求平台原生对等；这些路径不影响 Linux
主目标。

影响：Windows 基础信息缺少真实 GPU 型号；非 Linux 面板连接数为 0；进程数口径与
`ps` 行数/EnumProcesses 可能略有差异。后续平台任务可替换为 DXGI、平台 socket API
与原生进程枚举。

状态：已知限制，待平台任务补齐。

## D-018 重试耗尽后 CLI 以非零码退出

- **Go**：`EstablishWebSocketConnection` 在 v1 下耗尽 `max_retries` 后返回，root 命令的
  `for { server.UpdateBasicInfo(); server.EstablishWebSocketConnection() }` 立即再次进入连接循环。
- **Rust**：`AgentClient::run` 返回 `RunOutcome::RetriesExhausted`，CLI 以退出码 1 结束。

原因：任务要求 v1 下“返回/退出”，同时让启动失败可被服务管理器感知。退出码 0 会让
systemd `Restart=on-failure` 不重启，因此改为 1。
影响：`Restart=always` 与 `Restart=on-failure` 均可恢复；若部署依赖进程内部无限重连，
请把 `max_retries` 设为较大的值。服务管理器可配置 `Restart=on-failure` 或 `always`。

## D-019 指标采集已收敛到 A03，A05 仅保留 IP-only 模块

- **Go**：`monitoring` 统一提供 CPU/内存/磁盘/网络/连接/进程/GPU，IP 由
  `monitoring.GetIPAddress` 采集。
- **Rust**：A03 的 `agent_monitor::Collector` 已提供
  `collect_report_metrics()`（含 disk/network/connections/process/GPU/message）和
  `collect_basic_info()`（含 `disk_total`/`gpu_name`）。A05 已删除自己重复的
  磁盘/网络/GPU/连接/进程采集，`MonitorReportProvider` 直接调用 A03：
  - CPU 仍用 `sample_cpu(1s)` 固定窗口，并覆盖报告中的 CPU 值；
  - `month_rotate` 通过 `MonitorOptions::from_monitor_config` 传入 netstatic；
  - `ReportSample::from_metrics` 显式逐字段构造，A03 新增字段不会被静默丢弃。
  A03 不提供 IP，因此 `agent-client::ip` 仅保留 IP 采集；其外部服务行为见 D-020。

状态：已收敛。`agent-client::supplement` 已删除，不再有第二套指标采集实现。

## D-020 外部 IP 查询基本对齐（NIC > custom > 外部服务）

状态：**基本对齐**，剩余差异见本节末尾。

- **Go**：`GetIPAddress` 的优先级是：启用 `get_ip_addr_from_nic` 且取到地址时直接返回；
  否则使用 `custom_ipv4`/`custom_ipv6`；对应地址族仍为空时请求 visa.cn、ipify 等外部服务。
- **Rust**：已按同样顺序实现，外部查询只在 NIC 与 custom 都没有提供该地址族时执行：
  - Unix：用 `getifaddrs` + `IFF_UP`（不含 `IFF_RUNNING`）判断接口可用，排除回环接口、
    回环地址和 IPv6 链路本地地址，与 Go `net.FlagUp` 语义一致；
  - 非 Unix：`if-addrs::is_oper_up()` 只是近似（另见 D-011）。
- 与 Go 对齐的部分：
  - IPv4 顺序：visa.cn、qualcomm.cn、toutiao、edge-ip、vercel-ip、ipv4.ip.sb、ipify；
  - IPv6 顺序：v6.ip.zxinc.org、api6.ipify.org、ipv6.icanhazip.com、api-ipv6.ip.sb；
  - `User-Agent: curl/8.0.1`，单次请求 15 秒超时；
  - IPv4/IPv6 客户端分别锁定 `tcp4`/`tcp6`：自定义 DNS 结果按地址族过滤，并用本地
    地址族绑定兜底 IP 字面量；
  - 代理变量与 Go 的 `http.ProxyFromEnvironment` 一致：只读
    `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`（大小写均可），不读 `ALL_PROXY`；
    代理变量只读取一次（Go 的 `ProxyFromEnvironment` 也缓存首次结果），行为等价；
  - 支持 `socks5://` 与 `socks5h://`（scheme 大小写不敏感）；Go 对两者都使用远端
    DNS，Rust 将 `socks5://` 规范化为 reqwest 的 `socks5h://` 以对齐该行为；
    Go 不支持 `socks4`/`socks4a`；
  - IPv4 正则使用 ASCII `[0-9]{1,3}`，等价于 Go RE2 的 `\d`（Go 的 `\d` 不匹配全角数字）；
    IPv6 正则表达式与 Go 逐字符一致；
  - 传输错误、超时或无匹配时继续下一个，全部失败返回空字符串；
  - 与 Go 一样不检查 HTTP 状态码，非 2xx 的响应体仍会参与正则提取。
- 实现方式：基础信息在 `spawn_blocking` 中采集，因此外部查询使用阻塞式 `reqwest`；
  DNS 走运行时的 `DnsResolver`（含 `custom_dns`），日志只记录端点与错误，不记录 token、
  响应体或代理凭据。

已登记差异：

- reqwest 额外接受 `socks4`/`socks4a`；Go 的 `ProxyFromEnvironment` 不支持这两种协议。
- `NO_PROXY` 的匹配细节由 reqwest/hyper-util 提供，未逐条复刻 Go `httpproxy` 的全部边界
  （常见域名、IP、端口与通配场景已覆盖）。
- 外部查询使用阻塞式 `reqwest`（在 `spawn_blocking` 内），不是 Go 的 `net/http` transport；
  IPv6 不可用等网络环境差异可能导致相同请求在不同平台表现不同。

影响：未启用 NIC 且未设置 custom 的部署会像 Go 一样访问公网 IP 服务；容器/内网部署仍可
通过 `get_ip_addr_from_nic` 或 `custom_ipv4`/`custom_ipv6` 完全避免外部请求。

## D-021 WebSocket 不走 HTTP(S)_PROXY，也不启用 permessage-deflate

- **Go**：gorilla/websocket 使用 `http.ProxyFromEnvironment`，并默认启用
  `EnableCompression`（除非 `disable_compression`）。
- **Rust**：`tokio-tungstenite` 直连目标地址；v1 帧是普通 JSON，不启用 permessage-deflate。
  `--disable-compression` 仅为配置兼容而保留，实际始终不压缩。

原因：v1 上报体很小，压缩收益有限；代理支持需要额外解析代理协议与 TLS 隧道，暂不在 A05 范围。
影响：必须经 HTTP 代理访问面板的部署目前无法直接使用 Rust agent；需要直连或使用系统级
透明代理。`--disable-compression` 不会改变行为。

## D-022 月度流量统计已收敛到 A03

- **Go**：`month_rotate != 0` 时启动 `netstatic`，网络 `totalUp`/`totalDown` 按月度重置点统计。
- **Rust**：A03 已实现 netstatic；A05 通过
  `MonitorOptions::from_monitor_config(..., month_rotate, ...)` 把配置传入
  `agent_monitor::Collector`，由 `collect_report_metrics()` 产出月度累计值。
  采样点与持久化语义见 A03 的 D-013。

状态：已收敛，不再是 A05 的差异；保留编号用于历史对照。

## D-023 uptime 读取失败不写入上报 message

- **Go**：`monitoring.GenerateReport` 在 `monitoring.Uptime()` 返回错误时把
  `failed to get uptime: ...` 追加到 `message`。
- **Rust**：A03 的 `collect_report_metrics()` 只把 network/connections/GPU 错误写入
  `message`；uptime 读取失败会记录日志并按平台回退（`strict_proc` 下直接为 0），
  不追加到 `message`。

原因：uptime 采集已收敛到 A03，A05 不再维护第二套 message 组装逻辑；A03 当前接口
只返回 `u64`，没有携带 uptime 错误。
影响：依赖 `message` 中 uptime 诊断的部署会少一行文本，其余上报字段不受影响；
`HOST_PROC`（strict_proc）下 uptime 读取失败静默归零，不混入宿主机指标。

## D-024 不提供 FreeBSD/arm 构建产物

- **Go**：`build_all` 与 release 矩阵包含 `freebsd/arm`，产物为
  `komari-agent-freebsd-arm`。
- **Rust**：构建矩阵不包含 `freebsd/arm`。`armv7-unknown-freebsd` 是 tier 3
  target，rustup 不提供预编译 std，`cargo build --target armv7-unknown-freebsd`
  在标准工具链上无法直接完成。

原因：不为单个 tier 3 目标引入 nightly `-Z build-std` 或自建 sysroot，避免发布
流程依赖不稳定的工具链。

影响：FreeBSD/arm 用户目前需要使用 Go 版，或在 FreeBSD/arm 主机上用
`-Z build-std` 自行构建。`install.sh` 在 FreeBSD/arm 上会明确报错，不会下载
不存在的资产；其余 FreeBSD 目标（amd64/arm64/386）不受影响。
