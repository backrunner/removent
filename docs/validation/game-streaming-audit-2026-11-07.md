# 游戏串流可行性与音视频性能审查

日期：2026-11-07。源码基线：`66182547530526541c7dbd99243297164f37e939`，工作区开始时干净。

结论：可以继续向稳定的游戏串流推进，建议先验收有线局域网 1080p60，再推进 1440p60、1080p120 和可控公网路径。当前有足够的硬件编码、最新帧通道和自适应基础，但还没有证据证明实际显示平滑、音频低延迟或竞技游戏体验。主要工作是播放时间线、帧呈现、媒体期限和有界调度，不是单纯提高 FPS 上限或更换编码格式。

本记录保留优化前的源码审查、官方资料核对和生产音频队列行为探针。审查阶段没有修改生产链路，也没有运行完整构建、真实采集、声卡输出、双机网络或游戏测试。用户确认路线后完成的第一批改动及验证，另见[实现记录](game-streaming-implementation-2026-11-07.md)，不要把下面的基线问题当作修改后的状态。

## 已经具备的基础

- [采集](../../packages/media-capture/src/sck.rs)保留原生 CVPixelBuffer，ScreenCaptureKit queue depth 为 3；[编码输入](../../packages/media-codec/src/video/encoder/native.rs)在原尺寸时直接使用原生 buffer，缩放走 Metal，并有 CPU 回退。
- [视频发送](../../packages/host/src/session/video.rs)和解码交付采用最新帧通道，限制过期原始帧积压。现有发送节拍已把编码、写入耗时计入帧预算，不能再次误判为“每帧工作之后还固定多睡一个帧周期”。
- [编码器](../../packages/media-codec/src/video/encoder.rs)优先硬件，启用 RealTime、禁用帧重排、优先编码速度。HEVC 是常规首选，软件 AV1 已明确限制为主动选择。
- [动态质量](../../packages/core/src/adapt.rs)优先减少像素，再降低帧率；协议上限已经是 240 FPS，实际受主机显示刷新率、协商、编码和接收性能限制。
- [网络](../../packages/net/src/transport.rs)已有 BBR，[控制写入](../../packages/net/src/control_writer.rs)优先于媒体，剪贴板独立；[发送预算](../../packages/host/src/delivery.rs)随码率和基础 RTT 调整。这些能力应保留。

## 确认的问题与优化位置

### 1. 音频交付恢复后可能长期落后实时位置

[接收与播放](../../packages/client/src/session/media.rs)每 10 ms 从 JitterBuffer 弹出一个包，定时器使用 `MissedTickBehavior::Delay`。空队列返回 Wait，没有基于播放期限推进的缺包隐藏；包内 PTS 没有参与播放调度，解码后只传递 `Vec<i16>`。

[JitterBuffer](../../packages/client/src/jitter.rs)是有序包队列，不是按目标延迟工作的自适应抖动缓冲。`new(60)` 的实际防洪上限是 24 个包，即 240 ms，不能把这个参数解释成 60 ms 延迟保证。

本轮直接编译当前 `jitter.rs`，以模拟时钟重现生产端每 10 ms 弹一包的调用方式：

| 情况 | 观察 |
| --- | --- |
| 交付停顿后在第 120 ms 补齐包 | 首个恢复播放的包落后模拟源时间 110 ms |
| 随后正常供包、播放 100 个周期 | 仍落后 110 ms，剩余队列 110 ms |
| `new(60)` 一次收到 25 个包 | 保留 240 ms，淘汰 1 个包 |
| 连续 seq 的两个包，PTS 相差 40 ms | 连续弹出两个包，不根据时间戳产生 PLC |

这是队列和模拟节拍的行为证据，不是实机总延迟或声音质量测量。原始结果：[audio-playout.json](../benchmarks/2026-11-07-audio-playout.json)。复现：

```sh
rtk proxy python3 scripts/qa/probe_audio_playout.py
```

### 2. 主机丢弃音频的方式会破坏连续采样

