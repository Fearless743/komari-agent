# agent-monitor

Komari Agent 的系统指标采集库：**CPU、内存、Swap、负载、磁盘、网络、连接数、
进程数、GPU、OS、uptime 与虚拟化**。IP 地址与上报协议由后续任务实现。

## 快速开始

```rust
use agent_monitor::{Collector, MonitorOptions};

let collector = Collector::new(MonitorOptions::default());

// 周期上报：不探测 OS / 虚拟化，可每秒调用。
let report = collector.collect_report_metrics();
println!("cpu: {:.2}%", report.cpu.usage);
println!("disk: {} / {}", report.disk.used, report.disk.total);
println!("net: {}/s", report.network.total_down);

// 基础信息：Go 每 5 分钟调用一次，内部会探测发行版与虚拟化。
let basic = collector.collect_basic_info();
println!("os: {} kernel: {}", basic.os, basic.kernel_version);
```

`Collector::collect()` 返回完整的 `SystemSnapshot`，包含 OS/虚拟化，适合诊断与测试，
**不应放进高频上报循环**。

## 采集 API 与线格式边界

本 crate 只提供数据类型与采集；真正的协议编码由 A04/A05 负责。

| 方法 | 返回类型 | 是否探测 OS/虚拟化 | 建议调用频率 |
|---|---|---|---|
| `collect_report_metrics()` | `ReportMetrics` | 否 | 每个上报周期 |
| `collect_basic_info()` | `BasicInfo` | 是 | 与 Go 一致，约 5 分钟 |
| `collect()` | `SystemSnapshot` | 是 | 诊断/测试 |

`ReportMetrics` 与 Go `monitoring.GenerateReport()` 同形：

```json
{
  "cpu": {"usage": 0.001},
  "ram": {"total": 0, "used": 0},
  "swap": {"total": 0, "used": 0},
  "load": {"load1": 0.0, "load5": 0.0, "load15": 0.0},
  "disk": {"total": 0, "used": 0},
  "network": {"up": 0, "down": 0, "totalUp": 0, "totalDown": 0},
  "connections": {"tcp": 0, "udp": 0},
  "uptime": 0,
  "process": 0,
  "message": ""
}
```

`gpu` 仅在 `enable_gpu = true` 且探测到 GPU 时出现。CPU 使用率套用 Go 的下限：
`usage <= 0.001` 时上报 `0.001`。网络/连接/GPU 失败不会影响其它字段，只写入
`message`（与 Go 的错误文本前缀一致）。

`SystemSnapshot` **不是线格式**：它的 CPU 字段沿用 Go `monitoring.CpuInfo` 的键
（`cpu_name`、`cpu_usage` 等），`ram` 额外提供 `available`/`free`，并混有
`os`/`virtualization`。A04/A05 做协议映射时应使用 `ReportMetrics` / `BasicInfo`，
不要直接把 `SystemSnapshot` 序列化后当作 report 发送。

### CPU 采样窗口

- `collect_cpu()`：Linux 首次调用阻塞 `cpu_sample_interval`（默认 1 秒，与 Go 一致），
  之后使用上一次样本，采样窗口 = **两次调用之间的实际间隔**，不再阻塞。
- **非 Linux**：走 `sysinfo`，每次 `collect_cpu()` 都会阻塞一次采样窗口（至少
  `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`，默认约 1 秒）。
- `sample_cpu(window)`：绕过缓存，强制读取两次样本并等待 `window`，供 A05 需要固定
  上报窗口时使用；采样结果会**写回缓存**，之后的 `collect_cpu()` 以该样本为起点。

Go 的 `Cpu()` 每次调用固定采样 1 秒；两者在持续上报时结果接近，但窗口语义不同。

## 容器 / proc、sys 与 root 注入

所有 `/proc` 读取（`stat`、`meminfo`、`loadavg`、`uptime`、`cpuinfo`、
`self/cgroup`、`sys/kernel/osrelease`）都通过 `MonitorOptions::proc_root`；
CPU topology 与 DMI 通过 `sys_root`；发行版标记通过 `root`。

配置层解析 `HOST_PROC` 后传入：

```rust
let options = MonitorOptions::from_config(
    &config.host_proc,
    config.memory_include_cache,
    config.memory_report_raw_used,
);
```

本 crate 不会自行解析配置环境变量。唯一例外是 **默认 root（`/`）** 下 `pveversion`
的 `PATH` 查找：未注入 `process_path` 时会读取进程 `PATH`。使用注入 root 时不会读取
宿主 `PATH`，只接受 `<root>/usr/bin/pveversion` 或显式注入的 `process_path`。可通过
`with_process_path()` 或 `from_env_lookup()`（读取 `PATH`）显式注入，避免依赖全局环境。

