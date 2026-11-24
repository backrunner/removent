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

`packages/host/examples/wan_latency.rs` 使用实际 QUIC 双向认证、relay 认证与 RVP 控制泵，
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

原始 CSV：[QUIC relay](../benchmarks/2026-09-20-wan-quic-scheduler.csv)、
[WebSocket](../benchmarks/2026-09-20-wan-websocket-scheduler.csv)、
[直连](../benchmarks/2026-09-20-wan-direct-scheduler.csv)；
[本轮环境](../benchmarks/2026-09-20-wan-scheduler-environment.json)、
[第一轮高负载环境](../benchmarks/2026-09-20-wan-environment.json)。

## 2026-11-24 中继转发复测

QUIC 中继将密文数据报直接交给各目标的有界发送队列。队列满时允许淘汰尚未发送的旧数据报，转发循环可以继续处理其他目标。WebSocket 中继在同一主机向多目标转发时，满队列只影响对应目标。独占一个目标的转发循环保留最多 50 ms 背压。WebSocket 注册确认进入队列后才公布路由，避免首批载荷抢在确认之前。

在同一台 M5 Max 上使用下面的 `wan_latency` 命令分别运行修改前后的 release 二进制，每个场景采样约 6 秒。QUIC 150 ms RTT、2 Mbps、每段 5% 丢包的输入 p95 从 441.33 ms 降至 387.46 ms，控制 RTT p95 从 1,642.19 ms 降至 1,229.21 ms。无丢包过载场景的控制 RTT p95 从 175.19 ms 升至 200.61 ms。WebSocket 单会话停顿场景的输入 p95 位于两次基线之间。最终版两个载具的八个场景共 488 对按下和松开均完整交付。数据来自本机代理和合成媒体负载，公网上的实际呈现时延仍待测。[逐场景原始结果](../benchmarks/2026-11-24-relay-forwarding.csv)

## 2026-11-24 拥塞控制对照

在同一台 M5 Max 上运行 release 版 `wan_latency`，每场景约 8 秒。两次 BBR 基线包夹外层 CUBIC 测试，内层 QUIC 都使用 BBR。另一组在直连、QUIC Relay 和 WebSocket Relay 上将现有应用发送窗口额外限制为实时拥塞窗口加 16 KiB，最小允许值为 32 KiB。该限制约束等待确认的应用流字节，QUIC 仍自行处理发包和丢失恢复。[Quinn 应用发送窗口](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs) [完整 CSV](../benchmarks/2026-11-24-congestion-control.csv)

| QUIC Relay 场景 | BBR 两次基线 | 外层 CUBIC |
| --- | ---: | ---: |
| 无丢包过载，媒体实际吞吐 | 4.51–5.17 Mbps | 7.23 Mbps |
| 每段 2% 丢包，控制 RTT p95 | 242–259 ms | 780 ms |
| 150 ms RTT、2 Mbps、每段 5% 丢包，控制 RTT p95 | 1.00 s | 4.11 s |
| 同一重度丢包场景，2 秒未收齐 Pong | 0/81 | 55/81 |

直接限制应用发送窗口的收益也不一致。重度直连的输入 p95 从 298 ms 降至 173 ms，但控制 RTT p95 从 609 ms 升至 818 ms。重度 QUIC Relay 的控制 RTT p95 从 1.00 s 升至 1.62 s，媒体吞吐从 1.13 Mbps 降至 1.06 Mbps。WebSocket 重度停顿的控制 RTT p95 从 1.55 s 降至 1.52 s。两项候选均已撤回。

所有 32 个试验场景的按键按下和松开均完整抵达。Pong 超时计数包括超过 2 秒才收到的回包和收尾前缺失的回包。每组仍是合成负载的短测，实际画面呈现及公网效果待测。下一项对照应让同一会话经历带宽骤降和恢复，并记录达到新码率所用时间、视频帧龄及重新提档所需时间。

## 2026-11-24 同会话带宽骤降

新增 `wan_latency <direct|quic|websocket> 30 step` 场景。同一条加密会话在采样开始后第 4 秒从 20 Mbps 降至 2 Mbps，第 16 秒恢复至 20 Mbps。模拟容量同时作用于各方向和各段，名义 RTT 为 80 ms，UDP 队列满时丢包。主机使用生产的动态清晰度控制器和媒体写入路径。接收端对合成视频完整帧计算从发送准备到收齐的时间，使用同一主机的单调时钟。样本隔离发送侧自适应，涵盖合成帧与控制消息的真实传输，缺少接收端解码反馈、显示时序和公网路径。

主机现在单独识别持续受阻的视频写入。写入耗时达到 100 ms 后产生背压证据，持续观测至少 500 ms 可提前一档。单个已完成的慢写入只作为一次新证据。只提前每次拥塞的首降，并以原有首降最早时间为后续降档的冷却起点。链路连续健康五秒后可再次提前首降，提档沿用既有节奏。

连续快降的试验把 QUIC Relay 的低速阶段帧龄 p95 从 1,038 ms 降到 779 ms，同时将恢复阶段的完整帧从 326 帧降到 121 帧。最终做法将首次降档从 1,498 ms 提前到 999 ms，低速阶段帧龄 p95 为 929 ms，恢复阶段接收 327 帧，首次提档仍约在恢复后 5.75 秒。最终做法在该低速窗口内最低目标为约 2.02 Mbps，实际可用媒体码率还受协议开销影响。[完整分阶段记录](../benchmarks/2026-11-24-bandwidth-step.json)

| 路径 | 首次降档，基线 → 最终 | 低速段完整帧帧龄 p95，基线 → 最终 | 恢复段完整帧，基线 → 最终 |
| --- | ---: | ---: | ---: |
| 直连 | 1,499 → 748 ms | 789 → 766 ms | 323 → 323 |
| QUIC Relay | 1,498 → 999 ms | 1,038 → 929 ms | 326 → 327 |
| WebSocket Relay | 1,748 → 999 ms | 788 → 661 ms | 384 → 379 |

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
原始数据：[relay-wan-tuning.jsonl](../benchmarks/2026-09-20-relay-wan-tuning.jsonl)。

开发机还有独立于本任务的高负载作业。样本会受本机调度影响，不能外推稳定 SLA 或宣称
任意公网弱网都不影响操作。真实 macOS 注入到屏幕反馈、跨运营商路径与 Cloudflare 实例
仍需在可用的测试环境中验收。
