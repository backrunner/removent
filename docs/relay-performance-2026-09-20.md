# Relay 资源占用与公网画面压缩评估

日期：2026-09-20。结论适用于当前 v1 实现；这是本机测量，不是 Cloudflare/VPS 的容量承诺。

后续的输入优先级、动态发送预算、BBR 与 relay 队列调整及复测见
[公网弱网与操作延迟](wan-latency-2026-09-20.md)。以下表格保留本轮压缩/资源优化当时的原始结果，
不能直接当作后续传输改动的复测值。

## 已落实的调整

- relay 可执行程序的 Tokio I/O worker 默认最多 2 个，单核分配为 1 个；可用
  `TOKIO_WORKER_THREADS` 覆盖。连接使用异步任务，不为每条连接创建线程。
- WebSocket 读缓冲从每连接 128 KiB 改为 16 KiB，减少 87.5% 的这部分预留。
  最大消息仍为 2056 字节，16 KiB 可以容纳多条消息，收发队列和认证限制保持有界。
- 降画质后的静态恢复探测使用帧间压缩，不再每秒强制发送完整关键帧。
  画质切换、编码器重建和控制端明确请求仍强制关键帧；完整画质下不发送静态探测。
  这是被控端的改动，relay 不解密、不转码。

## 调整后资源实测

| 状态 | QUIC CPU（单核） | QUIC RSS 峰值 | WebSocket CPU（单核） | WebSocket RSS 峰值 |
| --- | ---: | ---: | ---: | ---: |
| 无连接 | 0.015% | 8.11 MiB | 0.005% | 7.22 MiB |
| 1 台主机待命 | 0.020% | 9.55 MiB | 0.003% | 7.77 MiB |
| 1 路连接无视频 | 0.070% | 9.81 MiB | 0.021% | 7.84 MiB |
| 32 台主机待命 | 0.196% | 11.45 MiB | 0.041% | 9.81 MiB |
| 断开全部连接后 | 0.000% | 11.44 MiB | 0.003% | 9.81 MiB |

| 传输 | 目标负载 | 实际 payload | CPU（单核） | RSS 峰值 | 控制 RTT p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| quic | 1 × 8 Mbps | 7.85 Mbps | 7.76% | 9.98 MiB | 3.31 ms |
| quic | 1 × 24 Mbps | 23.57 Mbps | 18.05% | 10.05 MiB | 6.48 ms |
| quic | 4 × 12 Mbps | 47.24 Mbps | 21.36% | 10.48 MiB | 9.73 ms |
| quic | 4 × 24 Mbps | 94.53 Mbps | 32.22% | 10.56 MiB | 12.56 ms |
| websocket | 1 × 8 Mbps | 7.84 Mbps | 2.06% | 7.98 MiB | 1.38 ms |
| websocket | 1 × 24 Mbps | 23.55 Mbps | 7.10% | 8.20 MiB | 3.67 ms |
| websocket | 4 × 12 Mbps | 47.12 Mbps | 10.89% | 8.94 MiB | 6.18 ms |
| websocket | 4 × 24 Mbps | 94.31 Mbps | 16.86% | 9.20 MiB | 9.79 ms |

这组样本中，空闲无连接约 7–8 MiB；32 台主机待命不超过 12 MiB、0.2% 单核 CPU。
被测进程线程数从基线的 11 降为 3（主线程 + 2 个 worker）。WebSocket 32 台主机
待命的 RSS 采样峰值从 13.33 MiB 降至 9.81 MiB，符合缩小读缓冲的预期。
这不意味着 CPU 每种负载都会同比下降：共享机器上的单次测量存在明显波动。
基线还出现过约 132 ms 的控制 RTT p95；调整后这次运行各负载 p95 为 1–13 ms，
不能仅凭这两次运行就保证生产延迟上限或排除外部调度影响。

