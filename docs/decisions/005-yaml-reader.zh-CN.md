# 005：YAML 由 akit 自己的读取器读取

[English](005-yaml-reader.md) | [简体中文](005-yaml-reader.zh-CN.md)

状态：于 I24 采纳（2026-11-24）。

## 背景

fit 读取 YAML 写的配置，与读取 JSON 一样（[fit 的决定 019](https://github.com/lesismal/fit/blob/main/docs/decisions/019-yaml-config.zh-CN.md)，
`adapter/yaml`）：YAML 逐键表达 JSON 会表达的内容。映射是对象，序列是数组，标量是其 YAML 1.2 标签所指的
JSON 值；锚点、别名和合并键（`<<`）会被解析，别名展开有上限（2^20 个节点、64 MiB、256 层）；同一个键出现
两次、不是标量的键、fit 配置不认识的标签（`!custom`）、JSON 装不下的数（`.inf`）以及第二个文档都会被拒绝；
空文件是空配置；错误指向 YAML 中的行和列。fit 用 go.yaml.in/yaml/v3（libyaml 的移植）解析，并遍历它的节点树。

akit 必须把同样的文件读进同样的模型（[决定 002](002-config-compatibility.zh-CN.md)），而
[决定 001](001-dependency-policy.zh-CN.md) 把选择留给了 I24：为 fit 接受的子集写一个 akit 自己的读取器，
还是用一个 crate，按体积、`unsafe`、严格程度和依赖数量权衡。

撰写时的 crate 情况：

- **serde_yaml** 已不再维护；它和它的分支（serde_yml）用 unsafe-libyaml 解析，即把 libyaml 机械地翻译成
  Rust，通篇 `unsafe`，并带来 serde、indexmap、itoa 和 ryu。它们解码成不保留位置的值。
- **yaml-rust2**（yaml-rust 仍在维护的分支）及其后继 **saphyr** 是纯 Rust，按 YAML 1.2 规范编写，事件 API
  带位置。它们自带几个 crate（环形缓冲、哈希表、编码，其中一些有 `unsafe`）；它们的加载器在键重复时只保留
  其一，所以重复要在事件上检查。

它们都不是 fit 用的库：YAML 的各种边角（制表符、流集合中的 `:` 和 `?`、锚点名、标签字符、哪些标量是数字）
各有各的读法。要与 fit 一致，无论如何都得在 crate 之上自己写标量解析、合并键、上限和错误，再与 fit 对照检查。

## 决定

- **akit 用自己的读取器读 YAML**，即 `akit-config` 的 `yaml` 模块：没有依赖，没有 `unsafe`（该 crate 是
  `#![forbid(unsafe_code)]`），任何输入都不会 panic（有模糊测试，目标 `config_yaml`），耗时与输入长度成线性。
  它产出与 JSON 读取器相同、带字节偏移的树，因此模型以同样方式读取两者，每个问题都有 YAML 中的行和列：配置
  错误会指出它所涉及的值所在的行和列，与 fit 一样。
- **它读 fit 的库所读的内容**，而不只是模板用到的子集：块集合和流集合，普通、带引号、字面和折叠标量（含 YAML
  的折行与尾部换行处理），显式键（`? k`），锚点、别名和合并键，核心 schema 的标签以及逐字的
  `!<tag:yaml.org,2002:…>` 标签，`%YAML` 指令，`---` 和 `...`。标量按 fit 的库的方式解析（`0x1F`、`0o17`、
  `017`、`1_000`、`.5`，时间戳作为字符串，`yes`/`on` 作为字符串），数字按 fit 的适配器写给其 JSON 解码器的
  形式写出，因此模型的检查看到的与 fit 的一样。
- **fit 怎么做，akit 就怎么做**：同样的拒绝（见上），同样的别名和合并键展开上限，空文件即空配置。
- **对照 fit 检查，而不只是对照规范。** `tools/yaml-diff/run.sh` 把 fit 的适配器构建成命令（Go，离线，fit
  放在 akit 旁边），让它和 akit 分别判定一组语料（手写用例、fit 仓库和文档中的每个 YAML 文件与 YAML 代码块、
  把注释掉的字段写出后的模板，以及对它们的随机编辑），并列出每个读法不同的用例。在 I24 时，总计约 470,000
  个用例的多轮运行把差异降到最后一轮 150,000 个用例中的 2 个，都是 libyaml 的怪异之处（`-` 与注释之间的制表符、
  流序列中的空显式键），akit 拒绝而 fit 接受。语料
  的一个样本连同 fit 的判定已签入（`crates/akit-config/testdata/yaml-fit.txt`），每次测试都会检查。
- **fit 能读而 akit 拒绝的内容**，各自报错，绝不按别的方式读：`%TAG` 指令和具名标签句柄（`!e!x`），报
  "not supported yet"；单独的回车作为换行；嵌套超过 128 层的集合（fit 允许 256 层），这是 JSON 读取器的上限，让解析器的递归保持很浅；
  上述 2 个边角。显式的空文档（只有 `---`）对两者都是空配置，akit
  读成 `{}`，而 fit 的适配器写成 `null`。

## 影响

读取器约 1,700 行代码，由 akit 维护并做模糊测试；换来的是没有需要审查、跟进、并挡在数据面之外的依赖。它只在
控制面加载配置时运行。与 fit 的差异要靠运行差分检查发现，这需要 Go 和一份 fit 检出；签入的样本让其中的用例
在没有这些时也不会回退。`akit adapt`（I73）可以按同样的规则把 YAML 写回。
