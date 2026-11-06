# 音视频实时性优化：第一批实现与验证

日期：2026-11-07。基于 `66182547530526541c7dbd99243297164f37e939`，修改尚在工作区。对应[审查路线](game-streaming-audit-2026-11-07.md)的音频正确性与局域网显示基础，不是完整游戏串流验收。未提交、发布或替换已安装应用。

## 已实现

### 音频

- 主机完整发送同一次采集中的连续 10 ms 音频块，去掉视频式的“仅保留最新音频”；可靠音频流优先级高于视频、低于控制。
- JitterBuffer 保留源 PTS，以本地单调时钟映射估计包龄，检测连续 seq 下的 PTS 缺口，最多执行 3 个周期的 PLC。重传恢复时跳过过期音频，重新初始化解码状态并通知输出淡化。
- 抖动队列的 `new(60)` 现在确实限制为 6 个周期，原来是 24 个。普通采集允许队列预算加一个播放周期（当前 70 ms）的有限包龄，更长的重传突发仍用 20 ms 追赶期限；包龄不能从相邻回调间距推导。
- 音频到达立即唤醒解码，10 ms 定时器仅维持断流时的补偿；解码按有界批次交付，设备回调负责播放节拍。解码到播放器通道由 128 块降为 6 块，满队列不再阻塞播放时钟。
- 桌面与 iOS 共用预分配的固定块队列和音频渲染器。设备回调中没有互斥锁、日志、堆分配或对 Engine 的访问。常规启动目标为 30 ms；若现有音频估计年龄接近 70 ms，提前播放以免缓冲等待本身触发 80 ms 过期上限。设备稳定水位包含两个周期的余量；对采集批次的水位涨落作补偿，避免把正常批次误判成时钟漂移。
- 速率调整限制为 ±0.2%，处理采集与播放时钟漂移；断点和重新缓冲有短淡化。输出队列上限为 8 块（80 ms），包含最大普通采集批次和设备余量。估计样本年龄超过 80 ms 时丢弃；半块播放时按下一个未播样本计算年龄，避免用块首时间误丢其余样本。这些是内部容量/策略，不是端到端延迟承诺，也不能相加当作实测延迟。
- 桌面请求设备支持范围内约 5 ms 的回调大小，失败自动回退设备默认值，记录实际回调帧数、采样率和系统输出时间戳估计。
- iOS 从 UI/display-link 轮询改为 `AVAudioSourceNode` 按设备需求读取，申请 5 ms IO buffer，并记录实际 AVAudioSession buffer 与输出延迟。独立渲染句柄持有队列，Engine 销毁和重连不会使正在执行的音频回调悬空。

### 视频与呈现

- 同步 VideoToolbox 调用、CPU 缩放和像素处理转移到专用编码线程，有界任务通道容量为 1，避免阻塞 Tokio 的输入、音频和网络任务。
- H.264/HEVC 播放端保留 BGRA CVPixelBuffer，直接交给 Metal texture cache。桌面通过 GPUI 原生 surface 绘制，iOS 通过 CAMetalLayer 绘制。像素缓冲和纹理包装持有到 GPU 完成，解码器销毁后画面仍有效。
- 本批保留 BGRA，由系统解码器完成颜色转换；没有同时引入 NV12 shader 颜色转换变更。软件 AV1、VNC、RDP 保留原 CPU 图像路径，iOS Metal 初始化或纹理导入失败可回退 Core Image。
- 桌面原先每帧 1 ms 的 UI 定时让步改为等待显示帧回调，单槽通道继续合并未消费画面；窗口停止刷新时有 100 ms 有界回退以观察关闭。
- 桌面和 iOS 真机的原生路径使用 drawable 的真实呈现回调统计新帧、重复帧及间隔 p95/p99。离屏绘制不会触发“实际呈现”统计。百分位使用 nearest-rank，避免 60 帧窗口中的一次长停顿被 p99 漏掉。
- iOS 模拟器 SDK 不提供 `addPresentedHandler` / `presentedTime`，模拟器只能记录交付 FPS；CPU 图像路径也是交付口径。不能将其与真机原生路径的实际呈现混用。静止桌面的重复/长间隔也需结合源活动判断。

