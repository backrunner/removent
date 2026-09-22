# 公网弱网与操作延迟

本机可重复的链路模拟，日期 2026-09-20。不是公网或 Cloudflare 实测，不含真实采集、
解码、屏幕刷新或 macOS 事件注入时间。网络传播、丢包重传和 TCP 队头阻塞不能保证为零；
本次目标是去掉应用自己增加的阻塞，保证会话内输入顺序，并测出余下的延迟。

## 实现

- 原生控制流优先级高于音视频，收发独立推进。一个未完成的写 future 跨 `select!`
  保留，避免取消后重发半条消息、破坏协议帧。
- 原生 UI 与 VNC 复用有界有序输入队列。只替换相邻、同显示器、同按钮状态和同类型的
  移动事件；最后位置、点击、按下/松开、滚轮、坐标几何事件保持顺序。
  满队列显式报错，应用结束连接，由被控端清理所持输入，不能静默丢弃松开。
- 最大 4 MiB 的剪贴板快照改走独立、低优先级 bidi 流，30 秒期限、最新待发快照覆盖旧快照。
  使用同一个双向认证连接。剪贴板阻塞不再把后续按键排在文本后面。
- Quinn 使用 BBR；视频按 4 KiB 写并让出调度，及时释放已确认的小分配，给控制写入调度机会。
  发送预算随选定码率和基础 RTT 调整为 64 KiB 至 2 MiB；队列膨胀不能反过来增大发送预算。
- relay 的 QUIC DATAGRAM 收/发队列各从 512 KiB 缩至 32 KiB；WebSocket 每方向从
  128 条缩至 16 条，最大 2056 字节/条。WebSocket 读写独立，保留背压、超时和资源上限。
- 保留现有实际写入压力/丢包驱动的降码率、降帧率和静态画面探测。可靠视频流仍可能
  因丢包等候重传；优先级不能跳过已进入同一 TCP 字节流的数据。

## 测量方法

`crates/host/examples/wan_latency.rs` 使用实际 QUIC 双向认证、relay 认证与 RVP 控制泵，
输入落点是内存 `InputSink`。源视频是按码率生成的负载，初始 12 Mbps / 30 fps，
使用生产 `DeliveryHealth` 与自适应控制器，接收端只读数据，不解码。

每场景预热 1 秒，目标每 100 ms 产生按下、松开与 Ping；初测 15 秒，调度监测复测 10 秒。
回包慢也不降低输入频率，操作系统错过的定时周期跳过，不突发补发。
之后留 2 秒收尾，检查所有按下/松开在 teardown 强制释放之前均到达。
输入延迟为同一台机器的单调时钟：发送端入队 → 被控端 mock 注入。
控制 RTT 为入队 → 对应 Pong；超过 2 秒及未返回均计入 `timeouts`，缺失回包不进入分位数。
这些数值均不是用户按键到屏幕显示变化的完整延迟。

测试工具另外输出 `sample_seconds`（实际采样时长）和 `scheduler_lag_max_ms`
（独立 10 ms 定时器最大调度迟到）。主机调度迟到很大时，不能把测到的延迟全归因于网络。
第一轮 15 秒原始 CSV 保留异常现场，尚未包含这两个字段，其媒体吞吐分母用名义时长，
在严重调度停顿下也会失真；不用于带宽结论。后续带 `scheduler` 后缀的复测使用实际时长。

`examples/wan/link.rs` 提供普通用户权限的 UDP/TCP 本地代理，不修改系统路由或网卡：

| 场景 | 总名义 RTT | 每方向限速 | UDP 随机丢包/每段 | TCP 字节流停顿/每段 |
| --- | ---: | ---: | ---: | ---: |
| idle | 80 ms | 8 Mbps | 0 | 0 |
| video-overload | 80 ms | 8 Mbps | 0 | 0 |
| loss-or-stall | 80 ms | 8 Mbps | 2% | 200 ms |
| severe | 150 ms | 2 Mbps | 5% | 300 ms |

UDP：每方向 50 ms 路由队列、固定伪随机种子；随机丢包消耗链路序列化时间，队列溢出另计。
直连一个代理，relay 两个代理，各段丢包会累积，QUIC 隧道分片还会放大内层丢包。
`proxy_dropped_packets` 包含随机丢包和拥塞丢包，不能当作设定的丢包率。
TCP：保留可靠字节序，在约每秒插入一次停顿；**没有丢弃 TCP 字节**，它是重传停顿模型，
不是内核 TCP 丢包仿真。WebSocket 外层为 loopback 明文 WS，内部始终是加密 RVP；
不包含 Cloudflare TLS、Worker、Durable Object 或 Container 开销。

## 延迟复测结果

完成本任务的编译和资源压测之后，串行复测每场景 10 秒，记录独立调度探针。
下表 relay 场景的探针最大迟到为 2.67–9.37 ms；它们的空闲输入 p95 约 45 ms，
与 80 ms 名义 RTT 的单程 40 ms 基线相符。第一轮高负载异常数据没有删除或用于比较优化幅度。

