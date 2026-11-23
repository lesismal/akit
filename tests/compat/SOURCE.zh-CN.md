# 兼容性夹具的来源

[English](SOURCE.md) | [简体中文](SOURCE.zh-CN.md)

`fixtures/` 存放 fit 配置模板和 fit-examples 配置的副本，固定在下表的提交上，供
[兼容性门禁](../../docs/compatibility.zh-CN.md) 使用。它们是复制进来的，而不是测试运行时再去拉取，
所以门禁可以离线运行、在每台机器上结果一致，fit 的变化只有在移动固定提交时才会进入 akit。

| Repository | Commit | What is copied |
| --- | --- | --- |
| fit | `7daee573b102d87837cc10f033e4cc5894769210` | `templates/`（`.conf`、YAML 和 JSON，各有单文件和 `conf.d` 目录两种形式）、`schema/fit.schema.json`、`LICENSE` |
| fit-examples | `9659d7e4b30a6f97d908536c85c2b2cf5b75fe8d` | 每个 `*/fit.conf`、`LICENSE` |

（表头保持英文，以便 `update-fixtures.sh` 在两种语言里用同一规则改写哈希。）`fixtures/PINS`
为工具和测试保存同样的两个哈希。

- fit：<https://github.com/lesismal/fit>
- fit-examples：<https://github.com/lesismal/fit-examples>

## 许可证

两个仓库都采用 Apache License 2.0，与 akit 相同。它们的许可证文件保存在副本旁边：
[fixtures/fit/LICENSE](fixtures/fit/LICENSE) 和
[fixtures/fit-examples/LICENSE](fixtures/fit-examples/LICENSE)。文件原样复制，未作修改；两个仓库
都没有 NOTICE 文件。

## 没有复制的内容

fit-examples 只复制配置：示例的其他文件（Docker Compose 文件、Kubernetes 清单、数据、WebAssembly
插件的 Go 源码）不是 fit 配置。fit-examples 自己的测试还会在环境变量中设置替身密钥、构建部分示例
加载的 WebAssembly 插件，并且对 `https` 示例只做 adapt（其证书文件由用户提供）；门禁中对应的内容
在 `src/lib.rs`（`EXAMPLE_SECRETS`、`NO_VALIDATE`）和 `make compat-gate` 中。

## 移动固定提交

```bash
tests/compat/update-fixtures.sh <fit-commit> <fit-examples-commit>
```

脚本在给定提交上克隆两个仓库（`FIT_REPO` 和 `FIT_EXAMPLES_REPO` 可指向本地克隆），替换
`fixtures/`，改写 `fixtures/PINS` 以及两种语言中的上表，并重新生成字段清单（`fit-fields.txt`）和
`docs/compatibility.md` 的表格。移动固定提交是需要评审的变更（决策 002）：单独提交，并在评审时
阅读 `fit-fields.txt` 的 diff，因为 fit 新增的每个字段，在 akit 支持它之前都会被 akit 拒绝。