独立使用时可使用注入式查找（识别 `HOST_PROC`、`HOST_SYS` 与 `PATH`）：

```rust
let options = MonitorOptions::from_env_lookup(&|key| std::env::var(key).ok());
```

测试还可直接注入文件系统根：

```rust
let options = MonitorOptions::default()
    .with_proc_root("/host/proc")   // 自动 strict_proc = true
    .with_sys_root("/host/sys")
    .with_root("/host/root");
```

`with_proc_root()` 与 `HOST_PROC` 一样会自动启用 `strict_proc`；若确实希望回退本机
`sysinfo`，在其后调用 `.with_strict_proc(false)`。

### A05 接线：HOST_SYS 与 PATH

`agent-config` 目前只有 `host_proc` 字段。A05 接线时如果需要宿主机 sysfs（物理核心
topology / DMI），必须自行读取 `HOST_SYS` 并调用 `with_sys_root()`；如需避免默认
root 下 `pveversion` 的全局 `PATH` 回退，可读取 `PATH` 并调用 `with_process_path()`。
本 crate 除该默认-root `PATH` 回退外不直接读取全局环境变量。大多数容器中默认 `/sys`
已反映宿主机拓扑，未设置 `HOST_SYS` 时可保持默认。

### strict_proc 的边界

`strict_proc` 只约束 **proc** 读取：`stat`、`meminfo`、`loadavg`、`uptime`、`cpuinfo`
等失败时返回空值/未知值。`sys_root`（`HOST_SYS`）与 `root`（发行版标记）是独立注入
项，不受 strict 影响，物理核心数/DMI 仍会从 `sys_root` 读取。因此 strict + 仅注入
`proc` 时，CPU 名称可能是 `Unknown`，但物理核心数仍可能来自默认 `/sys` 或显式
`sys_root`。

### strict_proc

`from_config`、`from_env_lookup`（`HOST_PROC` 非空）与 `with_proc_root()` 都会自动
启用 `strict_proc`。此时 proc 读取失败返回空值/未知值（CPU 为 `Unknown`、1 核），
而不是悄悄改用容器自身的 `sysinfo` 数据。失败时首次会通过 `tracing::warn!` 输出一条告警。

未配置 `HOST_PROC` 时默认 `strict_proc = false`，CPU/内存/Swap/负载/uptime 等 proc
读取失败会回退本机 `sysinfo`（磁盘挂载表与连接数除外，见下文）。
可用 `with_strict_proc(true/false)` 显式覆盖。

与 Go 的差异：Go 的内存/Swap 采集直接硬编码 `/proc`（不经过 `HOST_PROC` 参数），
本实现统一走注入的 proc root；Go 在 CPU 读取失败时返回 `Unknown/0/1 核`，本实现的
strict 模式行为与之对齐，非 strict 模式则提供本机回退。

## 与 Go 的字段对照

### 周期上报（`common.Report` 子集）

| Rust | 序列化键 | Go 来源 |
|---|---|---|
| `ReportMetrics.cpu.usage` | `cpu.usage` | `monitoring.Cpu().CPUUsage`（含 0.001 下限） |
| `ReportMetrics.ram.total/used` | `ram.total/used` | `monitoring.Ram()` |
| `ReportMetrics.swap.total/used` | `swap.total/used` | `monitoring.Swap()` |
| `ReportMetrics.load.load1/5/15` | `load.load1/5/15` | `monitoring.Load()` |
| `ReportMetrics.disk.total/used` | `disk.total/used` | `monitoring.Disk()` |
| `ReportMetrics.network.up/down` | `network.up/down` | `NetworkSpeed()` 两次采样的差值 |
| `ReportMetrics.network.total_up/down` | `network.totalUp/totalDown` | 累计字节；`month_rotate` 时为重置后的累计 |
| `ReportMetrics.connections.tcp/udp` | `connections.tcp/udp` | `ConnectionsCount()` |
| `ReportMetrics.process` | `process` | `ProcessCount()`（Go `int` -> `i64`） |
| `ReportMetrics.gpu` | `gpu` | `GetDetailedGPUInfo()` 的详细或 `models` 回退形状 |
| `ReportMetrics.message` | `message` | 采集错误累积文本 |
| `ReportMetrics.uptime` | `uptime` | `monitoring.Uptime()` |

### 基础信息（`server/basicInfo.go`）

`BasicInfo` 与 Go 上传键一致：`cpu_name`、`cpu_cores`、`cpu_physical_cores`、`arch`、
`os`、`kernel_version`、`mem_total`、`swap_total`、`disk_total`、`gpu_name`、
`virtualization`。