[音频发送](../../packages/host/src/session/audio.rs)收到一帧后，调用 `newest_queued` 丢掉队列中更早的音频，只编码最后一帧。[采集回调](../../packages/media-capture/src/sck.rs)允许一个回调产出多个连续的 10 ms 帧，因此批量到达、调度抖动或网络写入阻塞都会触发这种合并。

音频帧被丢弃后，Opus seq 仍按实际编码次数连续增长，接收端又不使用 PTS 检查源时间缺口，所以这些缺失不会自然变成 PLC。视频的“只取最新画面”策略不能直接用于连续音频。是否以及多频繁发生，需要记录采集批量大小、实际丢弃样本数和输出 underrun；本轮没有把这种条件性问题当作每次会话都发生。

修复方向：正常批量必须保持采样连续；真正超期时按时间线明确跳过并执行短交叉淡化或受控重置，统计被丢弃的时长，避免无声地拼接不连续音频。

### 3. 音频缓冲和输出时钟没有形成完整闭环

| 层 | 当前实现 | 含义 |
| --- | --- | --- |
| 主机采集通道 | 32 个 10 ms 帧 | 容量 320 ms，发送端另有上述合并逻辑 |
| 客户端解码 PCM 通道 | 128 个块 | 标准块为 10 ms 时容量 1.28 s |
| 桌面播放器 | 9,600 个采样帧 | 48 kHz 下最多排队 200 ms |
| iOS Rust 到 Swift 桥接 | 12 个块 | 容量 120 ms |
| iOS AVAudioPlayerNode | 12 个块后 stop/play | 约 120 ms 阈值，重启可能打断连续性 |

以上是代码容量，不是正常实测延迟，也不能简单相加作为总延迟。

[桌面输出](../../apps/desktop/src/audio.rs)使用 Mutex 共享 VecDeque，回调内锁定、逐采样消费，并允许 scratch resize；采样率转换只有固定比例，没有按队列水位修正源时钟与设备时钟漂移。[iOS](../../apps/mobile/Sources/ControllerModel.swift)在主线程 display-link 回调里拉取并投递音频，容易把 UI 停顿传到声音；[RemoteAudio](../../apps/mobile/Sources/RemoteAudio.swift)虽然已申请 10 ms IO buffer，但请求值不等于实际硬件延迟。

修复方向：保留 PCM 时间戳和 discontinuity，采用设备消费时钟、预分配单生产者单消费者环形队列、自适应目标水位和微量速率校正。短缺口按期限 PLC，长缺口逐渐静音并重新定锚；恢复后不能永久保留旧延迟。iOS 音频投递脱离 UI 轮询。