断开后的 RSS 不一定回到刚启动数值，包含 allocator 缓存和已触达页面；短测没有证明
长时间运行不存在增长。吞吐最高一档是设定的测试负载，**不是测出的饱和吞吐上限**。

## 测量方法与可复现性

设备为 Apple M4 / Mac16,10，10 个逻辑核心、16 GiB RAM、macOS 27.0。
使用 release 构建。机器存在其他任务负载，测量过程中出现调度延迟及内存压力；
吞吐、延迟和 RSS 都应连同这些条件理解。不能把两次单次运行的差值解释为精确的优化比例。

`resource_benchmark` 启动独立 relay 子进程；Python 只读取这个 PID 的累计 CPU 时间、
RSS 和线程数，排除两端 RVP、加密、负载生成器和采样器自己的 CPU/内存。
macOS 使用 `proc_pidinfo` 并转换 Mach 时间单位，Linux 使用 `/proc`；CPU 计时与
Python `process_time()` 的独立对照测试防止 Apple Silicon 上把时钟 tick 当纳秒。
**CPU 100% 代表占满一个逻辑核心**。RSS 每 250 ms 采样，峰值是采样峰值，
不包括操作系统 socket 缓冲、容器平台和其他进程，也不是硬内存上限。

链路使用真实角色凭据、设备签名、双方内层证书 pin、QUIC RVP 媒体和控制流。
视频负载为持续单向 uni stream 上每秒 30 次写入；同时在持久控制流上每秒进行
10 次回声往返。校验收到的全部媒体和控制数据。1 路是 1 个 host + 1 个 viewer；
4 路是 4 个独立房间、8 个外层连接。每个负载阶段先预热 2 秒。
比特率按收到的应用 payload / 整个阶段完成时间计算，包含控制回声完成时间；
不是网卡线速，也不包含协议头、加密、ACK 和重传的额外流量。

QUIC 包含 relay 的外层 TLS 加解密。WebSocket 使用仅限 loopback 测试的明文外层，
内部 RVP 仍加密；它模拟容器 origin，不计 Cloudflare edge TLS、Worker、DO 和网络。
因此不能据此断言公网 WSS 比 QUIC 更快。UDP 可用时优先 QUIC；有丢包时 WSS/TCP
的队头阻塞仍需在真实链路上验证。

```sh
cargo build --locked --release -p removent-relay --bin removent-relay --example resource_benchmark
python3 scripts/benchmark_relay.py --seconds 10 > /tmp/relay-resources.jsonl
# 显式线程配置对照：
TOKIO_WORKER_THREADS=4 python3 scripts/benchmark_relay.py --seconds 10

cargo build --locked --release -p removent-media-codec --example screen_compression
target/release/examples/screen_compression /tmp/screen-compression > /tmp/screen-compression.csv
# 仅针对尚未加密的编码结果作二次压缩对照：
zstd -1 -q -c /tmp/screen-compression/Hevc-local-motion-deduptrue-intrafalse.annexb > /tmp/local-motion.zst
```

资源原始记录：[调整前](benchmarks/2026-09-20-relay-before.jsonl)、
[调整后](benchmarks/2026-09-20-relay-after.jsonl)。基线每阶段 6 秒，调整后每阶段 8 秒。
这组短测不覆盖长期泄漏、最大 128 连接拥塞、网络攻击或特定小型 VPS 的饱和容量。

## 动态/静态画面已经怎样压缩

当前链路为：捕获 BGRA → 完全相同帧去重 → HEVC（或 H.264）→ RVP 端到端加密 → relay。

HEVC/H.264 自带块级帧间预测、运动补偿、残差与熵编码。静态区域通常使用参考帧/跳过块，
滚动和移动区域利用运动向量，其余变化编码残差。因此局部变化不等同于重新发送整幅
独立截图。当前关闭 B 帧重排以控制延迟；VideoToolbox 的 temporal compression 默认开启。
原始帧去重还可以直接避免整次编码、IOSurface 拷贝和发送；只有成功发送过的帧才作为
去重依据，丢帧和编码失败仍会重试。