GPUI Apple 部分固定在原锁定的 0.3.8，只补 BGRA、GPU 资源持有和呈现回调，保留许可证；说明见 [REMOVENT-PATCH.md](../../vendor/gpui-apple/REMOVENT-PATCH.md)。

## 已验证的证据

| 验证 | 结果与边界 |
| --- | --- |
| 生产 JitterBuffer，模拟 120 ms 交付停顿 | 恢复时落后从 110 ms 降为 20 ms，随后 100 个正常周期仍为 20 ms；只表示该队列探针，不是源到声音总延迟 |
| 队列容量与 PTS 缺口 | 突发 25 包后保留 60 ms；连续 seq 的时间缺口产生 PLC |
| 模拟设备时钟 ±150 ppm，各 5 分钟 | 无丢块、无 underrun、队列不持续增长 |
| 普通 20/40/60 ms 采集批次，各 1 分钟模拟设备回调 | 无 underrun、无丢块；另有 JitterBuffer 批次连续性测试 |
| 实际 macOS 音频设备回调，5 秒静音 | 44.1 kHz，220 帧回调（约 5 ms），无 underrun、丢块或断点，系统输出时间戳差约 7.3 ms；未测声学延迟和听感 |
| 实际 H.264/HEVC 硬件编解码 | 原生 BGRA 输出不生成整帧 Vec，解码器关闭后读回颜色与尺寸正确 |
| 实际 GPUI Metal 离屏绘制 | H.264 解码缓冲直接导入纹理，红绿蓝白四象限、方向、透明度验证通过；没有虚报实际呈现回调。[GPU 读回图](../benchmarks/2026-11-07-native-video.png) |
| 实际桌面窗口呈现回调 | 3 秒重绘探针收到 147 次实际 drawable 呈现回调，验证回调接通；包含启动且画面重复，不作为串流 FPS 成绩 |
| 会话与界面回归 | 最新完整回归：桌面 126 项、客户端 126 项、RDP 互操作 5 项、完整会话 21 项、主机 71 项、codec 16 项、codec roundtrip 7 项、mobile Rust 17 项通过；共 389 项，9 项按既有环境条件跳过（Keychain、GUI、外部 VNC fixture、人工性能测试和在线 feed） |
| 静态检查 | `cargo clippy --locked --workspace --all-targets -- -D warnings` 通过 |
| 本地性能测试构建 | 桌面、daemon、CLI 的 locked release 构建通过。首次构建耗时 3 分 56 秒，[首版校验值](../benchmarks/2026-11-07-release-build.json)仅用于追溯；最新本地构建见[最终本地验证记录](../benchmarks/2026-11-07-local-validation-final.json)。首版与第二版诊断 helper 已传到 br-gateway，最后的本地音频修正没有远端部署或复测 |
| iOS 构建 | Debug 模拟器与真机目标均通过，未签名安装或进行真机运行验收。存在原有 videotoolbox iOS 未用符号、Swift 未用返回值和 block future-incompat 提示 |

原始数据：[修改前](../benchmarks/2026-11-07-audio-playout.json)、[修改后](../benchmarks/2026-11-07-audio-playout-after.json)、[实际设备回调](../benchmarks/2026-11-07-audio-device.json)。修改后数据含实际 jitter 源文件 SHA-256。

可复现命令：

```sh
rtk proxy python3 scripts/qa/probe_audio_playout.py
rtk proxy cargo test --locked -p removent-app --bin removent -p removent-client -p removent-host -p removent-media-codec -p removent-mobile --lib --tests -j 4
rtk proxy cargo clippy --locked --workspace --all-targets -j 4 -- -D warnings
rtk proxy cargo run --locked -p removent-app --example native_video_probe -- /tmp/removent-native-video.png
rtk proxy cargo run --locked -p removent-app --example native_video_probe -- --present
rtk proxy cargo run --locked -p removent-app --example audio_output_probe
```

