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
