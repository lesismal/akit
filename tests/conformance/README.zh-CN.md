# Gateway API 一致性测试套件

[English](README.md) | [简体中文](README.zh-CN.md)

I60 的完成标准是 akit 通过 Gateway API 的 `GATEWAY-HTTP` 一致性 profile。该套件是 Go 包
`sigs.k8s.io/gateway-api/conformance`：一个测试，把它的清单应用到真实集群，等待实现接受并编排
（program）它们，再向 Gateway 的地址发送请求。akit 按 fit 的方式运行它
（[fit 的决策 012](https://github.com/lesismal/fit/blob/main/docs/decisions/012-gateway-conformance.zh-CN.md)），
使用 fit 自己的套件模块，该模块不依赖 fit 的任何代码：akit 的代码树中不引入 Go 代码。

[`run.sh`](run.sh) 是 fit 的 `test/conformance/run.sh` 的 akit 版本：它创建 akit 的
GatewayClass（`akit`，controller 为 `lesismal.github.io/akit`，以免在 fit 与 akit 同时服务的集群中
接管 fit 的 Gateway），在集群外用 Kubernetes provider 启动 `akit run`（Gateway 绑定在 127.0.0.1，
并经 `addresses` 把该地址写入其状态），再以 akit 声明的 profile 运行该套件。

## 运行方法

由维护者在基准测试主机或其他装有 Docker 的 Linux 机器上运行，同一时间只有一个会话（规则 23 和
30）；功能任务不运行它。

1. 一个 kind 集群，fit 的模块所要求版本（v1.3.0，TLSRoute、TCPRoute 和 UDPRoute 需要实验通道）
   的 Gateway API CRD，以及经节点到 Pod 网络的路由：

   ```sh
   kind create cluster --name akit --wait 120s --kubeconfig "$PWD/kubeconfig"
   export KUBECONFIG=$PWD/kubeconfig
   kubectl apply --server-side -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.3.0/experimental-install.yaml
   node=$(docker inspect -f '{{.NetworkSettings.Networks.kind.IPAddress}}' akit-control-plane)
   cidr=$(kubectl get node akit-control-plane -o jsonpath='{.spec.podCIDR}')
   sudo ip route add "$cidr" via "$node"
   ```

2. 允许绑定 80 和 443 端口的 akit 二进制，以及由 fit 的模块构建的套件测试二进制：

   ```sh
   cargo build --release -p akit
   sudo setcap cap_net_bind_service=+ep target/release/akit
   (cd ../fit/test/conformance && go test -c -o "$PWD/conformance.test" .)
   ```

3. 运行套件，报告写入 `report.yaml`，akit 的日志写入 `akit.log`：

   ```sh
   tests/conformance/run.sh target/release/akit ../fit/test/conformance/conformance.test
   ```

4. 清理（规则 29）：`kind delete cluster --name akit`，删除路由（`sudo ip route del "$cidr"`），
   读过之后删除套件的二进制、报告和日志。

套件报告的失败在 provider（`crates/akit-provider/src/kubernetes`）中修复，并在移植自 fit 的测试
旁边加上它自己的测试，然后再次运行套件。