公网与局域网使用相同的压缩能力。本报告完成后的适配策略已改为优先保留文字细节、
降低帧率，保持捕获尺寸，见[清晰度报告](readable-quality-2026-09-20.md)。
恢复时逐步提升；不会因为正常的公网 RTT 比局域网高就一直降画质。这里应根据实际链路
和内容选择质量，而不是仅根据是否走 relay 强制改用另一套编码。

## 视频测量

使用生产 `VideoEncoder`/`VideoDecoder`，1920×1080、目标 4 Mbps、30 fps、6 秒。
内容为合成密集文字纹理，局部运动覆盖 640×360（11.1%），另测全屏滚动与动态纹理。
所有实际编码帧均完成解码；每秒采样 BGR PSNR。合成结果不代表真实文字可读性、
电影压缩率或主观画质验收。这里的 Mbps 按 **画面时间** 计算，不按跑完 benchmark 的墙钟时间。
原始 BGRA 在此分辨率/帧率约为 1991 Mbps。

| 场景 | H.264 数据率 | HEVC 数据率 | 说明 |
| --- | ---: | ---: | --- |
| 静态，开启整帧去重 | 首帧 544,576 B | 首帧 536,743 B | 180 次捕获只编码 1 帧；以后不再发送视频，保活另计 |
| 11.1% 区域运动 | 5.35 Mbps | 5.44 Mbps | 包含首次及周期关键帧 |
| 全屏滚动 | 5.72 Mbps | 4.70 Mbps | 运动补偿仍有作用 |
| 全屏动态纹理 | 5.06 Mbps | 4.16 Mbps | 不能泛化为所有影片的码率 |
| 局部运动，每帧强制关键帧 | 323.91 Mbps | 211.81 Mbps | 用于展示移除时间参考的代价，非生产配置 |

相对于每帧独立编码，局部运动的帧间编码少传约 **98.35% / 97.43%**（H.264 / HEVC）。
采样 PSNR 分别为 31.21 vs 32.36 dB、32.29 vs 32.48 dB，画质并非严格等同，
不能把该比例当作所有内容的保证。HEVC 在局部运动样例没有比 H.264 更省字节，
说明“HEVC 固定节省某个百分比”的说法也不成立。

`AverageBitRate` 是平均目标，不是峰值上限：关键帧可能很大，本例的首次完整画面约
0.54 MB，在 4 Mbps 链路单传输就需约 1.1 秒。不能仅设置 4 Mbps 就宣称任何 4 Mbps
公网链路都能立即显示清晰的 1080p 画面。弱网初始画质、关键帧峰值和输入延迟仍需真机
WAN 测试，当前不宣称存在严格的编码峰值约束。

### 本次修正的静态恢复探测

同一张密集画面，6 秒内发送 6 次（首次完整画面 + 5 次每秒探测）：

| 编码 | 原先每次强制关键帧 | 使用帧间参考 | 总数据减少 |
| --- | ---: | ---: | ---: |
| H.264 | 4,194,516 B | 838,875 B | 80.00% |
| HEVC | 3,616,583 B | 728,786 B | 79.85% |

新探测仍产生可解码的真实帧；不是把视频换成空 ping。小探测只提供新鲜的交付/存活证据，
不能证明更高码率一定可持续；实际升级仍发送新画质关键帧，由后续拥塞反馈决定是否维持。
已经通过真实 QUIC + H.264/HEVC 解码测试验证：静态探测保留参考，显式刷新仍给出关键帧。

全部视频记录：[CSV](benchmarks/2026-09-20-screen-compression.csv)。
编码调用耗时包含 IOSurface 拷贝、提交和等待完成，不是独立硬件引擎利用率；
此次运行部分同时存在编译及外部负载，因此不据其耗时评定编码器最大吞吐。

