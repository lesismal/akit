//! A node's GPUs across vendors (fit's `llm/internal/gpu`): which vendors' GPUs are there (the
//! PCI bus on Linux, the SoC on Apple silicon), each vendor's tool (how to install it, query it
//! and change settings with it), the GPUs' detailed status, and settings such as power limits,
//! clock locks, fan speed and persistence mode.
//!
//! Commands are data (the catalog below). A setting's value is checked (integers in bounds, or
//! one of its options) before it is put into its command, inside one argument, so it can neither
//! inject shell syntax nor add arguments; settings run as argv, never through a shell. Install
//! scripts are constants of this program, never a caller's. Collecting runs the vendors' tools:
//! it blocks, on the node's pool.

use std::sync::Arc;
use std::time::Duration;

use crate::actions::goos;
use crate::exec::{self, is_root, look_path};
use crate::json::{self, Obj};

/// One integer value.
pub const KIND_NUMBER: &str = "number";
/// `min,max` integers.
pub const KIND_RANGE: &str = "range";
/// One of the options.
pub const KIND_ENUM: &str = "enum";
/// No value.
pub const KIND_ACTION: &str = "action";

/// The variable the install scripts read the node's data directory from (decision 038).
pub const ENV_DATA_DIR: &str = "AKIT_LLM_GPU_DATA_DIR";

/// One setting of a vendor's GPUs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// Its key.
    pub key: &'static str,
    /// Its label.
    pub label: &'static str,
    /// Its label in Chinese.
    pub label_zh: &'static str,
    /// [`KIND_NUMBER`], [`KIND_RANGE`], [`KIND_ENUM`] or [`KIND_ACTION`].
    pub kind: &'static str,
    /// Its unit.
    pub unit: &'static str,
    /// An enum's options.
    pub options: &'static [&'static str],
    /// A note.
    pub note: &'static str,
    /// The note in Chinese.
    pub note_zh: &'static str,
    /// The command's template: split on spaces into argv, then `{index}`, `{value}`, `{min}`,
    /// `{max}` and `{display}` replaced inside each argument.
    pub command: &'static str,
    /// For the pages: `power`, `clock`, `fan` or `mode`.
    pub group: &'static str,
}

/// How a vendor's tool is installed on one family of systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallMethod {
    /// `debian`, `rhel`, `darwin` or `any`.
    pub os: &'static str,
    /// Its title.
    pub title: &'static str,
    /// A constant script, run with `sh -c`.
    pub script: &'static str,
}

/// A vendor's GPUs and their tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vendor {
    /// Its ID.
    pub id: &'static str,
    /// Its name.
    pub name: &'static str,
    /// Its PCI vendor IDs (lower-case hex, no `0x`).
    pub pci_vendors: &'static [&'static str],
    /// Its tool.
    pub tool: &'static str,
    /// Its other tools.
    pub extra_tools: &'static [&'static str],
    /// The query, as shown.
    pub query_cmd: &'static str,
    /// Its docs.
    pub docs: &'static str,
    /// How its tool is installed.
    pub install: &'static [InstallMethod],
    /// Its settings.
    pub settings: &'static [Setting],
    /// A description.
    pub description: &'static str,
    /// The description in Chinese.
    pub description_zh: &'static str,
}

const fn setting(
    key: &'static str,
    label: &'static str,
    label_zh: &'static str,
    kind: &'static str,
    group: &'static str,
    command: &'static str,
) -> Setting {
    Setting {
        key,
        label,
        label_zh,
        kind,
        unit: "",
        options: &[],
        note: "",
        note_zh: "",
        command,
        group,
    }
}

const S0: &str = r##"set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update
if command -v ubuntu-drivers >/dev/null 2>&1 || apt-get install -y ubuntu-drivers-common; then
  # Install the recommended server driver (includes nvidia-smi / NVML)
  ubuntu-drivers install --gpgpu || ubuntu-drivers autoinstall
else
  # Debian: non-free driver from the distribution
  apt-get install -y nvidia-driver firmware-misc-nonfree
fi
apt-get install -y nvidia-settings || true
command -v nvidia-smi && nvidia-smi || echo "Driver installed: reboot the node to load the kernel module."
"##;
const S1: &str = r##"set -e
. /etc/os-release
MAJOR="${VERSION_ID%%.*}"
ARCH="$(uname -m)"
dnf install -y dnf-plugins-core kernel-devel-"$(uname -r)" kernel-headers || true
dnf config-manager --add-repo "https://developer.download.nvidia.com/compute/cuda/repos/rhel${MAJOR}/${ARCH}/cuda-rhel${MAJOR}.repo"
dnf module install -y nvidia-driver:latest-dkms || dnf install -y nvidia-driver nvidia-driver-cuda
command -v nvidia-smi && nvidia-smi || echo "Driver installed: reboot the node to load the kernel module."
"##;
const S2: &str = r##"set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update
if apt-get install -y rocm-smi; then
  rocm-smi || true
  exit 0