`CpuInfo` 的序列化键与 Go `monitoring.CpuInfo` 一致（`cpu_name`、
`cpu_architecture`、`cpu_cores`、`cpu_physical_cores`、`cpu_usage`），额外提供可选的
`per_core_usage`。`cpu_physical_cores = 0` 表示未知。

## 磁盘

对应 `monitoring/unit/disk.go`：

- `include_mountpoints` 使用分号分隔；设置后只统计列出的挂载点，读取失败的点直接
  跳过，不经过物理盘过滤（与 Go 一致）。
- 未设置时按 gopsutil 顺序读取 `<proc_root>/1/mountinfo`，回退 `<proc_root>/1/mounts`、
  `<proc_root>/self/mountinfo`、`<proc_root>/self/mounts`；挂载表不可读时整体返回 0
  （include 列表也不例外，Linux 与非 Linux 一致），按 Go 的
  `isPhysicalDisk` 排除 `/tmp`、`/dev`、`/run`、`/proc`、`/sys`、容器目录、
  `tmpfs`/`overlay`/`fuse` 等伪文件系统、网络盘与 `/dev/loop*`；`/` 永远保留。
- 同一设备只统计一次，保留 `total` 更大的挂载点；ZFS dataset 归并到 pool。
- `total`/`used` 来自 `statvfs`，`used = total - f_bfree * f_frsize`（与 gopsutil 一致）。
- 非 Linux 使用 `sysinfo`；Linux 挂载表不可读返回 0（include 也不例外）。

## 网络与月流量

对应 `monitoring/unit/net.go` 与 `monitoring/netstatic/static.go`：

- `include_nics` / `exclude_nics` 逗号分隔，使用 Go `filepath.Match` 风格通配符
  （`*`、`?`、`[...]`）；`br`、`docker`、`veth`、`vmbr`、`lo` 等前缀始终排除。
- 白名单优先于黑名单；白名单为空时默认包含所有非虚拟接口。
- `up`/`down` 是两次采样（默认间隔 1 秒）之间的字节差；计数器回绕/重置按 0 处理
  （Go 会下溢成巨大值，见 D-016）。
- 无法读取任何网卡时返回 0 并写入 `message`。
- `month_rotate != 0` 时，累计量来自 `<net_static_path>`（默认 `./net_static.json`）
  中 **重置日之后** 的逐接口增量；重置日按本地时区计算，月份不存在该日时顺延到
  下月 1 日；`month_rotate` 不在 1..=31 时使用当前时刻（当天流量不清零）。
- 每次上报的增量先进入内存 cache，达到 `save_interval`（默认 600 秒）才聚合成一条
  记录并原子写盘；`Collector` 销毁时 flush 并保存。记录前先应用网卡过滤，loopback/
  docker* 等不会写入数据库。
- Linux 从 `<proc_root>/net/dev` 读取累计字节/包；其它平台使用 `sysinfo`。

与 Go 的差异：Go 用后台 goroutine 每 2 秒采样一次；本 crate 在每次上报时采样，
增量缓存到 `save_interval` 再落盘，因此写盘次数/记录数与上报频率解耦。没有上报时
不会持续采样，停止上报前最后一个窗口依赖 `Collector` 的 `Drop` 保存（D-013）。

## 连接数与进程数

对应 `monitoring/unit/net.go`（`ConnectionsCount`）、`process_linux.go` 与非 Linux
`process_*.go`：

- TCP = `net/tcp` + `net/tcp6` 数据行，UDP = `net/udp` + `net/udp6`；缺失的单个文件
  跳过，整族都缺失则视为失败并上报 0。
- 进程数为 `<proc_root>` 下名称可解析为整数的条目数，`HOST_PROC` 生效。
- 非 Linux 进程数使用 `sysinfo`（Go 为 `ps` 行数或 `K32EnumProcesses`）；连接数目前
  只实现 Linux，非 Linux 恒为 0（D-014、D-017）。

## GPU

对应 `monitoring/unit/gpu*.go`：

- `gpu_name`（基础信息）Linux 先 `lspci`，再 `/sys/class/drm`；macOS 使用
  `system_profiler`，FreeBSD 使用 `pciconf`，Windows 返回 `Unknown`（Go 用 DXGI，
  见 D-017）。
- `enable_gpu` 打开详细探测：优先定位并执行 `nvidia-smi -q -x`，XML 解析成功即使用
  NVIDIA；否则尝试 `rocm-smi --showallinfo --json`（AMD）。Intel 目前仅参与型号
  识别，不提供详细显存/利用率。
- 详细形状：`{count, average_usage, detailed_info:[{name,memory_total,memory_used,
  utilization,temperature}]}`；详细探测失败但型号可用时输出 `{"models":[...]}`。