## 是否增加 Zstd、AV1 或动态/静态分区协议

- **不在 relay 压缩视频**：它持有的是密文，没有屏幕内容或编码参考。保持端到端加密。
- **不默认再套 Zstd**：对本例尚未加密的完整 HEVC 流做 zstd level 1，局部/全屏动态
  略微膨胀，滚动仅减少约 0.31%。整段压缩已比实时分包更有利，仍无明显收益；
  [原始对照](benchmarks/2026-09-20-video-zstd.json) 保留 H.264 和其他场景数据。
- **HEVC 继续作为主要选择**，H.264 用于兼容。当前 AV1 编码是软件 rav1e，保持显式
  opt-in；未在这次测试中证明其 1080p 低 CPU/低延迟能力，不能仅为省带宽自动切过去。
- 如果后续重点是低带宽下的文字无损，可以另做 **dirty tiles + 混合编码** 实验：
  静态文字/纯色 tile 用调色板、RLE/Zstd 无损编码，持续运动区域走视频编码，滚动可用
  区域复制；无变化区域不发送。这需要脏区检测、tile 版本/引用、完整刷新、合成顺序和
  解码端实现，且需比较额外 CPU、缓存、光晕/接缝与文字清晰度。**当前没有实现这套
  自定义分区协议**；已有块级动态/静态压缩来自成熟视频编码器，不应混淆两者。

## 部署预算与后续验收

Rust 进程轻量不等于平台没有固定费用。当前 Cloudflare 默认 `standard-2` 为 1 vCPU、
6 GiB；内存/磁盘按运行期间的预分配规格收费，CPU 按实际使用。`basic` 为 1/4 vCPU、
1 GiB，可作为小型部署的验证候选；`lite` 虽只有 256 MiB，但 CPU 仅 1/16 vCPU，
不能根据 relay 内存小就认为足够流畅。现有按需启动、无控制会话后休眠和手动停止机制
仍是 Cloudflare 减少常驻成本的主要手段。本次没有部署或修改任何线上实例。

VPS 可从 1 vCPU / 512 MiB 的单会话测试起步；这是测试起点，不是已验证的最低规格。
QUIC 每连接收发 DATAGRAM 队列各限制 32 KiB；WebSocket 消息、队列和写缓冲也有界。
`max_connections=128` 是准入上限，不保证 128 条活跃视频能共享一颗 CPU。
应保留系统/TLS/网络缓冲余量，在目标 Linux/cgroup 上重新测量。

正式容量验收需要目标 VPS / Cloudflare 规格，至少覆盖 1/4/更多会话、长期待命、
拥塞后的内存回收，以及 30/80/150 ms RTT、不同带宽和 0.5%/2% 丢包的双向链路。
同时记录输入往返 p95/p99、可视画质、CPU、cgroup memory、容器唤醒时间；
不能用 loopback 的吞吐或编码字节数替代这些结果。

## 检查与资料

- relay：17 个测试通过，2 个吞吐 benchmark 默认忽略；本报告使用独立进程 harness。
- host session：17 个测试通过，涵盖本次新增的增量探测解码、显式关键帧，以及已有画质
  恢复、分辨率变化、帧率限制测试。
- relay/host/media-codec 全 target Clippy 通过；Python relay 脚本 9 个通过、1 个本地
  Docker Compose 集成检查跳过。本次没有重跑整个 workspace。
- 初次并行 relay 测试遇到系统 loopback `AddrNotAvailable`；串行复测完整通过，
  没有修改断言、认证行为或超时时间。

资料核对于 2026-09-20：
[Apple temporal compression](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_allowtemporalcompression)、
[Apple keyframe interval](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_maxkeyframeinterval)、
[Cloudflare instance limits](https://developers.cloudflare.com/containers/platform/limits/)、
[Cloudflare pricing](https://developers.cloudflare.com/containers/platform/pricing/)。
