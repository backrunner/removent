# 公网自适应画质：先保文字清晰

当前 v1 的直连、QUIC relay、Cloudflare WSS relay 共用同一条端到端媒体反馈链路。
relay 仍然只转发加密数据，不解码、不转码、不参与双方的清晰度判定。

## 已实现

- 接收端每 500 ms 自动发送 `StatsReport`，包括实际视频吞吐、到达间隔相对源时间戳的
  抖动、部分帧收取停顿、提交到解码完成的耗时。没有帧活动时不回报虚假的健康证据。
  待解码记录最多 32 条，遥测满队列直接跳过，不能排在输入前等待发送。
- 被控端每 250 ms 合并接收反馈和本地写入压力/QUIC 丢包；拒绝无效数字，接收反馈
  750 ms 后过期。静态屏幕的低吞吐和正常的跨地域高 RTT 不单独触发降档。
- 默认保持 **协商后的捕获尺寸**，不再自动缩到 75%/50%。当前原生捕获仍受既有
  1920×1080 包围框限制，这不是新增 4K 原生像素支持；用户显示器缩放和客户端缩放
  也会影响文字大小。原有尺寸热切换、`FrameGeometry` 与输入顺序约束仍保留。
- 每帧最低目标预算为 `ceil(width × height / 100) × 8` bit（约 0.08 bit/像素/帧）。
  初始码率负担不了请求帧率时先减少帧率；拥塞时先按比例降 fps/码率，保留每帧预算。
  15 fps 以下才在预算内继续降码率/帧率。默认最低 2 fps，协商上限更低则遵守上限。
  1080p 预算对应最低 332 kbps；720p 为 148 kbps，小画面另有 64 kbps 下限。
- H.264/HEVC 尝试设置 VideoToolbox `MaxAllowedFrameQP=28`，AV1 显式设置 rav1e
  bitrate 模式的 quantizer 上限 100（不同编码器的量化刻度不能直接比较）。硬件不支持
  可选 QP 属性时记录警告，保留帧预算保护。编码器因质量限制主动跳帧会触发降档，
  不按普通编码错误计数；持续 10 秒无输出仍明确结束会话，避免永久黑屏。
- 至少间隔 2 秒调档；连续 5 秒丢包 <0.5%、抖动 <10 ms、解码与发送健康后，码率
  每次提高 25%，先恢复每帧细节预算，再恢复刷新率。丢包 >2%、抖动 >20 ms 或解码
  超过 `max(40 ms, 当前帧间隔)` 触发降档。显示模式变化会重新计算预算。
- 降档后静态画面仍用低频帧间压缩探测恢复；不把缓存帧的 +1 µs 时间戳当网络抖动。
  真正质量切换刷新关键帧；输入与剪贴板的独立流、优先级和最新帧槽保持原有约束。

这些是可测试的工程下限，不是任意内容的感知保证。极小字号、彩色细线、满屏运动、
不支持 QP 限制的硬件，以及低于一帧预算的用户码率上限，都不能只靠这两个常数证明
可读性。低于可持续带宽时优先等候清晰画面，刷新变慢，不能承诺弱网没有操作影响。

## 真实编解码与文字样本

Apple M4 / macOS 27，release 构建；合成 1920×1080 明暗两栏界面，包含中文设置、
英文域名、代码、符号及 10/12/14/16/20/22 px 字体，底部 140 行为局部动态区域。
不采集开发机桌面。每档提交 30 帧，通过生产 VideoToolbox 编解码后保存首帧及最后一帧。

| 编码器 | 目标码率 / fps | 接受 QP 上限 | 输出 / 提交 | 实际 payload 码率 | 首帧字节 |
| --- | ---: | --- | ---: | ---: | ---: |
| H.264 | 8000 kbps / 30 | 是 | 30/30 | 916.02 kbps | 47,112 |
| H.264 | 2000 kbps / 12 | 是 | 30/30 | 612.60 kbps | 45,135 |
| H.264 | 332 kbps / 2 | 是 | 30/30 | 296.20 kbps | 45,135 |
| HEVC | 8000 kbps / 30 | 是 | 30/30 | 827.63 kbps | 50,866 |
| HEVC | 2000 kbps / 12 | 是 | 30/30 | 486.83 kbps | 49,457 |
| HEVC | 332 kbps / 2 | 是 | 30/30 | 268.35 kbps | 49,457 |

码率分母是源时间轴（30/fps 秒），不是本机运行时间；不包含加密、ACK、重传和音频。
这是带局部运动的稀疏 UI，不能外推满屏视频、照片或密集文档。编码调用 p95 为
6.33–7.14 ms，包含一次同步提交/取包；它不等同于捕获到屏幕反馈延迟，也不是 CPU 使用率。

Apple Vision OCR（中文优先、自动语言检测）在原图以及两种编码器最低档的首帧/最后一帧中，
均保留了明暗两栏的两条中文说明、域名行与代码行，共 **8/8 条精确匹配**。
这不计标题、菜单、符号和小字号：OCR 在未压缩原图的这些项目上也会误识别。
已人工检查最低档 HEVC 解码图，主要界面文字清楚；这仍不是所有用户字号的视觉验收。
静态区域 RGB PSNR 在最低档为 H.264 46.98 dB、HEVC 49.01 dB；背景面积较大，
该指标仅作像素回环检查，不作为可读性判据。

**首帧代价：**最低档首帧仍有约 45–49 KB。在 332 kbps 净带宽下仅序列化就约
1.09–1.19 秒，另加 RTT、编码、解码与丢包等待。2 fps 表示刷新目标，不承诺首帧 500 ms
内到达。低延迟输入可以先到达，但画面变化的可见反馈仍受下行媒体限制。

