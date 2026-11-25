# 009：打包：用 rust-lld 链接的静态 musl 二进制，沿用 fit 的镜像、chart 和 unit

[English](009-packaging.md) | [简体中文](009-packaging.zh-CN.md)

状态：在 I65 中采纳（2026-11-25）。

## 背景

I65 按 fit 的打包方式打包 akit（[fit 的 deploy 目录](https://github.com/lesismal/fit/blob/main/deploy/README.zh-CN.md)、
[fit 的决策 016](https://github.com/lesismal/fit/blob/main/docs/decisions/016-container-image.zh-CN.md)）：带校验和的
发布包、基于 distroless static 镜像的容器镜像、Helm chart 和 systemd unit。fit 关掉 cgo 后 Go 直接给出静态二进制。
Rust 二进制会链接目标平台的 C 库：`*-linux-gnu` 上是动态链接的 glibc，`*-linux-musl` 上是 Rust 静态链接进去的
musl。akit 和 akio 全是 Rust（akit 构建的任何目标上都没有 `cc`，没有 C 依赖），所以 musl 目标不需要为 akit 的代码
准备 C 工具链；只有最后一步链接需要一个认识该目标的链接器。

## 决策

- **Linux 二进制是静态的，来自 Rust 的 musl 目标**（`x86_64-unknown-linux-musl`、`aarch64-unknown-linux-musl`），
  正如开发计划的 I65 所要求。它们能在任何发行版上运行，也能在没有 C 库的 distroless static 镜像上运行。
- **它们由 `rust-lld` 链接**：rustup 随每个工具链提供的链接器，配合 Rust 为 musl 目标提供的 C 运行时。任何主机
  （Linux 或 macOS，x86-64 或 arm64）都能构建两个 Linux 目标，不需要交叉工具链、`cross`、zig 或模拟；镜像的构建
  阶段在构建机自己的平台上运行，也是如此。
- **macOS 二进制**（`x86_64-apple-darwin`、`aarch64-apple-darwin`）在 macOS 上用 Xcode 的链接器构建，与所有 macOS
  程序一样链接系统库。fit 提供的 Windows 和 FreeBSD 版本，等 akio 能在那里运行再说。
- **release profile 不变**：`lto = "thin"`、一个 codegen unit、保留符号，因此 panic 的 backtrace 能给出函数名，
  基准测试主机上的 profile 读的也是发布的同一种二进制。发布包因此稍大（每个约 5 MB）。
- **版本号**：发布版打印的版本来自构建时的 `AKIT_VERSION`（`option_env!`），对应 fit 的 `-X main.version`；没有
  设置时打印 crate 的版本。
- **镜像、chart 和 unit 沿用 fit 的**，换成 akit 的名字和路径（`/etc/akit/akit.json`、`/var/lib/akit`、用户
  `akit`），chart 的控制器是 `lesismal.github.io/akit` 而不是 fit 的，这样两者可以在同一个集群里各自服务自己的类。

## 影响

- 多线程下 musl 的内存分配器比 glibc 的慢。akit 没有设置全局分配器（C 写的分配器是新依赖，除非有测量数据支持，
  否则按[决策 001](001-dependency-policy.zh-CN.md) 拒绝），所以静态二进制使用 musl 的分配器。这是否影响 akit 的
  吞吐量，交给基准测试任务：用 musl 二进制和同一提交的 glibc 构建对比 fixed 和 proxy 场景（开发计划的基准表里有
  这一行）。
- 没有任何签名：镜像没有 cosign 签名或来源证明，发布包只有 `checksums.txt`，macOS 二进制没有公证。这些都需要
  维护者的密钥或身份，写在 [deploy/README.zh-CN.md](../../deploy/README.zh-CN.md) 里。