- 工具缺失、无 GPU、XML/JSON 损坏都只导致 `gpu` 缺失，其它指标不受影响。
- 测试可通过 `with_gpu_tools()` 注入 `nvidia-smi`/`rocm-smi` 路径与 `PATH`，不依赖
  真实 GPU。

## 物理核心数

顺序如下：

1. `<sys_root>/devices/system/cpu/cpuN/topology/core_cpus_list`；
2. `thread_siblings_list`（旧内核）；
3. `/proc/cpuinfo` 的 `physical id` + `cpu cores` 求和；
4. `/proc/cpuinfo` 的 `physical id` + `core id` 去重；
5. 仅按 `processor` 条目去重（ARM64/RISC-V/LoongArch 常见）。

第 1-3 步与 gopsutil 一致。第 4、5 步是 Rust 的安全兜底：gopsutil 在缺少 topology
且 cpuinfo 没有 `physical id` + `cpu cores` 时返回 `0`，本实现则分别返回去重后的
核心数/逻辑核数。该差异已登记为 [`docs/parity-deviations.md`](../../docs/parity-deviations.md)
的 D-010。

CPU 型号优先级对齐 gopsutil：`model name`/`Model Name`/`cpu`/`uarch`，然后
`Processor`、`Hardware`、`Model`。

## 内存口径

| 条件 | `MemoryMode` | used 计算 |
|---|---|---|
| `memory_include_cache = true` | `includeCache` | `total - MemFree` |
| `memory_report_raw_used = true` | `htoplike` | `total - (free + cached + sreclaimable + buffers) + shmem` |
| Linux 默认 | `htoplike` | 同上 |
| 非 Linux 默认 | `gopsutil` | `total - available` |

`memory_include_cache` 优先级高于 `memory_report_raw_used`，与 Go 一致。Swap used 为
`total - free - swapCached`，并带有与 Go 相同的下溢保护。

## 发行版识别

Linux 按 Go 的顺序检测：Android（`getprop`/`build.prop`/目录）、Proxmox VE
（`pveversion`）、Synology DSM（`synoinfo.conf`）、fnOS（`/usr/trim`），最后读取
`os-release` 的 `PRETTY_NAME`，失败回退 `Linux`。所有路径均可通过 `root` 注入。

## 平台支持

- **Linux**：完整实现，读取注入的 proc/sys/root。
- **macOS / FreeBSD / Windows**：通过 `sysinfo` 提供 CPU、内存、Swap、负载（Windows
  无负载平均值，返回 0）、磁盘、网络、uptime、OS 名称与内核版本；连接数返回 0，
  详细 GPU 探测不可用（`gpu_name` 在 macOS/FreeBSD 走系统命令，Windows 返回
  `Unknown`）；虚拟化探测在 macOS 使用 `sysctl kern.hv_vmm_present`，FreeBSD 使用
  `sysctl kern.vm_guest`，Windows 暂返回空。

## 已知限制

- 非 Linux 的 OS 名称来自 `sysinfo`，尚未像 Go 一样读取 Windows 注册表或
  `sw_vers`/`uname -sr`；Windows 虚拟化探测未实现。
- 虚拟化回退到 DMI 时，KVM 客户机通常同时暴露 QEMU 字符串；已优先匹配 KVM，
  但极少数环境可能得到 `qemu` 而非 `kvm`。正常系统会先命中 `systemd-detect-virt`。
- 非 strict 模式下 CPU/内存等 proc 读取失败会退回本机 `sysinfo`；磁盘挂载表不可读
  时返回 0（与 Go `disk.Partitions` 一致），不会回退枚举本机磁盘。容器部署通过
  `from_config` 自动启用 strict 以避免误报。
- CPU `collect_cpu()` 的采样窗口是两次调用间隔；需要固定窗口时使用 `sample_cpu(window)`。
- 月流量数据库没有后台采样线程，采样点等于上报点；进程被强制杀死时最多丢失一个
  `save_interval` 窗口的内存增量（D-013）。
- Go 的 Linux 连接数回退会做 per-pid fd/inode 去重，但仍依赖 `/proc/net`；Rust 不
  做该回退，快路径失败直接上报 0；非 Linux 连接数恒为 0（D-014、D-017）。
- 详细 GPU 探测为空（例如驱动存在但无设备）时省略 `gpu`，不会产生 Go 的
  `count:0` + `NaN` 平均使用率（D-015）。
- Windows GPU 型号未知；非 Linux 进程数使用 `sysinfo`，与 `ps`/EnumProcesses 口径
  可能略有差异（D-017）。
- 磁盘挂载表与网络计数器从注入的 `proc_root` 读取；挂载表来源与 gopsutil 一致
  （优先 `/proc/1/mountinfo`），但读取根可被 `HOST_PROC` 重定向，容器部署更可控。