原始证据：[CSV](benchmarks/2026-09-20-readability/codec.csv)、
[OCR](benchmarks/2026-09-20-readability/ocr.jsonl)、
[环境](benchmarks/2026-09-20-readability/environment.json)、
[原图](benchmarks/2026-09-20-readability/source.png)、
[H.264 首帧](benchmarks/2026-09-20-readability/H264-332-2-first.png)、
[HEVC 首帧](benchmarks/2026-09-20-readability/Hevc-332-2-first.png)、
[H.264 最后帧](benchmarks/2026-09-20-readability/H264-332-2.png)、
[HEVC 最后帧](benchmarks/2026-09-20-readability/Hevc-332-2.png)。

## 回归验证

- core/client/host/media-codec 的库及集成测试 **200 通过、3 项显式跳过**。
  跳过项为 Keychain 集成、外部 libvncserver fixture 和实际 GUI launchd 生命周期。
- 新端到端测试在真实 QUIC 连接中保持视频头不完整，由生产接收器自动发送反馈，
  驱动被控端降档；不注入伪造 `StatsReport`，不提供本地 delivery 健康证据。
  视频未完成时按下/松开仍到达；随后真实 HEVC 解码与健康接收允许恢复，尺寸始终不变。
- 控制器测试覆盖预算下限、手动上限、慢解码、无效/过期反馈、静态画面、高 RTT、
  显示模式变化和恢复滞回。已有静态刷新、编码器热切换、输入坐标顺序继续通过。
- 受影响的 5 个 crate（含 app）`clippy --all-targets -- -D warnings` 通过。
- 一轮 VNC 回归暴露了测试中的调度竞态：发送队列排空并不意味着独立接收任务已读取
  首字节。改为在原有 2 秒期限内同时等待两个事实；仍要求剩余 33 MB 未发送时完成
  500 对按下/松开。没有修改生产 VNC 行为或放宽时限，修正后完整相关测试通过。

本次没有公网/Cloudflare 实例验收，也没有锁屏或注销当前 Mac。macOS 无人值守验收边界
仍见 [解锁文档](macos-unlock.md)。网络损伤工具的测量范围见 [延迟报告](wan-latency-2026-09-20.md)。

## 传输延迟复测

更换控制器后，使用既有 `wan_latency` 逐条复测 QUIC relay 与 WebSocket relay，各场景
10 秒。该工具走真实 relay 认证和 QUIC 控制泵，但媒体是合成字节负载，不包含本次
接收器解码/OCR；它用于检查输入调度回归，不替代上面的真实编解码和端到端反馈测试。
UDP 为每段随机丢包模型，TCP 为每段周期性停顿模型，均为本地代理，不是实际公网。

| 路径 / 场景 | 输入到达 p95 | 控制 RTT p95 | 超过 2 秒/未返回 Pong |
| --- | ---: | ---: | ---: |
| QUIC / 80 ms RTT、8 Mbps，视频过载 | 44.50 ms | 170.92 ms | 0/101 |
| QUIC / 80 ms RTT、8 Mbps，每段 2% 丢包 | 138.48 ms | 272.16 ms | 0/101 |
| QUIC / 150 ms RTT、2 Mbps，每段 5% 丢包 | 387.01 ms | 1370.16 ms | 0/101 |
| WebSocket / 80 ms RTT、8 Mbps，视频过载 | 44.06 ms | 181.00 ms | 0/101 |
| WebSocket / 80 ms RTT、8 Mbps，周期停顿 200 ms | 224.98 ms | 501.68 ms | 0/101 |
| WebSocket / 150 ms RTT、2 Mbps，周期停顿 300 ms | 335.31 ms | 1489.92 ms | 0/101 |

包括两个空闲对照在内，共 808 对按下/松开全部在 teardown 前送达；独立调度探针最大
迟到 3.13–7.71 ms。单次短样本不能用来断言相对旧控制器的延迟提升；重度弱网仍明显
影响操作。WebSocket 过载场景本次 10 秒内仍维持原目标码率，说明仅靠发送写入健康
不能完整反映接收端体验；生产链路新增的接收停顿反馈另由上述端到端测试覆盖。
这里的输入落点是内存 InputSink，数值不是按键到真实 Mac 屏幕反馈的耗时。

原始 CSV：[QUIC relay](benchmarks/2026-09-20-readable-wan-quic.csv)、
[WebSocket relay](benchmarks/2026-09-20-readable-wan-websocket.csv)。

## 复现

```bash
swift scripts/readability_fixture.swift generate /tmp/removent-readability
cargo run --locked --release -p removent-media-codec --example readable_quality -- /tmp/removent-readability
# 用 sips 将待检查的 PPM 转为 PNG（保留原始像素）。
sips -s format png /tmp/removent-readability/Hevc-332-2-first.ppm --out /tmp/removent-readability/Hevc-332-2-first.png
swift scripts/readability_fixture.swift recognize /tmp/removent-readability/source.png /tmp/removent-readability/Hevc-332-2-first.png
cargo test --locked -p removent-core -p removent-client -p removent-host -p removent-media-codec --lib --tests -j 2 -- --test-threads=1
cargo clippy --locked -p removent-core -p removent-client -p removent-host -p removent-media-codec -p removent-app --all-targets -j 2 -- -D warnings
cargo build --locked --release -p removent-host --example wan_latency -j 2
target/release/examples/wan_latency quic 10
target/release/examples/wan_latency websocket 10
```

VideoToolbox 属性行为依据本机 SDK 的 `VTCompressionProperties.h`：QP 上限为可选项，
编码器可通过跳帧满足质量要求；AV1 上限语义核对 rav1e 0.8.1 的 `ContextInner::new`
和 rate control 实现。所有量化值都仍需按真实屏幕内容和目标硬件继续校准。
