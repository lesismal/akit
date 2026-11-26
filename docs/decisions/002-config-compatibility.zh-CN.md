# 002：akit 读取 fit 的配置

[English](002-config-compatibility.md) | [简体中文](002-config-compatibility.zh-CN.md)

状态：已采纳（2026-11-23），由用户在 I02 之前确认；由[决定 037](037-akit-names.zh-CN.md)修订：akit 对外显示的名字
（指标、请求头字段、cookie、插件 ABI、以网关命名的默认值）是 akit 的，不是 fit 的。

## 背景

akit 和 fit 在两个引擎上做同样的事。它们的用户、示例、模板、Admin API 客户端和控制台页面可以共用，也可能分裂成
两种逐渐走远的方言。

## 决定

- **fit 的配置模式就是 akit 的。** 字段名、默认值、含义和三种格式（.conf、YAML、允许注释和尾随逗号的 JSON）都相同，
  以网关命名的默认值除外（决定 037）。Admin API 的路径和载荷、CLI 的命令和参数、访问日志字段也都相同；指标名和请求头
  字段是 fit 的名字把 `fit` 改为 `akit`。
- **尚未支持即拒绝。** akit 还没实现的字段是一个指出它的错误（"not supported yet: `<path>`"），绝不读入后忽略。这些
  字段的列表由兼容性门禁（I07a、I07b）生成到 `docs/compatibility.md` 中，随里程碑逐步缩短。
- **配置模式由 fit 主导。** 新字段先在 fit 中设计，或者两边同时设计；akit 不添加 fit 不会接受的字段。akio 允许而 fib
  不允许的东西（每个监听器的 io_uring 后端、eBPF 分流），对应字段也向 fit 提出，fit 可以接受它并在 fib 上忽略、附上
  文档说明，也可以拒绝。
- **门禁是一项测试。** CI 用固定的 fit 提交，把 fit 的模板和 fit-examples 跑一遍 `akit validate`。移动这个固定版本
  是需要审查的改动。

## 影响

在 fit 和 akit 之间迁移配置只需换一个二进制，再加上决定 037 改名的那些名字；文档和示例两边通用，fit 的工具要改用 akit
的名字。akit 不能独自改进配置，必须跟随 fit 的变化，
门禁让这些变化可见。