fi
# Fall back to AMD's installer (Ubuntu 22.04 / 24.04)
. /etc/os-release
VER=6.2.4; PKG=6.2.60204-1
curl -fsSLO "https://repo.radeon.com/amdgpu-install/${VER}/ubuntu/${VERSION_CODENAME}/amdgpu-install_${PKG}_all.deb"
apt-get install -y "./amdgpu-install_${PKG}_all.deb"
amdgpu-install -y --usecase=rocm --no-dkms
rocm-smi || echo "Installed: reboot the node if the amdgpu driver was updated."
"##;
const S3: &str = r##"set -e
. /etc/os-release
MAJOR="${VERSION_ID%%.*}"
dnf install -y "https://repo.radeon.com/amdgpu-install/6.2.4/rhel/${VERSION_ID}/amdgpu-install-6.2.60204-1.el${MAJOR}.noarch.rpm"
amdgpu-install -y --usecase=rocm --no-dkms
rocm-smi || echo "Installed: reboot the node if the amdgpu driver was updated."
"##;
const S4: &str = r##"set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y gpg curl intel-gpu-tools || true
. /etc/os-release
curl -fsSL https://repositories.intel.com/gpu/intel-graphics.key | gpg --yes --dearmor -o /usr/share/keyrings/intel-graphics.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/intel-graphics.gpg] https://repositories.intel.com/gpu/ubuntu ${VERSION_CODENAME} unified" > /etc/apt/sources.list.d/intel-gpu.list
apt-get update
apt-get install -y xpu-smi
xpu-smi discovery
"##;
const S5: &str = r##"set -e
. /etc/os-release
dnf install -y 'dnf-command(config-manager)' || true
dnf config-manager --add-repo "https://repositories.intel.com/gpu/rhel/${VERSION_ID}/unified/intel-gpu-${VERSION_ID}.repo"
dnf install -y xpu-smi intel-gpu-tools || dnf install -y xpu-smi
xpu-smi discovery
"##;
const S6: &str = r##"set -e
# Put the driver / firmware .run packages for this card in $AKIT_LLM_GPU_DATA_DIR/ascend first
# (download them from https://www.hiascend.com/hardware/firmware-drivers/community).
DIR="${AKIT_LLM_GPU_DATA_DIR:-.}/ascend"
ls "$DIR"/*.run >/dev/null 2>&1 || { echo "No .run packages in $DIR"; exit 1; }
groupadd -f HwHiAiUser; id HwHiAiUser >/dev/null 2>&1 || useradd -g HwHiAiUser -d /home/HwHiAiUser -m HwHiAiUser -s /bin/bash
for f in "$DIR"/*npu-driver*.run "$DIR"/*npu-firmware*.run; do
  [ -f "$f" ] || continue
  chmod +x "$f"; "$f" --full --install-for-all --quiet
done
npu-smi info || echo "Installed: reboot the node to load the driver."
"##;
const S7: &str = r##"set -e
# Put the musa driver .deb for this card in $AKIT_LLM_GPU_DATA_DIR/mthreads first
DIR="${AKIT_LLM_GPU_DATA_DIR:-.}/mthreads"
ls "$DIR"/*.deb >/dev/null 2>&1 || { echo "No .deb packages in $DIR"; exit 1; }
export DEBIAN_FRONTEND=noninteractive
apt-get install -y "$DIR"/*.deb
mthreads-gmi || echo "Installed: reboot the node to load the driver."
"##;
const S8: &str = r##"set -e
# Put the DCU driver .run package in $AKIT_LLM_GPU_DATA_DIR/hygon first
DIR="${AKIT_LLM_GPU_DATA_DIR:-.}/hygon"
ls "$DIR"/*.run >/dev/null 2>&1 || { echo "No .run packages in $DIR"; exit 1; }
for f in "$DIR"/*.run; do chmod +x "$f"; "$f" -y || "$f"; done
hy-smi || echo "Installed: reboot the node to load the driver."
"##;
const S9: &str = r##"set -e
system_profiler SPDisplaysDataType
if command -v pip3 >/dev/null 2>&1; then pip3 install --user asitop && echo "asitop installed: run sudo asitop"; fi
"##;

/// The vendors known, as fit lists them.
pub const VENDORS: &[Vendor] = &[
    Vendor {
        id: "nvidia",
        name: "NVIDIA",
        pci_vendors: &["10de"],
        tool: "nvidia-smi",
        extra_tools: &["nvidia-settings"],
        query_cmd: "nvidia-smi --query-gpu=index,name,uuid,driver_version,memory.total,memory.used,utilization.gpu,temperature.gpu,power.draw,power.limit,power.min_limit,power.max_limit,power.default_limit,clocks.gr,clocks.mem,clocks.max.gr,clocks.max.mem,fan.speed,pstate,persistence_mode,compute_mode --format=csv,noheader,nounits",
        docs: "https://docs.nvidia.com/deploy/nvidia-smi/index.html",
        description: "nvidia-smi ships with the NVIDIA driver (NVML). Clock offsets and manual fan control need nvidia-settings, a running X server and Coolbits enabled.",
        description_zh: "nvidia-smi 随 NVIDIA 驱动（NVML）一起安装。频率偏移（超频）与手动风扇控制需要 nvidia-settings、运行中的 X Server 并开启 Coolbits。",
        install: &[
            InstallMethod {
                os: "debian",
                title: "Ubuntu / Debian: driver + nvidia-smi",
                script: S0,
            },
            InstallMethod {
                os: "rhel",
                title: "RHEL / Rocky / Alma: CUDA repo driver",
                script: S1,
            },
        ],
        settings: &[
            Setting {
                options: &["1", "0"],
                note: "1 = on: keeps the driver loaded, reduces start-up latency",
                note_zh: "1 = 开启：保持驱动常驻，降低任务启动延迟",
                ..setting(
                    "persistence",
                    "Persistence mode",
                    "持久模式",
                    KIND_ENUM,
                    "mode",
                    "nvidia-smi -i {index} -pm {value}",
                )
            },
            Setting {
                unit: "W",
                note: "Must be within the min / max power limit of the board",
                note_zh: "须在显卡的最小 / 最大功耗范围内",
                ..setting(
                    "power_limit",
                    "Power limit",
                    "功耗上限",
                    KIND_NUMBER,
                    "power",
                    "nvidia-smi -i {index} -pl {value}",
                )
            },
            Setting {
                unit: "MHz",
                note: "min,max in MHz (Volta and newer)",
                note_zh: "min,max，单位 MHz（Volta 及更新架构）",
                ..setting(
                    "gpu_clock_lock",
                    "Lock GPU clocks",
                    "锁定核心频率",
                    KIND_RANGE,
                    "clock",
                    "nvidia-smi -i {index} -lgc {min},{max}",
                )
            },
            setting(
                "gpu_clock_reset",
                "Reset GPU clocks",
                "重置核心频率",
                KIND_ACTION,
                "clock",
                "nvidia-smi -i {index} -rgc",
            ),
            Setting {
                unit: "MHz",
                note: "min,max in MHz (Ampere and newer)",
                note_zh: "min,max，单位 MHz（Ampere 及更新架构）",
                ..setting(
                    "mem_clock_lock",
                    "Lock memory clocks",
                    "锁定显存频率",
                    KIND_RANGE,
                    "clock",
                    "nvidia-smi -i {index} -lmc {min},{max}",
                )
            },
            setting(
                "mem_clock_reset",
                "Reset memory clocks",
                "重置显存频率",
                KIND_ACTION,
                "clock",
                "nvidia-smi -i {index} -rmc",
            ),
            Setting {
                unit: "MHz",
                note: "Needs nvidia-settings, an X server and Coolbits (nvidia-xconfig --cool-bits=28)",
                note_zh: "需要 nvidia-settings、X Server 与 Coolbits（nvidia-xconfig --cool-bits=28）",
                ..setting(
                    "gpu_clock_offset",
                    "GPU clock offset (overclock)",
                    "核心频率偏移（超频）",
                    KIND_NUMBER,
                    "clock",
                    "nvidia-settings -c {display} -a [gpu:{index}]/GPUGraphicsClockOffsetAllPerformanceLevels={value}",
                )
            },
            Setting {
                unit: "MHz",
                note: "Transfer rate offset (2× the memory clock); needs X + Coolbits",
                note_zh: "传输速率偏移（显存频率的 2 倍）；需要 X + Coolbits",
                ..setting(
                    "mem_clock_offset",
                    "Memory clock offset (overclock)",
                    "显存频率偏移（超频）",
                    KIND_NUMBER,
                    "clock",
                    "nvidia-settings -c {display} -a [gpu:{index}]/GPUMemoryTransferRateOffsetAllPerformanceLevels={value}",
                )
            },
            Setting {
                unit: "%",
                note: "Manual fan control; needs X + Coolbits",
                note_zh: "手动风扇控制；需要 X + Coolbits",
                ..setting(
                    "fan_speed",
                    "Fan speed",
                    "风扇转速",
                    KIND_NUMBER,
                    "fan",
                    "nvidia-settings -c {display} -a [gpu:{index}]/GPUFanControlState=1 -a [fan:{index}]/GPUTargetFanSpeed={value}",
                )
            },
            setting(
                "fan_auto",
                "Automatic fan control",
                "风扇自动控制",
                KIND_ACTION,
                "fan",
                "nvidia-settings -c {display} -a [gpu:{index}]/GPUFanControlState=0",
            ),
            Setting {
                options: &["DEFAULT", "EXCLUSIVE_PROCESS", "PROHIBITED"],
                ..setting(
                    "compute_mode",
                    "Compute mode",
                    "计算模式",
                    KIND_ENUM,
                    "mode",
                    "nvidia-smi -i {index} -c {value}",
                )
            },
        ],
    },
    Vendor {
        id: "amd",
        name: "AMD",
        pci_vendors: &["1002"],
        tool: "rocm-smi",
        extra_tools: &["amd-smi"],
        query_cmd: "rocm-smi --showproductname --showdriverversion --showtemp --showpower --showmaxpower --showuse --showmeminfo vram --showclocks --showfan --showperflevel --json",
        docs: "https://rocm.docs.amd.com/projects/rocm_smi_lib/en/latest/",
        description: "rocm-smi (and the newer amd-smi) come with ROCm. Changing clocks needs the perf level set to manual; the power overdrive changes the board power cap.",
        description_zh: "rocm-smi（以及更新的 amd-smi）随 ROCm 安装。修改频率需先把性能等级设为 manual；power overdrive 修改显卡功耗上限。",
        install: &[
            InstallMethod {
                os: "debian",
                title: "Ubuntu / Debian: rocm-smi",
                script: S2,
            },
            InstallMethod {
                os: "rhel",
                title: "RHEL / Rocky / Alma: amdgpu-install",
                script: S3,
            },
        ],
        settings: &[
            Setting {
                unit: "W",
                ..setting(
                    "power_cap",
                    "Power cap (overdrive)",
                    "功耗上限（overdrive）",
                    KIND_NUMBER,
                    "power",
                    "rocm-smi -d {index} --autorespond y --setpoweroverdrive {value}",
                )
            },
            setting(
                "power_reset",
                "Reset power cap",
                "重置功耗上限",
                KIND_ACTION,
                "power",
                "rocm-smi -d {index} --autorespond y --resetpoweroverdrive",
            ),
            Setting {
                options: &["auto", "low", "high", "manual"],
                ..setting(
                    "perf_level",
                    "Performance level",
                    "性能等级",
                    KIND_ENUM,
                    "mode",
                    "rocm-smi -d {index} --setperflevel {value}",
                )
            },
            Setting {
                note: "DPM level index; needs perf level manual",
                note_zh: "DPM 档位序号；需要性能等级为 manual",
                ..setting(
                    "sclk_level",
                    "GPU clock level",
                    "核心频率档位",
                    KIND_NUMBER,
                    "clock",
                    "rocm-smi -d {index} --autorespond y --setsclk {value}",
                )
            },
            Setting {
                note: "DPM level index; needs perf level manual",
                note_zh: "DPM 档位序号；需要性能等级为 manual",
                ..setting(
                    "mclk_level",
                    "Memory clock level",
                    "显存频率档位",
                    KIND_NUMBER,
                    "clock",
                    "rocm-smi -d {index} --autorespond y --setmclk {value}",
                )
            },
            Setting {
                unit: "%",
                note: "Percent above the default top clock (0-20)",
                note_zh: "相对默认最高频率的提升百分比（0-20）",
                ..setting(
                    "overdrive",
                    "GPU clock overdrive",
                    "核心频率超频比例",
                    KIND_NUMBER,
                    "clock",
                    "rocm-smi -d {index} --autorespond y --setoverdrive {value}",
                )
            },
            setting(
                "clock_reset",
                "Reset clocks",
                "重置频率",
                KIND_ACTION,
                "clock",
                "rocm-smi -d {index} --resetclocks",
            ),
            Setting {
                unit: "%",
                ..setting(
                    "fan_speed",
                    "Fan speed",
                    "风扇转速",
                    KIND_NUMBER,
                    "fan",
                    "rocm-smi -d {index} --setfan {value}%",
                )
            },
            setting(
                "fan_auto",
                "Automatic fan control",
                "风扇自动控制",
                KIND_ACTION,
                "fan",
                "rocm-smi -d {index} --resetfans",
            ),
        ],
    },
    Vendor {
        id: "intel",
        name: "Intel",
        pci_vendors: &["8086"],
        tool: "xpu-smi",
        extra_tools: &["intel_gpu_top"],
        query_cmd: "xpu-smi discovery -j; xpu-smi stats -d <id> -j",
        docs: "https://github.com/intel/xpumanager",
        description: "xpu-smi (Intel XPU Manager) manages Data Center GPUs (Flex / Max) and Arc; intel_gpu_top (intel-gpu-tools) only monitors integrated / consumer GPUs.",
        description_zh: "xpu-smi（Intel XPU Manager）管理数据中心 GPU（Flex / Max）与 Arc；intel_gpu_top（intel-gpu-tools）只能监控集成 / 消费级 GPU。",
        install: &[
            InstallMethod {
                os: "debian",
                title: "Ubuntu: Intel GPU repository + xpu-smi",
                script: S4,
            },
            InstallMethod {
                os: "rhel",
                title: "RHEL / Rocky: Intel GPU repository + xpu-smi",
                script: S5,
            },
        ],
        settings: &[
            Setting {
                unit: "W",
                ..setting(
                    "power_limit",
                    "Power limit",
                    "功耗上限",
                    KIND_NUMBER,
                    "power",
                    "xpu-smi config -d {index} --powerlimit {value}",
                )
            },
            Setting {
                unit: "MHz",
                ..setting(
                    "freq_range",
                    "GPU frequency range",
                    "核心频率范围",
                    KIND_RANGE,
                    "clock",
                    "xpu-smi config -d {index} --frequencyrange {min},{max}",
                )
            },
            Setting {
                options: &["0", "1"],
                note: "0 = default, 1 = never standby",
                note_zh: "0 = 默认，1 = 从不待机",
                ..setting(
                    "standby",
                    "Standby mode",
                    "待机模式",
                    KIND_ENUM,
                    "mode",
                    "xpu-smi config -d {index} --standby {value}",
                )
            },
            Setting {
                unit: "µs",
                ..setting(
                    "scheduler_timeout",
                    "Scheduler: timeout mode",
                    "调度器：超时模式",
                    KIND_NUMBER,
                    "mode",
                    "xpu-smi config -d {index} --scheduler timeout,{value}",
                )
            },
        ],
    },
    Vendor {
        id: "ascend",
        name: "Huawei Ascend",
        pci_vendors: &["19e5"],
        tool: "npu-smi",
        extra_tools: &[],
        query_cmd: "npu-smi info",
        docs: "https://www.hiascend.com/document",
        description: "npu-smi is installed with the Ascend NPU driver (Ascend-hdk-*-npu-driver). Download the driver package matching the card from the Ascend site and install it with --full.",
        description_zh: "npu-smi 随昇腾 NPU 驱动（Ascend-hdk-*-npu-driver）安装。从昇腾官网下载与卡型号匹配的驱动包，并以 --full 安装。",
        install: &[InstallMethod {
            os: "any",
            title: "Ascend driver package",
            script: S6,
        }],
        settings: &[Setting {
            options: &["0", "1"],
            note: "Atlas 300I / 800 series; 0 = high performance, 1 = power saving",
            note_zh: "Atlas 300I / 800 系列；0 = 高性能，1 = 节能",
            ..setting(
                "power_mode",
                "Power-saving mode (custom)",
                "节能模式（自定义）",
                KIND_ENUM,
                "power",
                "npu-smi set -t pwr-mode -i {index} -d {value}",
            )
        }],
    },
    Vendor {
        id: "mthreads",
        name: "Moore Threads",
        pci_vendors: &["1ed5"],
        tool: "mthreads-gmi",
        extra_tools: &[],
        query_cmd: "mthreads-gmi -q",
        docs: "https://developer.mthreads.com/",
        description: "mthreads-gmi is installed with the MUSA driver (musa_*.deb / .rpm from the Moore Threads developer site).",
        description_zh: "mthreads-gmi 随 MUSA 驱动安装（从摩尔线程开发者网站获取 musa_*.deb / .rpm）。",
        install: &[InstallMethod {
            os: "debian",
            title: "MUSA driver package",
            script: S7,
        }],
        settings: &[],
    },
    Vendor {
        id: "hygon",
        name: "Hygon DCU",
        pci_vendors: &["1d94"],
        tool: "hy-smi",
        extra_tools: &[],
        query_cmd: "hy-smi",
        docs: "https://developer.hpccube.com/",
        description: "hy-smi is installed with the DTK / DCU driver; its options follow rocm-smi.",
        description_zh: "hy-smi 随 DTK / DCU 驱动安装，参数与 rocm-smi 一致。",
        install: &[InstallMethod {
            os: "any",
            title: "DCU driver package",
            script: S8,
        }],
        settings: &[
            Setting {
                unit: "W",
                ..setting(
                    "power_cap",
                    "Power cap",
                    "功耗上限",
                    KIND_NUMBER,
                    "power",
                    "hy-smi -d {index} --setpoweroverdrive {value}",
                )
            },
            Setting {
                options: &["auto", "low", "high", "manual"],
                ..setting(
                    "perf_level",
                    "Performance level",
                    "性能等级",
                    KIND_ENUM,
                    "mode",
                    "hy-smi -d {index} --setperflevel {value}",
                )
            },
            Setting {
                unit: "%",
                ..setting(
                    "fan_speed",
                    "Fan speed",
                    "风扇转速",
                    KIND_NUMBER,
                    "fan",
                    "hy-smi -d {index} --setfan {value}%",
                )
            },
        ],
    },
    Vendor {
        id: "apple",
        name: "Apple Silicon",
        pci_vendors: &[],
        tool: "system_profiler",
        extra_tools: &["powermetrics"],
        query_cmd: "system_profiler SPDisplaysDataType -json; sudo powermetrics --samplers gpu_power -n 1",
        docs: "https://support.apple.com/guide/mac-help/",
        description: "The GPU is part of the SoC: the tools are built into macOS and there are no power or clock settings. Use low power mode (pmset) to cap power.",
        description_zh: "GPU 集成在 SoC 中：工具为 macOS 内置，没有功耗或频率设置。可以用低功耗模式（pmset）限制功耗。",
        install: &[InstallMethod {
            os: "darwin",
            title: "Built in (optional: asitop monitor)",
            script: S9,
        }],
        settings: &[Setting {
            options: &["0", "1"],
            note: "Applies to the whole machine",
            note_zh: "作用于整机",
            ..setting(
                "low_power",
                "Low power mode",
                "低功耗模式",
                KIND_ENUM,
                "power",
                "pmset -a lowpowermode {value}",
            )
        }],
    },
];

fn strs_json(v: &[&str]) -> String {
    json::array(v.iter().map(|s| json::string(s)))
}

/// A list as Go writes a slice: `null` when it has none.
fn slice_json(v: &[&str]) -> String {
    if v.is_empty() {
        "null".to_owned()
    } else {
        strs_json(v)
    }
}

impl Setting {
    /// As JSON (fit's `gpu.Setting`).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .str("key", self.key)
            .str("label", self.label)
            .str("label_zh", self.label_zh)
            .str("kind", self.kind)
            .str_omit("unit", self.unit);
        if !self.options.is_empty() {
            o = o.raw("options", &strs_json(self.options));
        }
        o.str_omit("note", self.note)
            .str_omit("note_zh", self.note_zh)
            .str("command", self.command)
            .str("group", self.group)
            .end()
    }
}

impl Vendor {
    fn obj(&self) -> Obj {
        let mut o = Obj::new()
            .str("id", self.id)
            .str("name", self.name)
            .raw("pci_vendors", &slice_json(self.pci_vendors))
            .str("tool", self.tool);
        if !self.extra_tools.is_empty() {
            o = o.raw("extra_tools", &strs_json(self.extra_tools));
        }
        let settings = if self.settings.is_empty() {
            "null".to_owned()
        } else {
            json::array(self.settings.iter().map(Setting::to_json))
        };
        o.str("query_cmd", self.query_cmd)
            .str("docs", self.docs)
            .raw(
                "install",
                &json::array(self.install.iter().map(|m| {
                    Obj::new()
                        .str("os", m.os)
                        .str("title", m.title)
                        .str("script", m.script)
                        .end()
                })),
            )
            .raw("settings", &settings)
            .str("description", self.description)
            .str("description_zh", self.description_zh)
    }

    /// As JSON (fit's `gpu.Vendor`).
    #[must_use]
    pub fn to_json(&self) -> String {
        self.obj().end()
    }

    /// Its setting `key`.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<&'static Setting> {
        self.settings.iter().find(|s| s.key == key)
    }
}

/// The catalog as JSON (`/node/gpu/catalog`).
#[must_use]
pub fn catalog_json() -> String {
    json::array(VENDORS.iter().map(Vendor::to_json))
}

/// Vendor `id`.
#[must_use]
pub fn vendor(id: &str) -> Option<&'static Vendor> {
    VENDORS.iter().find(|v| v.id == id)
}

/// A vendor as this node sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorStatus {
    /// The vendor.
    pub vendor: &'static Vendor,
    /// Its devices on the PCI bus.
    pub detected: i64,
    /// Where its tool is.
    pub tool_path: String,
    /// Whether its tool is there.
    pub installed: bool,
    /// Whether each of its other tools is there.
    pub extra_found: Vec<(String, bool)>,
    /// The install method for this system, `""` for none.
    pub install_for: String,
}

impl VendorStatus {
    fn to_json(&self) -> String {
        let mut o = self
            .vendor
            .obj()
            .int("detected", self.detected)
            .str_omit("tool_path", &self.tool_path)
            .bool("installed", self.installed);
        if !self.extra_found.is_empty() {
            let mut f = self.extra_found.clone();
            f.sort();
            let mut m = Obj::new();
            for (k, v) in &f {
                m = m.bool(k, *v);
            }
            o = o.raw("extra_found", &m.end());
        }
        o.str_omit("install_for", &self.install_for).end()
    }
}

/// One GPU's detailed status; an unknown number is -1.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Its vendor.
    pub vendor: String,
    /// Its index.
    pub index: i64,
    /// Its name.
    pub name: String,
    /// Its UUID.
    pub uuid: String,
    /// Its driver's version.
    pub driver: String,
    /// MiB.
    pub mem_total_mb: f64,
    /// MiB.
    pub mem_used_mb: f64,
    /// Percent.
    pub utilization: f64,
    /// Celsius.
    pub temperature: f64,
    /// Watts.
    pub power_draw: f64,
    /// Watts.
    pub power_limit: f64,
    /// Watts.
    pub power_min: f64,
    /// Watts.
    pub power_max: f64,
    /// Watts.
    pub power_default: f64,
    /// MHz.
    pub clock_gfx: f64,
    /// MHz.
    pub clock_mem: f64,
    /// MHz.
    pub clock_gfx_max: f64,
    /// MHz.
    pub clock_mem_max: f64,
    /// Percent.
    pub fan_speed: f64,
    /// Its performance state.
    pub pstate: String,
    /// Its persistence mode.
    pub persistence: String,
    /// Its compute mode.
    pub compute_mode: String,
    /// Its performance level.
    pub perf_level: String,
}

impl Device {
    /// A device of `vendor` at `index`, every number unknown.
    #[must_use]
    pub fn unknown(vendor: &str, index: i64) -> Device {
        Device {
            vendor: vendor.to_owned(),
            index,
            name: String::new(),
            uuid: String::new(),
            driver: String::new(),
            mem_total_mb: -1.0,
            mem_used_mb: -1.0,
            utilization: -1.0,
            temperature: -1.0,
            power_draw: -1.0,
            power_limit: -1.0,
            power_min: -1.0,
            power_max: -1.0,
            power_default: -1.0,
            clock_gfx: -1.0,
            clock_mem: -1.0,
            clock_gfx_max: -1.0,
            clock_mem_max: -1.0,
            fan_speed: -1.0,
            pstate: String::new(),
            persistence: String::new(),
            compute_mode: String::new(),
            perf_level: String::new(),
        }
    }

    /// As JSON (fit's `gpu.Device`).
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("vendor", &self.vendor)
            .int("index", self.index)
            .str("name", &self.name)
            .str_omit("uuid", &self.uuid)
            .str_omit("driver", &self.driver)
            .float("mem_total_mb", self.mem_total_mb)
            .float("mem_used_mb", self.mem_used_mb)
            .float("utilization", self.utilization)
            .float("temperature", self.temperature)
            .float("power_draw", self.power_draw)
            .float("power_limit", self.power_limit)
            .float("power_min", self.power_min)
            .float("power_max", self.power_max)
            .float("power_default", self.power_default)
            .float("clock_gfx", self.clock_gfx)
            .float("clock_mem", self.clock_mem)
            .float("clock_gfx_max", self.clock_gfx_max)
            .float("clock_mem_max", self.clock_mem_max)
            .float("fan_speed", self.fan_speed)
            .str_omit("pstate", &self.pstate)
            .str_omit("persistence", &self.persistence)
            .str_omit("compute_mode", &self.compute_mode)
            .str_omit("perf_level", &self.perf_level)
            .end()
    }
}

/// A node's GPUs (fit's `gpu.Status`).
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// `debian`, `rhel`, `darwin`, `linux` or Go's `GOOS`.
    pub os: String,
    /// The node runs as root.
    pub root: bool,
    /// Commands are prefixed with `sudo -n`.
    pub use_sudo: bool,
    /// Every vendor of the catalog.
    pub vendors: Vec<VendorStatus>,
    /// The GPUs read through the tools installed.
    pub devices: Vec<Device>,
    /// The tools' output, for vendors without a parser.
    pub raw: Vec<(String, String)>,
    /// The tools' errors, per vendor.
    pub errors: Vec<(String, String)>,
    /// When it was read, in seconds since 1970.
    pub collected_at: i64,
}

fn map_json(m: &[(String, String)]) -> String {
    let mut m = m.to_vec();
    m.sort();
    let mut o = Obj::new();
    for (k, v) in &m {
        o = o.str(k, v);
    }
    o.end()
}

impl Status {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("os", &self.os)
            .bool("root", self.root)
            .bool("use_sudo", self.use_sudo)
            .raw(
                "vendors",
                &json::array(self.vendors.iter().map(VendorStatus::to_json)),
            )
            .raw(
                "devices",
                &json::array(self.devices.iter().map(Device::to_json)),
            )
            .raw("raw", &map_json(&self.raw))
            .raw("errors", &map_json(&self.errors))
            .int("collected_at", self.collected_at)
            .end()
    }
}

/// How commands run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Settings and install scripts are prefixed with `sudo -n` when the node is not root.
    pub use_sudo: bool,
}

impl Options {
    fn sudo(self) -> bool {
        self.use_sudo && goos() != "windows" && !is_root()
    }

    /// `argv` prefixed with `sudo -n` (keeping `DISPLAY`, `XAUTHORITY` and
    /// [`ENV_DATA_DIR`]) when set and not root.
    #[must_use]
    pub fn wrap(self, argv: Vec<String>) -> Vec<String> {
        if !self.sudo() {
            return argv;
        }
        let mut out = vec![
            "sudo".to_owned(),
            "-n".to_owned(),
            format!("--preserve-env=DISPLAY,XAUTHORITY,{ENV_DATA_DIR}"),
        ];
        out.extend(argv);
        out
    }
}

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// The family of install methods this system takes, from `/etc/os-release` on Linux.
#[must_use]
pub fn os_family() -> String {
    if goos() != "linux" {
        return goos().to_owned();
    }
    let rel = read_trim("/etc/os-release");
    family_of(&rel)
}

fn family_of(os_release: &str) -> String {
    let mut ids: Vec<String> = Vec::new();
    for line in os_release.lines() {
        if let Some(v) = line.strip_prefix("ID=") {
            ids.push(v.trim_matches('"').to_owned());
        }
        if let Some(v) = line.strip_prefix("ID_LIKE=") {
            ids.extend(v.trim_matches('"').split_whitespace().map(str::to_owned));
        }
    }
    for id in &ids {
        match id.as_str() {
            "debian" | "ubuntu" | "linuxmint" | "pop" | "kylin" | "uos" | "deepin" => {
                return "debian".to_owned();
            }
            "rhel" | "centos" | "fedora" | "rocky" | "almalinux" | "ol" | "anolis"
            | "openEuler" | "openeuler" | "kylinsec" => return "rhel".to_owned(),
            _ => {}
        }
    }
    "linux".to_owned()
}

fn install_for(v: &Vendor, family: &str) -> Option<&'static InstallMethod> {
    let vendor = vendor(v.id)?;
    vendor
        .install
        .iter()
        .find(|m| m.os == family || m.os == "any" && family != "darwin" && family != "windows")
}

/// The display controllers (class 0x03) and processing accelerators (0x12) per vendor on the
/// PCI bus (Linux).
fn pci_vendors() -> Vec<(&'static str, i64)> {
    let mut out: Vec<(&'static str, i64)> = Vec::new();
    if goos() != "linux" {
        return out;
    }
    let Ok(dirs) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return out;
    };
    for d in dirs.flatten() {
        let p = d.path();
        let class = read_trim(&p.join("class").to_string_lossy());
        if !class.starts_with("0x03") && !class.starts_with("0x12") {
            continue;
        }
        let vid = read_trim(&p.join("vendor").to_string_lossy()).to_lowercase();
        let vid = vid.trim_start_matches("0x");
        for v in VENDORS {
            if v.pci_vendors.contains(&vid) {
                match out.iter_mut().find(|(id, _)| *id == v.id) {
                    Some((_, n)) => *n = n.saturating_add(1),
                    None => out.push((v.id, 1)),
                }
            }
        }
    }
    out
}

/// Reads the vendors and every GPU through the tools installed (fit's `gpu.Collect`). Blocks:
/// on a pool.
#[must_use]
pub fn collect(pool: &Arc<akio::Pool>, opts: Options) -> Status {
    let family = os_family();
    let mut st = Status {
        os: family.clone(),
        root: is_root(),
        use_sudo: opts.sudo(),
        vendors: Vec::new(),
        devices: Vec::new(),
        raw: Vec::new(),
        errors: Vec::new(),
        collected_at: akit_roles::linktoken::unix_now(),
    };
    let pci = pci_vendors();
    for v in VENDORS {
        let mut vs = VendorStatus {
            vendor: v,
            detected: pci
                .iter()
                .find(|(id, _)| *id == v.id)
                .map_or(0, |(_, n)| *n),
            tool_path: String::new(),
            installed: false,
            extra_found: Vec::new(),
            install_for: install_for(v, &family).map_or_else(String::new, |m| m.os.to_owned()),
        };
        if v.id == "apple" && goos() == "darwin" && crate::actions::goarch() == "arm64" {
            vs.detected = 1;
        }
        if let Some(p) = look_path(v.tool) {
            vs.tool_path = p.to_string_lossy().into_owned();
            vs.installed = true;
        }
        for t in v.extra_tools {
            vs.extra_found
                .push(((*t).to_owned(), look_path(t).is_some()));
        }
        let installed = vs.installed;
        st.vendors.push(vs);
        if !installed {
            continue;
        }
        let (devs, raw, err) = query(pool, v);
        if let Some(e) = err {
            st.errors.push((v.id.to_owned(), e));
        }
        if !raw.is_empty() {
            st.raw.push((v.id.to_owned(), raw));
        }
        st.devices.extend(devs);
    }
    st
}

const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

fn output(pool: &Arc<akio::Pool>, argv: &[&str]) -> Result<String, (String, String)> {
    exec::output(pool, argv, QUERY_TIMEOUT, true)
}

/// Reads the GPUs of `v`, whose tool is installed: parsed, or its output raw for a vendor
/// without a parser.
fn query(pool: &Arc<akio::Pool>, v: &Vendor) -> (Vec<Device>, String, Option<String>) {
    match v.id {
        "nvidia" => match output(
            pool,
            &[
                "nvidia-smi",
                "--query-gpu=index,name,uuid,driver_version,memory.total,memory.used,utilization.gpu,temperature.gpu,power.draw,power.limit,power.min_limit,power.max_limit,power.default_limit,clocks.gr,clocks.mem,clocks.max.gr,clocks.max.mem,fan.speed,pstate,persistence_mode,compute_mode",
                "--format=csv,noheader,nounits",
            ],
        ) {
            Ok(out) => (parse_nvidia_smi(&out), String::new(), None),
            Err((out, e)) => (Vec::new(), out, Some(e)),
        },
        "amd" => {
            let r = output(
                pool,
                &[
                    "rocm-smi",
                    "--showproductname",
                    "--showdriverversion",
                    "--showtemp",
                    "--showpower",
                    "--showmaxpower",
                    "--showuse",
                    "--showmeminfo",
                    "vram",
                    "--showclocks",
                    "--showfan",
                    "--showperflevel",
                    "--json",
                ],
            );
            let out = match r {
                Ok(o) => o,
                Err((o, e)) if !o.trim().starts_with('{') => return (Vec::new(), o, Some(e)),
                Err((o, _)) => o,
            };
            match parse_rocm_smi(&out) {
                Ok(d) => (d, String::new(), None),
                Err(e) => (Vec::new(), out, Some(e)),
            }
        }
        "intel" => query_intel(pool),
        "apple" => match output(pool, &["system_profiler", "SPDisplaysDataType", "-json"]) {
            Ok(out) => (parse_apple_displays(&out), String::new(), None),
            Err((out, e)) => (Vec::new(), out, Some(e)),
        },
        "hygon" | "ascend" | "mthreads" => {
            let argv: &[&str] = match v.id {
                "hygon" => &["hy-smi"],
                "ascend" => &["npu-smi", "info"],
                _ => &["mthreads-gmi"],
            };
            match output(pool, argv) {
                Ok(out) => (Vec::new(), out, None),
                Err((out, e)) => (Vec::new(), out, Some(e)),
            }
        }
        _ => (Vec::new(), String::new(), None),
    }
}

/// A number of a tool's output; `[N/A]`, `N/A`, `""` and the like are -1.
fn num(s: &str) -> f64 {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .unwrap_or(-1.0)
}

/// The records of CSV `text` (Go's `encoding/csv` with leading spaces trimmed and any number
/// of fields), or none when a quote is unterminated.
fn csv_records(text: &str) -> Option<Vec<Vec<String>>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = Vec::new();
        let mut chars = line.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                chars.next();
            }
            let mut f = String::new();
            if chars.peek() == Some(&'"') {
                chars.next();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            f.push('"');
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        f.push(c);
                    }
                }
                if !closed {
                    return None;
                }
                while chars.peek().is_some_and(|c| *c != ',') {
                    chars.next();
                }
            } else {
                while let Some(c) = chars.peek() {
                    if *c == ',' {
                        break;
                    }
                    f.push(*c);
                    chars.next();
                }
            }
            fields.push(f);
            if chars.next().is_none() {
                break;
            }
        }
        out.push(fields);
    }
    Some(out)
}

/// The CSV of the nvidia-smi query.
#[must_use]
pub fn parse_nvidia_smi(out: &str) -> Vec<Device> {
    let Some(recs) = csv_records(out) else {
        return Vec::new();
    };
    let mut devs = Vec::new();
    for f in recs {
        if f.len() < 21 {
            continue;
        }
        let g = |i: usize| f.get(i).map_or("", String::as_str);
        let Ok(idx) = g(0).trim().parse::<i64>() else {
            continue;
        };
        let mut d = Device::unknown("nvidia", idx);
        d.name = g(1).to_owned();
        d.uuid = g(2).to_owned();
        d.driver = g(3).to_owned();
        d.mem_total_mb = num(g(4));
        d.mem_used_mb = num(g(5));
        d.utilization = num(g(6));
        d.temperature = num(g(7));
        d.power_draw = num(g(8));
        d.power_limit = num(g(9));
        d.power_min = num(g(10));
        d.power_max = num(g(11));
        d.power_default = num(g(12));
        d.clock_gfx = num(g(13));
        d.clock_mem = num(g(14));
        d.clock_gfx_max = num(g(15));
        d.clock_mem_max = num(g(16));
        d.fan_speed = num(g(17));
        d.pstate = g(18).to_owned();
        d.persistence = g(19).to_owned();
        d.compute_mode = g(20).to_owned();
        devs.push(d);
    }
    devs
}

/// `rocm-smi ... --json`, whose keys vary between ROCm releases: values found by part of their
/// key.
///
/// # Errors
///
/// Not the JSON it writes.
pub fn parse_rocm_smi(out: &str) -> Result<Vec<Device>, String> {
    let out = out.find('{').map_or(out, |i| out.get(i..).unwrap_or(out));
    let v = json::parse(out.as_bytes()).map_err(|e| format!("parse rocm-smi json: {e}"))?;
    let akit_config::value::Value::Object(cards) = &v else {
        return Err("parse rocm-smi json: not an object".to_owned());
    };
    let find = |kv: &akit_config::value::Value, sub: &str| -> String {
        if let akit_config::value::Value::Object(ms) = kv {
            for (k, v) in ms {
                if k.to_lowercase().contains(sub) {
                    if let akit_config::value::Value::String(s) = v {
                        return s.trim().to_owned();
                    }
                    return String::new();
                }
            }
        }
        String::new()
    };
    let first = |vals: &[String]| {
        vals.iter()
            .find(|s| !s.is_empty())
            .cloned()
            .unwrap_or_default()
    };
    let driver = v
        .get("system")
        .map(|s| find(s, "driver version"))
        .unwrap_or_default();
    let mut devs = Vec::new();
    for (card, kv) in cards {
        let Some(n) = card.strip_prefix("card") else {
            continue;
        };
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let idx = n.parse::<i64>().unwrap_or(0);
        let mut d = Device::unknown("amd", idx);
        d.name = first(&[
            find(kv, "card series"),
            find(kv, "device name"),
            find(kv, "card model"),
        ]);
        d.driver = first(&[find(kv, "driver version"), driver.clone()]);
        d.temperature = num(&first(&[
            find(kv, "temperature (sensor edge)"),
            find(kv, "temperature (sensor junction)"),
        ]));
        d.power_draw = num(&first(&[
            find(kv, "average graphics package power"),
            find(kv, "current socket graphics package power"),
        ]));
        d.power_limit = num(&find(kv, "max graphics package power"));
        d.utilization = num(&find(kv, "gpu use"));
        let b = num(&find(kv, "vram total memory"));
        if b >= 0.0 {
            d.mem_total_mb = b / f64::from(1u32 << 20);
        }
        let b = num(&find(kv, "vram total used memory"));
        if b >= 0.0 {
            d.mem_used_mb = b / f64::from(1u32 << 20);
        }
        d.clock_gfx = clock(&find(kv, "sclk clock speed"));
        d.clock_mem = clock(&find(kv, "mclk clock speed"));
        d.fan_speed = num(&find(kv, "fan speed (%)"));
        d.perf_level = find(kv, "performance level");
        devs.push(d);
    }
    devs.sort_by_key(|d| d.index);
    Ok(devs)
}

/// A clock as rocm-smi writes it: `(1800Mhz)`.
fn clock(s: &str) -> f64 {
    let Some(i) = s.find("Mhz") else {
        return -1.0;
    };
    let head = s.get(..i).unwrap_or("").trim_end();
    let start = head
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |j| j.saturating_add(1));
    let n = head.get(start..).unwrap_or("");
    if n.is_empty() || !n.starts_with(|c: char| c.is_ascii_digit()) {
        return -1.0;
    }
    num(n)
}

fn query_intel(pool: &Arc<akio::Pool>) -> (Vec<Device>, String, Option<String>) {
    let out = match output(pool, &["xpu-smi", "discovery", "-j"]) {
        Ok(o) => o,
        Err((o, e)) => return (Vec::new(), o, Some(e)),
    };
    let Ok(disc) = json::parse(out.as_bytes()) else {
        return (
            Vec::new(),
            out,
            Some("parse xpu-smi discovery: not JSON".to_owned()),
        );
    };
    let mut devs = Vec::new();
    for dd in json::get_arr(&disc, "device_list") {
        let id = json::get_int(dd, "device_id");
        let mut d = Device::unknown("intel", id);
        d.name = json::get_str(dd, "device_name");
        d.uuid = json::get_str(dd, "uuid");
        if let Ok(s) = output(pool, &["xpu-smi", "stats", "-d", &id.to_string(), "-j"]) {
            apply_xpu_stats(&mut d, &s);
        }
        devs.push(d);
    }
    (devs, String::new(), None)
}

/// `xpu-smi stats -d N -j` into `d`.
pub fn apply_xpu_stats(d: &mut Device, out: &str) {
    let Ok(v) = json::parse(out.as_bytes()) else {
        return;
    };
    for m in json::get_arr(&v, "device_level") {
        let val = json::get_float(m, "value");
        match json::get_str(m, "metrics_type").as_str() {
            "XPUM_STATS_POWER" => d.power_draw = val,
            "XPUM_STATS_GPU_FREQUENCY" => d.clock_gfx = val,
            "XPUM_STATS_GPU_CORE_TEMPERATURE" => d.temperature = val,
            "XPUM_STATS_MEMORY_USED" => d.mem_used_mb = val,
            "XPUM_STATS_GPU_UTILIZATION" => d.utilization = val,
            "XPUM_STATS_MEMORY_FREQUENCY" => d.clock_mem = val,
            _ => {}
        }
    }
}

/// `system_profiler SPDisplaysDataType -json`.
#[must_use]
pub fn parse_apple_displays(out: &str) -> Vec<Device> {
    let Ok(v) = json::parse(out.as_bytes()) else {
        return Vec::new();
    };
    json::get_arr(&v, "SPDisplaysDataType")
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut d = Device::unknown("apple", i64::try_from(i).unwrap_or(0));
            d.name = json::get_str(it, "sppci_model");
            if d.name.is_empty() {
                d.name = json::get_str(it, "_name");
            }
            let cores = json::get_str(it, "sppci_cores");
            if !cores.is_empty() {
                d.name = format!("{} ({cores} cores)", d.name);
            }
            d
        })
        .collect()
}

/// The X display for nvidia-settings: `$DISPLAY` when well formed, else `:0`.
fn display() -> String {
    let d = std::env::var("DISPLAY").unwrap_or_default();
    let ok = d.split_once(':').is_some_and(|(host, rest)| {
        host.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            && {
                let (a, b) = rest.split_once('.').unwrap_or((rest, "0"));
                !a.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && !b.is_empty()
                    && b.bytes().all(|c| c.is_ascii_digit())
                    && !rest.ends_with('.')
            }
    });
    if ok { d } else { ":0".to_owned() }
}

/// The argv of setting `key` of `vendor` on GPU `index`, its `value` checked (fit's
/// `gpu.Command`): integers in bounds or one of the options, put inside single arguments.
///
/// # Errors
///
/// An unknown vendor or setting, a bad index or value.
pub fn command(vendor_id: &str, index: i64, key: &str, value: &str) -> Result<Vec<String>, String> {
    let v = vendor(vendor_id).ok_or_else(|| format!("unknown vendor {vendor_id:?}"))?;
    let s = v
        .setting(key)
        .ok_or_else(|| format!("{} has no setting {key:?}", v.name))?;
    if !(0..=1024).contains(&index) {
        return Err(format!("invalid gpu index {index}"));
    }
    let value = value.trim();
    let mut repl: Vec<(&str, String)> =
        vec![("{index}", index.to_string()), ("{display}", display())];
    match s.kind {
        KIND_NUMBER => {
            let n = value
                .parse::<i64>()
                .ok()
                .filter(|n| (-100_000..=100_000).contains(n))
                .ok_or_else(|| format!("{}: value must be an integer", s.label))?;
            repl.push(("{value}", n.to_string()));
        }
        KIND_RANGE => {
            let bad = || format!("{}: value must be min,max integers with min ≤ max", s.label);
            let (lo, hi) = value.split_once(',').ok_or_else(bad)?;
            let a = lo.trim().parse::<i64>().map_err(|_| bad())?;
            let b = hi.trim().parse::<i64>().map_err(|_| bad())?;
            if a < 0 || b < a || b > 100_000 {
                return Err(bad());
            }
            repl.push(("{min}", a.to_string()));
            repl.push(("{max}", b.to_string()));
        }
        KIND_ENUM => {
            if !s.options.contains(&value) {
                return Err(format!(
                    "{}: value must be one of {}",
                    s.label,
                    s.options.join(", ")
                ));
            }
            repl.push(("{value}", value.to_owned()));
        }
        KIND_ACTION => {}
        k => return Err(format!("bad setting kind {k:?}")),
    }
    Ok(s.command
        .split_whitespace()
        .map(|a| {
            // Go's strings.Replacer: each place takes the first of the pairs that matches there.
            let mut out = String::new();
            let mut rest = a;
            'outer: while !rest.is_empty() {
                for (k, v) in &repl {
                    if let Some(r) = rest.strip_prefix(k) {
                        out.push_str(v);
                        rest = r;
                        continue 'outer;
                    }
                }
                let mut it = rest.chars();
                if let Some(c) = it.next() {
                    out.push(c);
                }
                rest = it.as_str();
            }
            out
        })
        .collect())
}

/// The title and argv (`sh -c` with the constant script) installing the tool of `vendor` on
/// this system. The caller applies [`Options::wrap`] and runs it as a job.
///
/// # Errors
///
/// An unknown vendor, or none for this system.
pub fn install_command(vendor_id: &str) -> Result<(String, Vec<String>), String> {
    let v = vendor(vendor_id).ok_or_else(|| format!("unknown vendor {vendor_id:?}"))?;
    let family = os_family();
    let m = install_for(v, &family)
        .ok_or_else(|| format!("no install method of {} for {family}", v.name))?;
    Ok((
        m.title.to_owned(),
        vec!["sh".to_owned(), "-c".to_owned(), m.script.to_owned()],
    ))
}

/// The indices of `vendor`'s devices in `st`, or as many as were detected (to expand "every
/// GPU").
#[must_use]
pub fn device_indices(st: &Status, vendor_id: &str) -> Vec<i64> {
    let out: Vec<i64> = st
        .devices
        .iter()
        .filter(|d| d.vendor == vendor_id)
        .map(|d| d.index)
        .collect();
    if !out.is_empty() {
        return out;
    }
    st.vendors
        .iter()
        .filter(|v| v.vendor.id == vendor_id)
        .flat_map(|v| 0..v.detected)
        .collect()
}

#[cfg(test)]
mod tests;