Opus 包长目前已经是 10 ms。LowDelay 和 5 ms 可以做对照实验，但应先修复时间线；实际算法延迟应读取 `OPUS_GET_LOOKAHEAD`，不能只用包长推断。LowDelay 会限制编码模式，FEC 能力需按实际模式验证，不能默认“低延迟模式加 FEC”总能成立。[Opus 官方 CTL 文档](https://www.opus-codec.org/docs/opus_api-1.3.1/group__opus__encoderctls.html)

### 4. 视频每帧同步等待编码完成，编码和发送串行

[EncoderSession::encode_buffer](../../packages/media-codec/src/video/encoder.rs)每帧提交后调用 `VTCompressionSessionCompleteFrames(INVALID)`，再阻塞 `recv()`；[发送任务](../../packages/host/src/session/video.rs)直接在 Tokio 任务中调用它，随后 await 网络写入。这限制了硬件回调与发送的重叠，也会占用 runtime worker。具体性能收益还需测量，不能从源码直接推导能提升多少 FPS。

修复方向：编码会话由专用 worker 管理，完成回调通知有界发送通道，最多保留 1–2 帧在途并测量真实年龄。只覆盖尚未编码的原始帧；已经编码的参考帧不能随意丢弃。保留 generation、重建关键帧、取消和回调资源寿命保证，不能把同步等待简单替换成无限异步提交。

当前创建字典只要求硬件编码，没有启用专门的 `EnableLowLatencyRateControl`。Apple 提供了低延迟编码选择及其约束，值得做能力探测和 H.264 对照；不能假定默认 HEVC、所有硬件和 macOS 版本均支持同样行为。它会影响 GOP、帧类型和码率控制，必须同时验收关键帧恢复和画质。[Apple 官方说明](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablelowlatencyratecontrol)

### 5. 客户端视频仍需要 CPU 帧复制和图像上传

[解码](../../packages/media-codec/src/video/decoder.rs)回调把 CVPixelBuffer 转成 `Vec<u8>`。[像素转换](../../packages/media-codec/src/video/pixels.rs)即使拿到 BGRA，也逐行复制整幅图像。[桌面](../../apps/desktop/src/ui/viewer/lifecycle.rs)逐帧创建、替换并释放 RenderImage；[iOS](../../apps/mobile/Sources/ControllerModel.swift)从 CPU 字节构造 CGImage，再交给 UIImageView。

3840 × 2160 × 4 × 60 对应每秒约 1.99 GB 的单份 BGRA 像素数据量；复制读写、上传和分配另有成本。这是尺寸计算，不是实测内存带宽瓶颈，也不代表独显 PCIe 传输。

修复方向：把保留引用的 CVPixelBuffer/IOSurface 送到原生视频呈现层，使用 Metal texture cache，尽量保持 NV12 到 shader；跨 Rust/Swift/GPUI 生命周期、颜色矩阵、range、stride、旋转和缩放都要验证。保留 CPU 回退并记录是否命中，避免把正常截图、VNC 或测试入口破坏。

### 6. 稳定平均 FPS 尚不等于稳定上屏

桌面 `fps_counter` 在 `handle_frame` 增长，反映界面收到帧；客户端 StatsReport 明确发送 `render_fps: 0`。iOS 已用 CADisplayLink，但统计的是取帧，不是最终显示。当前没有按源 PTS、显示刷新相位和实际呈现完成组成的帧时间线。这不代表底层 UI 框架完全没有垂直同步。

修复方向：记录源 frame ID 到实际 present 的对应关系、重复帧、丢帧、帧龄及呈现间隔 p50/p95/p99。以显示时钟选择最新可用帧，迟到帧跳过显示，但不能破坏解码参考；小幅有界缓冲只用于吸收测得的 jitter，不固定增加多帧延迟。Apple 的 `presentedTime`/呈现回调可提供实际 drawable 上屏时间；真实显示测试仍应包含外部测量。[Apple 官方文档](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime)

### 7. 可靠媒体流没有按帧期限放弃重传

当前音视频各用独立的可靠 QUIC uni-stream。两者之间没有单个 QUIC 流的字节顺序依赖，但各自会因缺失字节等待重传，并共享拥塞和发送预算。音频与视频使用相同媒体优先级；控制更高的优先级已经存在。

原生 QUIC relay 虽然用 DATAGRAM 搬运内层报文，内层 RVP 媒体仍然是可靠流。不能把“relay 使用 DATAGRAM”当成媒体已经支持丢弃过期帧。WSS 外层还会带来 TCP 层的阻塞。

建议通过新能力位协商实时媒体通道，逐步支持 QUIC DATAGRAM。DATAGRAM 不要求传输层重传，仍必须服从拥塞控制；它本身不提供媒体期限、纠错、分片重组或公平调度，这些需要应用实现。[RFC 9221](https://www.rfc-editor.org/rfc/rfc9221.html)

- 音频：独立调度预算、包序号、时间戳、播放期限、晚到丢弃和 PLC；按实际 Opus 模式评估 FEC 或有限冗余。
- 视频：按 path MTU 分片，有界重组和帧 deadline，按预算做 FEC/有限重传。明确参考依赖；丢失参考后请求可独立解码帧，并抑制关键帧风暴。不能仅把 `write_all` 改成 `send_datagram`。
- 控制、认证和必须完整交付的数据继续可靠传输；音频优先于视频，避免大关键帧或数据报淹没输入和声音。
- 旧端继续协商可靠媒体流；新旧端、重连、relay、WSS 回退都要覆盖。WSS 保留兼容接入定位，不承诺重传停顿下的游戏稳定性。

### 8. 游戏输入仍有功能缺口

[协议](../../packages/proto/src/msg.rs)和实际注入当前以绝对鼠标位置、键盘与滚轮为主；`RELATIVE_POINTER` 只有能力常量，没有找到完整相对移动传输及消费路径，也没有手柄事件链路。自由视角 FPS 需要相对鼠标、指针捕获和可靠松开；手柄支持需单独设计和实机验证。即使视频变快，也不能因此宣称已完整支持游戏。

## 建议实施顺序与验收

1. **测量与音频正确性。** 增加采集批量、被丢样本、每层队列时长、underrun、PLC、实际设备 buffer 和帧时间线；修复批量音频合并、PTS 丢失和停顿后不追实时。先用现有可靠流验证，降低变量数量。
2. **稳定 LAN 显示。** 专用编码 worker、有界回调、GPU 解码输出到显示、刷新时钟调度，完成真实 1080p60 连续动态内容验收。测量后再调整采集 queue depth，不能盲目把 3 改成 1。
3. **实时媒体协议。** 独立能力协商、小包/分片期限、依赖恢复、音频优先级和弱网回归。质量控制分成快速排队保护和慢速质量调整；当前 500 ms 反馈、1 s 持续压力、2 s 最小变更间隔对避免振荡有价值，但不足以承担几十毫秒媒体期限。
4. **高刷新率与游戏输入。** 1080p120、1440p60/120 单独验收，再补相对鼠标和手柄；软件 AV1 不作为首轮提帧手段。高复杂运动时评估当前 QP 上限导致的编码器主动丢帧，保持画质和 FPS 的明确取舍。

以下是建议的首轮工程目标，均不是当前实测成绩或任意网络承诺。条件：有线 LAN、RTT ≤ 5 ms、持续动态内容、60 Hz 显示、扬声器或有线音频设备，连续 30 分钟：

| 指标 | 首轮建议目标 |
| --- | --- |
| 实际新画面呈现 | 平均 ≥ 59 FPS，同时报告每秒分布 |
| 新画面间隔 | p95 ≤ 20 ms，p99 ≤ 33.4 ms，无非预期 > 100 ms 冻结 |
| 采集到呈现 | p95 ≤ 50 ms，p99 ≤ 65 ms |
| 输入到可见响应 | 同一固定测试场景 p95 ≤ 80 ms，包含远端渲染 |
| 源音频到设备输出 | p95 ≤ 50 ms，实际输出设备延迟计入 |
| 稳态音画偏差 | 绝对值 ≤ 20 ms，并记录长时间漂移 |
| 注入 100 ms 交付停顿 | 恢复正常后 ≤ 250 ms 回到目标播放水位，无永久额外延迟 |
| 30 分钟音频 | 无正常链路下的 underrun、意外静音或不断增长的队列 |

120 Hz 应使用对应的 8.33 ms 帧预算，不能沿用 60 Hz 的平均 FPS 验收。实际输入到画面还受游戏自身帧率影响；上述结果也不等于竞技级保证。

验证矩阵需要分别覆盖：直连 LAN、Wi-Fi 抖动、原生 QUIC relay、公网 RTT 20/40/80 ms、随机与突发丢包、带宽骤降、WSS 停顿；内容包含高速转向、全屏复杂运动、滚动文字、场景切换及同步闪光/音频脉冲。每个场景记录码率、CPU/GPU、温度、重复/丢帧、重传、关键帧、音频水位和感知质量，不能用简单移动标记替代复杂游戏画面。

跨设备的 PTS 不能直接相减：需要单调时钟映射、漂移估计和测量误差范围。输入到光学反馈及音画同步用高速摄像/光学和音频回环交叉验证。现有 [WAN 验证](wan-latency-2026-09-20.md)使用模拟媒体和 mock 输入，不包含真实采集、解码、显示或音频，属于历史局部证据，不能复用为本轮端到端验收。