`native_video_probe --present` 是 3 秒本机窗口重绘与实际呈现回调验证，复用同一张测试画面，不能算成真实串流新帧率。`audio_output_probe` 只发送静音，不生成测试音。

## 剩余路线与验收门槛

尚未完成：真实双机 1080p60 连续 30 分钟动态内容验收、输入到画面及声学回环测量、音画同步闭环、弱网和公网矩阵、高刷新率和游戏输入。

编码线程目前仍按帧同步完成并串行发送；本批隔离了 runtime 阻塞，没有实现硬件异步提交与发送重叠，也没有启用未经测量的低延迟码率控制选项。下一步应依据真实帧龄与呈现间隔，推进最多 1–2 帧在途的有界流水线。

媒体仍是可靠 QUIC 流。本批不改线协议，旧端可以继续互通；接收端能丢弃晚到音频，却不能消除可靠流的重传等待。实时 DATAGRAM、MTU 分片、期限、参考帧恢复和能力协商仍需下一批单独实施。不能据此承诺弱网游戏级稳定，也没有新增相对鼠标或手柄支持。

用户指定 br-gateway 为受控端，本机 mac-studio 为控制端。Serlink Dev 重启解锁后 SSH 已恢复。远端为 M4 Mac mini、macOS 27.0.1、4K HDMI 显示且 UI 为 1920×1080 @60 Hz，原安装版本为 0.1.10。使用 Removent 已保存凭据可直接进入桌面；`CGSSessionScreenIsLocked` 标志不能单独作为要求用户解锁的依据。

当前连接使用 `192.168.1.11:48688`。远端路由到本机 `192.168.1.22` 的接口为 USB 有线网卡 en8，本机反向路由为 en0。远端另外存在 Wi-Fi 地址 `192.168.1.119`，但不能因此把本次测试标记为 Wi-Fi。

测试副本保存在两端 `/tmp/removent-gaming-20261107`，使用与原版相同的 Developer ID 签名，远端安装包 SHA-256 校验通过；未替换 `/Applications` 下的应用。原 daemon 已通过 CLI 暂停，临时 helper 已启动。临时路径下的 helper 最初没有继承屏幕录制授权；用户已允许本轮所需权限并完成管理员认证，之后实际动态测试已运行。新一版诊断 helper 已传输并验证，但切换服务的 Serlink 执行审批超时，尚未替换正在运行的第一版测试进程。用户随后明确选择保留当前远端服务，本轮不切换或恢复 daemon。保存连接的 4K 画质上限已恢复。

后续恢复双机测试时应再次核对刷新率、音频输出设备和网络路径，再按原审查中的 1080p60 指标采集。达到该门槛之后，再进入 120 FPS 与弱网传输阶段。跨设备 PTS 不直接相减；当前呈现间隔和本地音频时钟映射不等于已获得精确的跨设备采集到呈现延迟。

## 动态测试夹具

`scripts/fixtures/streaming_fixture.swift` 使用 MTKView 生成持续变化的网格与运动图形，按源显示器请求 60 FPS，并以实际 drawable 呈现回调输出每 5 秒的新帧数量、间隔 p95/p99/最大值。可选 `--audio` 输出低幅度 440 Hz 连续测试音。默认 180 秒后自动退出；该夹具用于区分源端掉帧与传输/接收端掉帧，不测量跨设备或声学端到端延迟。已通过本机 Swift 优化构建并签名，并在 br-gateway 完成 180 秒动态画面及低幅度测试音运行。

控制端实际为 M5 Max，DELL G3223Q 3840×2160，UI 1920×1080 @144 Hz。源端 60 Hz 与接收显示 144 Hz 并非整数倍，呈现间隔须结合这个刷新率组合解释。

