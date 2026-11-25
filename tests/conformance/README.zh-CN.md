# Gateway API 一致性测试套件

[English](README.md) | [简体中文](README.zh-CN.md)

M4 的退出标准（I60 和 I66 的完成标准）是 akit 在 fit 锁定的 Gateway API 版本（v1.3.0）上通过
fit 声明的一致性 profile `GATEWAY-HTTP`。该套件是 Go 包
`sigs.k8s.io/gateway-api/conformance`：一个测试，把它的清单应用到真实集群，等待实现接受并编排
（program）它们，再向 Gateway 的地址发送请求。akit 按 fit 的方式运行它
（[fit 的决策 012](https://github.com/lesismal/fit/blob/main/docs/decisions/012-gateway-conformance.zh-CN.md)），
使用 fit 自己的套件模块，该模块不依赖 fit 的任何代码：akit 的代码树中不引入 Go 代码。

[`run.sh`](run.sh) 是 fit 的 `test/conformance/run.sh` 的 akit 版本：它创建 akit 的
GatewayClass（`akit`，controller 为 `lesismal.github.io/akit`，以免在 fit 与 akit 同时服务的集群中
接管 fit 的 Gateway），在集群外用 Kubernetes provider 启动 `akit run`（Gateway 绑定在 127.0.0.1，
并经 `addresses` 把该地址写入其状态），再以 akit 声明的 profile 运行该套件。

## 结果

akit 通过了它：[reports/gateway-http-v1.3.0-2026-11-25.yaml](reports/gateway-http-v1.3.0-2026-11-25.yaml)
是套件写出的报告（I66，在基准测试主机上，Linux 7.0，kind v0.30.0，akit 位于 `346ad8b`）：
**`GATEWAY-HTTP` core，通过 33，失败 0，跳过 0**，即 fit 所声明的该 profile 的 core（fit 的 I61：
Gateway、HTTPRoute 和 ReferenceGrant，同样的 33 个测试）。套件的其余 38 个测试属于扩展特性和其他
profile，因 fit 和 akit 都未声明而被跳过；某个特性在其测试通过后，以 `run.sh` 中的一个参数声明
（fit 的决策 012）。

- akit 在集群外以 akio 的 epoll 后端运行：`akit run` 没有切换到 io_uring 的开关（akio 的 `Backend`
  在代码中按监听器选择，只有 akit 的测试通过 `AKIO_TEST_BACKEND` 设置它），因此无法让套件针对
  io_uring 上的 akit 运行。provider 和监听器自己的测试在两种后端上都运行。
- 扩展特性尚未针对 akit 运行过；原因和下一步见
  [M4 报告](../../bench/results/m4-report-2026-11-25.zh-CN.md)。

CI 在 "Gateway API conformance (kind)" 作业（[ci.yml](../../.github/workflows/ci.yml)）中运行它，
与 fit 的 CI 相同：检出 akit 所读 fit 提交中的套件模块，一个 kind 集群，实验通道的 CRD，带
`CAP_NET_BIND_SERVICE` 构建的 akit，`run.sh`，并把报告和 akit 的日志保存为 artifact。

## 运行方法

CI 在每次推送时运行它；在别处，一台装有 Docker 的 Linux 机器按下面的步骤运行。在基准测试主机上，
同一时间只有一个会话（规则 23 和 30），并持有共享锁。

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

   在已有另一个 kind 集群的主机上，给这个集群单独的 Pod 和 Service 子网（kind 配置中的
   `networking.podSubnet` 和 `serviceSubnet`，例如 `10.250.0.0/16` 和 `10.251.0.0/16`），以免到其
   Pod 的路由与另一个集群冲突。

4. 清理（规则 29）：`kind delete cluster --name akit`，删除路由（`sudo ip route del "$cidr"`），
   读过之后删除套件的二进制、报告和日志。

套件报告的失败在 provider（`crates/akit-provider/src/kubernetes`）中修复，并在移植自 fit 的测试
旁边加上它自己的测试，然后再次运行套件。
