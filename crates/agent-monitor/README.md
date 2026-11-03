# agent-monitor

Komari Agent 的基础系统指标采集库。当前范围仅包含 **CPU、内存、Swap、负载、OS、
uptime 与虚拟化**；磁盘、网络、GPU 由 A03 实现，IP/进程数与上报协议由后续任务实现。

## 快速开始

```rust
use agent_monitor::{Collector, MonitorOptions};

let collector = Collector::new(MonitorOptions::default());

// 周期上报：不探测 OS / 虚拟化，可每秒调用。
let report = collector.collect_report_metrics();
println!("cpu: {:.2}%", report.cpu.usage);

// 基础信息：Go 每 5 分钟调用一次，内部会探测发行版与虚拟化。
let basic = collector.collect_basic_info();
println!("os: {} kernel: {}", basic.os, basic.kernel_version);
```

`Collector::collect()` 返回完整的 `SystemSnapshot`，包含 OS/虚拟化，适合诊断与测试，
**不应放进高频上报循环**。

## 采集 API 与线格式边界

A02 只提供数据类型；真正的协议编码由 A04/A05 负责。

| 方法 | 返回类型 | 是否探测 OS/虚拟化 | 建议调用频率 |
|---|---|---|---|
| `collect_report_metrics()` | `ReportMetrics` | 否 | 每个上报周期 |
| `collect_basic_info()` | `BasicInfo` | 是 | 与 Go 一致，约 5 分钟 |
| `collect()` | `SystemSnapshot` | 是 | 诊断/测试 |

`ReportMetrics` 与 Go `monitoring.GenerateReport()` 的子集同形：

```json
{
  "cpu": {"usage": 0.001},
  "ram": {"total": 0, "used": 0},
  "swap": {"total": 0, "used": 0},
  "load": {"load1": 0.0, "load5": 0.0, "load15": 0.0},
  "uptime": 0
}
```

其中 CPU 使用率套用 Go 的下限：`usage <= 0.001` 时上报 `0.001`。

`SystemSnapshot` **不是线格式**：它的 CPU 字段沿用 Go `monitoring.CpuInfo` 的键
（`cpu_name`、`cpu_usage` 等），`ram` 额外提供 `available`/`free`，并混有
`os`/`virtualization`。A04/A05 做协议映射时应使用 `ReportMetrics` / `BasicInfo`，
不要直接把 `SystemSnapshot` 序列化后当作 report 发送。

### CPU 采样窗口

- `collect_cpu()`：首次调用阻塞 `cpu_sample_interval`（默认 1 秒，与 Go 一致），
  之后使用上一次样本，采样窗口 = **两次调用之间的实际间隔**，不再阻塞。
- `sample_cpu(window)`：忽略缓存，强制读取两次样本并等待 `window`，供 A05 需要固定
  上报窗口时使用。

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

本 crate **不会**自行读取进程环境变量。独立使用时可使用注入式查找（同时识别
`HOST_PROC` 与 `HOST_SYS`）：

```rust
let options = MonitorOptions::from_env_lookup(&|key| std::env::var(key).ok());
```

测试还可直接注入文件系统根：

```rust
let options = MonitorOptions::default()
    .with_proc_root("/host/proc")
    .with_sys_root("/host/sys")
    .with_root("/host/root");
```

### strict_proc

`from_config` 与 `from_env_lookup` 在 `HOST_PROC` 非空时会自动启用 `strict_proc`。
此时 proc 读取失败返回空值/未知值（CPU 为 `Unknown`、1 核），而不是悄悄改用容器
自身的 `sysinfo` 数据。失败时首次会通过 `tracing::warn!` 输出一条告警。

未配置 `HOST_PROC` 时默认 `strict_proc = false`，proc 不可读会回退本机 `sysinfo`。
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
| `ReportMetrics.uptime` | `uptime` | `monitoring.Uptime()` |

### 基础信息（`server/basicInfo.go`）

`BasicInfo` 与 Go 上传键一致：`cpu_name`、`cpu_cores`、`cpu_physical_cores`、`arch`、
`os`、`kernel_version`、`mem_total`、`swap_total`、`virtualization`。

`CpuInfo` 的序列化键与 Go `monitoring.CpuInfo` 一致（`cpu_name`、
`cpu_architecture`、`cpu_cores`、`cpu_physical_cores`、`cpu_usage`），额外提供可选的
`per_core_usage`。`cpu_physical_cores = 0` 表示未知。

## 物理核心数

顺序与 gopsutil 对齐：

1. `<sys_root>/devices/system/cpu/cpuN/topology/core_cpus_list`；
2. `thread_siblings_list`（旧内核）；
3. `/proc/cpuinfo` 的 `physical id` + `core id` 去重；
4. `/proc/cpuinfo` 的 `physical id` + `cpu cores` 求和；
5. 仅按 `processor` 条目去重（ARM64/RISC-V/LoongArch 常见）。

CPU 型号优先级同样对齐 gopsutil：`model name`/`Model Name`/`cpu`/`uarch`，然后
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
  无负载平均值，返回 0）、uptime、OS 名称与内核版本；虚拟化探测在 macOS 使用
  `sysctl kern.hv_vmm_present`，FreeBSD 使用 `sysctl kern.vm_guest`，Windows 暂返回空。

## 已知限制

- 非 Linux 的 OS 名称来自 `sysinfo`，尚未像 Go 一样读取 Windows 注册表或
  `sw_vers`/`uname -sr`；Windows 虚拟化探测未实现。
- 虚拟化回退到 DMI 时，KVM 客户机通常同时暴露 QEMU 字符串；已优先匹配 KVM，
  但极少数环境可能得到 `qemu` 而非 `kvm`。正常系统会先命中 `systemd-detect-virt`。
- 非 strict 模式下注入的 `proc_root` 不可读会退回本机 `sysinfo`；容器部署通过
  `from_config` 自动启用 strict 以避免误报。
- CPU `collect_cpu()` 的采样窗口是两次调用间隔；需要固定窗口时使用 `sample_cpu(window)`。
