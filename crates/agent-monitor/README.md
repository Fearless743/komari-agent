# agent-monitor

Komari Agent 的基础系统指标采集库。当前范围仅包含 **CPU、内存、Swap、负载、OS、
uptime 与虚拟化**；磁盘、网络、GPU 由 A03 实现，IP/进程数与上报协议由后续任务实现。

## 快速开始

```rust
use agent_monitor::{Collector, MonitorOptions};

let collector = Collector::new(MonitorOptions::default());
let snapshot = collector.collect();

println!("cpu: {:.2}%", snapshot.cpu.usage);
println!("ram: {} / {}", snapshot.memory.used, snapshot.memory.total);
println!("uptime: {}s", snapshot.uptime);
```

`Collector` 会缓存上一次 `/proc/stat` 样本：首次 `collect_cpu()` 会阻塞
`cpu_sample_interval`（默认 1 秒，与 Go 一致），后续调用直接使用上次样本计算差分。

## 容器 / host_proc

所有 `/proc` 读取（`stat`、`meminfo`、`loadavg`、`uptime`、`cpuinfo`、
`self/cgroup`、`sys/kernel/osrelease`）都通过 `MonitorOptions::proc_root`。
配置层解析 `HOST_PROC` 后传入：

```rust
let options = MonitorOptions::from_config(&config.host_proc, config.memory_include_cache, config.memory_report_raw_used);
```

本 crate **不会**自行读取进程环境变量。独立使用时可使用注入式查找：

```rust
let options = MonitorOptions::from_env_lookup(&|key| std::env::var(key).ok());
```

## 与 Go 的字段对照

### 周期上报（`common.Report`）

| Rust 字段 | 序列化键 | Go 来源 |
|---|---|---|
| `SystemSnapshot::cpu.usage` | `cpu.usage` | `monitoring.Cpu().CPUUsage` |
| `SystemSnapshot::memory.total/used` | `ram.total/used` | `monitoring.Ram()` |
| `SystemSnapshot::swap.total/used` | `swap.total/used` | `monitoring.Swap()` |
| `SystemSnapshot::load.load1/5/15` | `load.load1/5/15` | `monitoring.Load()` |
| `SystemSnapshot::uptime` | `uptime` | `monitoring.Uptime()` |

### 基础信息（`server/basicInfo.go`）

`BasicInfo` 的字段与 Go 上传键一致：`cpu_name`、`cpu_cores`、`cpu_physical_cores`、
`arch`、`os`、`kernel_version`、`mem_total`、`swap_total`、`virtualization`。

`CpuInfo` 的序列化键与 Go `monitoring.CpuInfo` 一致（`cpu_name`、
`cpu_architecture`、`cpu_cores`、`cpu_physical_cores`、`cpu_usage`），额外提供可选的
`per_core_usage`。`cpu_physical_cores = 0` 表示未知，与 Go 的兼容语义一致。

## 内存口径

| 条件 | `MemoryMode` | used 计算 |
|---|---|---|
| `memory_include_cache = true` | `includeCache` | `total - MemFree` |
| `memory_report_raw_used = true` | `htoplike` | `total - (free + cached + sreclaimable + buffers) + shmem` |
| Linux 默认 | `htoplike` | 同上 |
| 非 Linux 默认 | `gopsutil` | `total - available` |

`memory_include_cache` 优先级高于 `memory_report_raw_used`，与 Go 一致。Swap used 为
`total - free - swapCached`，并带有与 Go 相同的下溢保护。

## 平台支持

- **Linux**：完整实现，读取注入的 proc root。
- **macOS / FreeBSD / Windows**：通过 `sysinfo` 提供 CPU、内存、Swap、负载（Windows
  无负载平均值，返回 0）、uptime、OS 名称与内核版本；虚拟化探测在 macOS 使用
  `sysctl kern.hv_vmm_present`，FreeBSD 使用 `sysctl kern.vm_guest`，Windows 暂返回空。

## 已知限制

- 非 Linux 的 OS 名称来自 `sysinfo`，尚未像 Go 一样读取 Windows 注册表或
  `sw_vers`/`uname -sr`；Windows 虚拟化探测未实现。
- Linux 物理核心数依赖 `/proc/cpuinfo` 的 `physical id` + `core id`；ARM 等不提供
  这两个字段的平台返回 `0`（未知），与 Go 的默认语义一致。
- 特殊发行版识别（Android、Proxmox VE、Synology、fnOS）尚未移植，Linux 发行版名称
  统一取 `os-release` 的 `PRETTY_NAME`。
- 虚拟化回退到 DMI 时，KVM 客户机通常同时暴露 QEMU 字符串；已优先匹配 KVM，
  但极少数环境可能得到 `qemu` 而非 `kvm`。正常系统会先命中 `systemd-detect-virt`。