## 首次双机测试发现与修正

第一轮源端实际呈现稳定约 60 FPS、间隔约 16.667 ms；控制端 143 个采样窗口平均新帧率约 56.21 FPS，多数窗口约 57 FPS，最差间隔 618.2 ms。音频 underrun、PLC 和误丢持续增加。本轮同时存在本机编译和 SFTP 界面操作，不能将最差停顿直接归因于传输，也不能视为无负载稳定性成绩。该轮未通过游戏级验收。

原始证据：[接收端指标](../benchmarks/2026-11-07-br-gateway-first-pass.log)、[源端实际呈现](../benchmarks/2026-11-07-br-gateway-source-first-pass.jsonl)。没有使用静态桌面重复刷新冒充 60 FPS。

进一步记录了 2,000 个连续音频包的相对到达时间，源 PTS 始终每 10 ms 递增。实际采集常按 1024 / 48000 秒（约 21.333 ms）分批交付；独立的播放时钟使原来的期限与 PLC 时机过早，正常包被持续误丢。现将这份仅含相对时序的 CSV 加入回归，同时模拟包到达、解码唤醒、独立 10 ms 补偿时钟和 5 ms 设备回调。最终本地修正覆盖四种解码相位和 44.1/48 kHz 两种输出，共 8 种组合，均无包丢失、PLC 或输出过期；允许源启动延迟变化导致的一次重新缓冲，随后约 18 秒不再 underrun。该模拟仍不是声学延迟测量。

本轮额外修正了已播放半块的样本年龄判断，并增加到包立即唤醒解码。对 120 ms 重传停顿的原有探针仍能追赶到 20 ms，队列不持续增长。修正后的实际双机动态复测尚待完成；不能用这份回归替代 30 分钟实机验收。

修正版客户端已连接第一版测试 host，完成静态桌面下的实际音频观察。连续 155.17 秒的指标窗口内，`late_dropped=0`、`plc_frames=0`、`dropped_output=0`；设备端 underrun 和丢块各从启动后的 1 增至 2。因此持续误丢已消除，但仍有一次额外重新缓冲，未达到零 underrun 标准。该轮没有运行动态夹具，也未测声音延迟；不能替代动态负载复测。[实际音频指标](../benchmarks/2026-11-07-br-gateway-audio-second-pass.log)。

当前安排：用户已明确选择“先完成本地验证，保留当前远端服务”，不再重发服务切换审批。按上一轮最后确认，第一版临时 host 的 PID 为 39965；本轮没有重新连接远端，也没有改变其进程或文件。第二版诊断 helper 暂存于远端 `next/RemoventHost.app`。本地测试客户端已退出，原安装版本已重新打开，保存连接的画质上限已恢复为 4K。恢复正式 daemon 或切换新副本留待后续明确安排。

## 本地验证收尾

用户选择保留远端服务后，补查了实际 44.1 kHz / 220 帧输出设备对应的非整数回调间隔（约 4.989 ms）。原先仅以 48 kHz / 240 帧模拟，会遗漏重新缓冲期间的边界：已经延迟的音频继续等待 30 ms，可能在开始播放前超过 80 ms 期限。已修正为接近期限时提前播放，没有扩大过期上限、队列容量或最大速率偏移。

真实到包回归现在覆盖 8 种相位与输出采样率组合；20/40/60 ms 普通采集批次也覆盖两种输出采样率。相关模拟回归保持有限队列，并检查晚包、PLC、过期丢块和稳定阶段 underrun。这次修正仅完成本地验证，不能据此声称已经解决双机观测到的所有偶发断音。

最终本地验证的命令、结果、源码与二进制 SHA-256 见[机器可读记录](../benchmarks/2026-11-07-local-validation-final.json)。远端动态帧率与零断音验收继续标为未通过；没有提交、推送、发布或替换已安装应用。