| 路径 / 场景 | 输入到达 p95 | 控制 RTT p95 | 控制 RTT p99 | 超过 2 秒/未返 Pong |
| --- | ---: | ---: | ---: | ---: |
| QUIC relay / 视频超过 8 Mbps 链路容量 | 44.28 ms | 190.02 ms | 274.14 ms | 0/101 |
| QUIC relay / 每段 2% 丢包 | 75.23 ms | 222.70 ms | 277.48 ms | 0/101 |
| QUIC relay / 150 ms RTT、2 Mbps、每段 5% 丢包 | 308.37 ms | 1329.13 ms | 1729.12 ms | 0/101 |
| WebSocket / 视频超过 8 Mbps 链路容量 | 44.11 ms | 182.38 ms | 190.06 ms | 0/101 |
| WebSocket / 每段周期性停顿 200 ms | 228.58 ms | 490.89 ms | 514.43 ms | 0/101 |
| WebSocket / 150 ms RTT、2 Mbps、周期性停顿 300 ms | 330.92 ms | 1448.20 ms | 1632.26 ms | 0/101 |

以上各场景均发出并收到 101 次按下和 101 次松开，接收发生在 teardown 清理之前。
包括直连对照在内，12 个场景共 1212 对按下/松开完整交付，全部 Pong 在 2 秒内返回；
这一轮各场景的调度探针最大迟到不超过 9.37 ms。
中度丢包下输入保持较短延迟，重度场景仍显著变慢，**不能把它验收成“弱网完全不影响操作”**。
控制回包也不是画面反馈：可视反馈还会叠加视频可靠流等候、采集、编码、解码和显示延迟。
Cloudflare 的 WebSocket/TCP 无法提供 QUIC 独立流相同的丢包隔离，应保留原生 QUIC VPS 路径供选择。

原始 CSV：[QUIC relay](benchmarks/2026-09-20-wan-quic-scheduler.csv)、
[WebSocket](benchmarks/2026-09-20-wan-websocket-scheduler.csv)、
[直连](benchmarks/2026-09-20-wan-direct-scheduler.csv)；
[本轮环境](benchmarks/2026-09-20-wan-scheduler-environment.json)、
[第一轮高负载环境](benchmarks/2026-09-20-wan-environment.json)。

## 复现

```bash
cargo build --release --locked -p removent-host --example wan_latency -j 2
target/release/examples/wan_latency direct 15
target/release/examples/wan_latency quic 15
target/release/examples/wan_latency websocket 15
cargo test --locked -p removent-net -p removent-client -p removent-host -p removent-relay --lib --tests -j 2 -- --test-threads=1
```

新增回归覆盖：部分控制帧写入被反复取消后仍正确解码；4 MiB 剪贴板接收完全停住时
松开消息仍能到达；下行 WebSocket 写死不挡上行；控制写死时仍接收松开；
50,000 次鼠标移动合并后保留最后位置与 500 对按键顺序。已有取消/断开输入清理继续验证。

验证：net/client/host/relay 的库和集成测试共 165 项通过、4 项显式 opt-in 测试跳过；
包含桌面应用在内的五个相关 crate 通过 `clippy --all-targets -- -D warnings`。
这不是完整 workspace/真实 GUI 验收。独立 relay 子进程资源复测另保留 JSONL。

## relay 资源复测

独立 relay 进程，release、每阶段 5 秒、10 核 Apple M4 / macOS 27；100% CPU 表示占满一个核。
此处仍是 loopback，不含网络损伤、编码器或 Cloudflare 基础设施开销。

| 状态 | QUIC | WebSocket |
| --- | ---: | ---: |
| 无连接 CPU | 0.0024% | 0.0078% |
| 无连接 RSS 峰值 | 8.17 MiB | 7.22 MiB |
| 32 台主机待命 CPU | 0.0266% | 0.0242% |
| 32 台主机待命 RSS 峰值 | 10.48 MiB | 8.45 MiB |
| 4 × 24 Mbps 实际吞吐 | 94.57 Mbps | 94.49 Mbps |
| 同负载 CPU / RSS 峰值 | 40.59% / 9.63 MiB | 12.74% / 7.58 MiB |
| 同负载控制 RTT p95 | 12.61 ms | 8.57 ms |

两种传输峰值均为 3 个线程。缩小队列后仍完成本次约 95 Mbps 的吞吐目标；短样本和
同机其他负载不允许据此宣称跨版本 CPU 的稳定提升或下降，也不是最大容量测试。
原始数据：[relay-wan-tuning.jsonl](benchmarks/2026-09-20-relay-wan-tuning.jsonl)。

开发机还有独立于本任务的高负载作业。样本会受本机调度影响，不能外推稳定 SLA 或宣称
任意公网弱网都不影响操作。真实 macOS 注入到屏幕反馈、跨运营商路径与 Cloudflare 实例
仍需在可用的测试环境中验收。
