# Removent 技术架构与模块设计

版本：v1.0（2026-08-22） · 上游文档：[requirements.md](./requirements.md) · 下游：[protocol.md](./protocol.md)

## 1. 总览

单二进制 `removent`（GUI 应用），同时承担**控制端**与**被控端**两种角色。
传输层为 QUIC（UDP 48688，端口常量以 protocol.md 为准），其上运行自研 RVP 协议；
媒体管线基于 macOS 系统框架（ScreenCaptureKit / VideoToolbox / CoreAudio）+ Opus；
UI 为 GPUI + gpui-component。

```
┌────────────────────────── removent.app ───────────────────────────┐
│  UI 层 (GPUI + gpui-component)                                     │
│   Home / Viewer 会话窗 / Host 面板 / 设置 / 配对弹窗 / 菜单栏        │
├────────────────────┬───────────────────────────────────────────────┤
│  client 会话引擎    │  host 服务引擎                                 │
│  解码/渲染/输入采集  │  准入/捕获/编码/注入                           │
│  剪贴板/自适应控制   │  剪贴板/边缘提示条                             │
├────────────────────┴───────────────────────────────────────────────┤
│  core: 会话编排、类型、配置、身份、存储(userdata/)、日志             │
├─────────────────────────────────────────────────────────────────────┤
│  proto (RVP 消息与帧格式)      net (QUIC/mDNS/配对)                 │
├─────────────────────────────────────────────────────────────────────┤
│  media-capture (SCK)   media-codec (VideoToolbox/Opus)   input(CGE) │
└─────────────────────────────────────────────────────────────────────┘
```

## 2. Cargo Workspace 划分

| crate | 职责 | 关键依赖 | 备注 |
| --- | --- | --- | --- |
| `removent-app` | GPUI 界面、菜单栏、窗口管理；`main()` 入口 | gpui, gpui-component, removent-client/host/core | 仅 UI，不含业务逻辑 |
| `removent-client` | 控制端会话引擎：视频接收解码、音频播放、输入采集转发、剪贴板同步(本侧)、自适应控制、重连状态机 | removent-proto/net/media-codec/core | 可被 headless 测试驱动 |
| `removent-host` | 被控端服务引擎：准入策略、捕获编码管线、输入注入、剪贴板同步(本侧)、会话提示条 | removent-proto/net/capture/codec/input/core | 同上 |
| `removent-core` | 公共类型/错误、配置读写、身份密钥管理、`userdata/` 存储、日志初始化、i18n 资源 | tokio, tracing, serde, rcgen | 无 UI 无协议细节 |
| `removent-proto` | RVP 全部消息定义、postcard 序列化、媒体帧头打包/解析、版本协商 | postcard, bytes | 纯函数库，fuzz 重点 |
| `removent-net` | QUIC endpoint 封装、mDNS 发现/广播、SPAKE2 配对、证书互 pin、连接生命周期 | quinn, mdns-sd, rustls, rcgen, spake2 | tokio 运行时在此 crate 内启动 |
| `removent-media-capture` | ScreenCaptureKit 屏幕帧 + 系统音频采样回调，转成统一 `Frame`/`Pcm` 类型 | screencapturekit | macOS-only feature |
| `removent-media-codec` | 编解码封装：VTCompressionSession/VTDecompressionSession（HEVC/H264）、Opus enc/dec | videotoolbox 或 objc2-video-toolbox, opus | trait 抽象隔离平台 |
| `removent-input` | 键鼠事件模型、CGEvent 注入、NSPasteboard 读写、光标形状抓取 | objc2, objc2-app-kit, core-graphics | macOS-only |
| `removent-daemon` | 无头被控服务端 `removentd`：常驻 accept 循环（host::serve_forever）、UDS IPC 服务、launchd 托管 | removent-host/core, tokio | 唯一服务端进程，flock 防多实例（.daemon.lock） |
| `RemoventTray`（tray/） | Swift 菜单栏管理端：状态/会话/PIN/准入审批/开关/自启 | AppKit（SwiftPM） | 经 core::ipc JSON Lines 与 daemon 通信 |

依赖方向严格单向：`app → client → core → {proto, net, media-*, input}`，`daemon → host → ...`（app 不再直接依赖 host/capture/codec）；
`proto` 不依赖任何 IO 库（可独立 fuzz/测试）；平台绑定只出现在 `media-*` 与 `input`；`daemon` 只依赖 host/core，tray 为独立 SwiftPM 包（不进 Cargo workspace）。

## 3. 媒体数据流

### 3.1 被控端（发送）

```
SCStream(SCK) ──CMSampleBuffer──▶ capture 线程队列
                                    │ 只保留最新一帧（丢帧策略）
                                    ▼
                        VTCompressionSession (硬编 HEVC/H264, RealTime)
                                    │ 编码完成回调 → Annex-B NALU + 关键帧标志
                                    ▼
                        帧打包(proto::video_frame_header)
                                    │ try_send 有界通道(容量2, 满→丢非关键帧)
                                    ▼
                        QUIC uni-stream 写任务（背压监测见 §5.3）
音频：
SCStream audio ──PCM 48k f32 stereo──▶ 重采样/整形 ─▶ Opus 10ms 帧 ─▶ 同上(独立 uni-stream)
```

要点：
- **零拷贝优先**：SCK 给出的 IOSurface/CVPixelBuffer 直接喂给 VT（同进程内无需转 RGBA）；仅在需要缩放时插入 VTPixelTransferSession。
- **排除自身窗口**：SCContentFilter 排除本 app 的窗口，避免"套娃画面"；音频设置 `excludesCurrentProcessAudio`。
- **静态画面零负载**：SCK 本身只在内容变化时回调；配合 `minimumFrameInterval` 允许 15–60fps 区间浮动。
- **静态帧保护**：如果采集层仍重复回调完全相同的 BGRA，host 在 VideoToolbox 前做逐字节去重；只提交成功写入的帧，关键帧请求可从最近缓存帧立即重发。

### 3.2 控制端（接收）

```
QUIC uni-stream 读任务 ─▶ 帧解析 ─▶ VTDecompressionSession (硬解)
                                        │ 解码回调 CVPixelBuffer
                                        ▼
                          LatestFrameSlot（原子指针交换，三缓冲）
                                        ▼
              GPUI render loop: 每 paint 取最新帧 → RenderImage(BGRA) → img() 绘制
音频：
uni-stream 读 ─▶ Opus dec ─▶ 自适应抖动缓冲(ring buffer) ─▶ CoreAudio output callback 拉取
（当前状态：client 端 CoreAudio 播放尚未实现，app 协商 caps 时声明 audio=false，
host 据此跳过音频采集与 Opus 编码；实现播放后恢复 Caps::all()。）
```

要点：
- **渲染解耦**：解码线程与 GPUI 渲染之间用 latest-wins 槽位，渲染永远拿得到一帧、永不阻塞解码。
- **首帧优化**：关键帧到达即渲染，不等 vsync 对齐；窗口尺寸变化触发重新协商分辨率档位。
- RenderImage 更新走 GPUI 图片缓存 id 复用（同一纹理槽换内容），避免每帧新建纹理；若实测上传带宽不足，升级路径见 research.md §R4。

## 4. 线程与运行时模型

| 执行体 | 数量 | 职责 | 注意 |
| --- | --- | --- | --- |
| 主线程 | 1 | GPUI 事件循环与渲染 | 绝不做阻塞 IO |
| SCK 回调队列 | 1–2 | 帧/音频回调入口 | 回调内只入队，不编码 |
| 编码 dispatch 队列 | 1 | VT 编码 | 输出回调直接打包入队 |
| tokio multi-thread runtime | worker=物理核一半 | quinn 收发、会话控制、发现 | 由 removent-net 创建并持有 |
| 解码线程 | 1/路 | VT 解码回调 → LatestFrameSlot | |
| 音频输出回调(CoreAudio 实时线程) | 1 | 从抖动缓冲拉 PCM | 实时约束：无锁(spsc ring)、不分配 |

跨执行体通信全部使用有界通道 + 明确的**满载丢弃策略**（媒体丢新帧保实时，控制消息必须送达故走 QUIC 可靠流自身保证）。

## 5. 连接与会话生命周期

### 5.1 状态机（控制端视角）

```
Idle → Discovering → Connecting(TLS) → Pairing(仅首次) → Requesting(准入)
     → Negotiating(能力协商) → Streaming ⇄ Reconnecting → Idle/Failed
```

被控端对应：`Listening → IncomingRequest(等待用户裁决) → Capturing → …`。

### 5.2 断线与重试（边界情况处理，FR-05/NFR-06）

| 场景 | 行为 |
| --- | --- |
| Wi-Fi 切换/瞬断 (<30s 恢复) | QUIC 连接超时判定（idle_timeout=10s，keep-alive=2s）。客户端进入 `Reconnecting`：指数退避 0.5s→1s→2s→4s（上限 8 次），携带 resume token 重连；成功则免准入恢复会话（含音视频与剪贴板状态），UI 显示"重连中…"遮罩而非断开弹窗 |
| 断线 >30s 或 resume token 过期 | 回落完整流程：重新 TLS+准入。若对方设置了"信任设备自动接入"，用户几乎无感 |
| 被控端休眠 | mDNS goodbye 缺失场景：设备列表条目带 TTL，过期置灰；点击灰条目先发探测 ping（1s×3）再报错 |
| 被控端唤醒 | launchd KeepAlive 拉起服务重新注册 mDNS；客户端下次浏览自动刷新 |
| 显示器热插拔/分辨率变化 | host 发 DisplayListUpdate；正在查看的显示器消失则自动切到主显示器并 toast 提示 |
| 权限中途被撤销 | 捕获/注入 API 返回失败 → 会话降级或结束，明确告知原因并引导权限体检页 |
| 版本不兼容 | Hello 阶段 proto_version 不一致且无法降级 → 双端显示可读的版本错误，附下载链接 |
| 时钟偏差 | 所有 pts 使用各流单调时钟 + 会话建立时的相对基准，不用墙钟，规避 NTP 跳变影响音画同步 |
| 编码器故障（如驱动异常） | codec 层返回错误 → 按 H264 → 低档位 → 结束会话的阶梯降级，每步通知 UI |
| 磁盘满（文件传输） | 传输暂停 + 明确报错；续传记录持久化于 userdata/cache/transfers/ |
| 半开连接（对端崩溃未 FIN） | QUIC idle timeout 兜底；host 侧会话表带心跳检查，僵尸会话 15s 清理并释放捕获资源 |

### 5.3 流控与背压

- 视频 uni-stream 设大接收窗口但发送端**主动限速**：令牌桶按当前目标码率放行；写缓冲积压 >2 帧即丢非关键帧，>8 帧触发降档请求。
- 控制流（含输入）永不被媒体阻塞：独立 bidi stream；输入事件合并策略——鼠标移动只发最新位置（每渲染帧至多一条），按键全部必达。
- 接收端每 250ms 发 StatsReport{rtt, loss, recv_bitrate, jitter, decode_ms}，发送端据此调档（算法详见 protocol.md §6.4）；丢包的完整语义（含队头阻塞对策与音频 PLC）见 protocol.md §7.1。

## 6. 安全架构摘要（详见 protocol.md §4）

- 身份：Ed25519 密钥对存于 `userdata/identity/`，自签证书（rcgen），SHA-256 指纹 = 设备身份。
- 传输：quinn mutual-TLS（双方互 pin 已配对指纹），TLS 1.3。
- 配对：SPAKE2（PIN 为口令）绑定双方指纹，防 PIN 在线爆破；配对成功写入 `userdata/peers.json`。
- 准入：每次会话 host 校验 peer 指纹 → 信任则按准入模式放行，否则弹窗。
- 进程边界：媒体 panic 用 `catch_unwind` 包裹线程入口，崩溃只终止会话不拖垮 app。

## 7. 平台集成要点

### 7.1 菜单栏、daemon 与窗口形态
- **被控服务端是独立进程 `removentd`**（crates/daemon）：无头常驻，launchd LaunchAgent 托管（KeepAlive），与 GPUI app 解耦。
- **管理端是独立 Swift 菜单栏应用 `RemoventTray`**（tray/，SwiftPM/AppKit）：状态色点（空闲绿/会话中橙/禁用灰）、会话列表、配对 PIN 展示、准入审批 NSAlert、服务开关、开机自启 toggle。
- **IPC**：daemon 与管理端经 Unix domain socket（`userdata/run/removentd.sock`，0700）+ JSON Lines（core::ipc）；事件推送（StateChanged/Session*/AdmissionRequest/PairingPin）+ 请求响应（Status/SetEnabled/AdmissionReply/KickSession/Shutdown）。
- GPUI app 的「被控服务」开关改为经 IPC 控制 daemon（daemon 不在线时拉起 removentd）；app 前台时也订阅 daemon 事件作为准入/PIN 弹窗兜底。
- Viewer 会话窗支持全屏（`WindowOptions` fullscreen），全屏下工具栏为悬浮 auto-hide 元素。
- 来连准入弹窗：tray NSAlert 为主，app 弹窗为辅；30s 无响应自动拒绝（protocol §4.4）。

### 7.2 输入注入（removent-input）
- CGEventPost 到 HID session；键盘用虚拟键码 + modifiers；鼠标坐标换算到目标显示器像素空间。
- 注入节流：与显示器刷新对齐批量提交；检测 `CGEventSourceSecondsSinceLastEventType` 防注入风暴。

### 7.3 剪贴板同步（双向）
- 监听：NSPasteboard `changeCount` 轮询（250ms）比对，变化才读取。
- 方向仲裁：每端维护 last_sent_hash/last_received_hash，收到远端内容写入本地粘贴板后标记抑制位，防止回环。
- macOS 15+ 隐私限制：控制端仅在会话窗口聚焦时把远端内容写入粘贴板；失焦超过 60s 自动清空会话写入的内容（可关）。
- 格式分级：UTF-8 文本(P0) → RTF/HTML/PNG(P1) → 文件引用转文件传输(P1，配合 FR-35)。

## 8. 存储布局（便携式 userdata 目录）

**所有用户数据统一存放于 `userdata/` 目录**（决策 D9），不散落到 `~/Library`：

```
<数据根>/userdata/
├── identity/
│   ├── device.key          # Ed25519 私钥（0600）
│   └── device.crt          # 自签证书
├── peers.json              # 信任设备：指纹/名称/公钥/最近连接时间/授予能力
├── settings.toml           # 全部用户设置（含每设备覆盖段）
├── logs/
│   ├── removent.log        # tracing 滚动日志，默认 INFO，诊断包开关 DEBUG
│   └── ...
└── cache/
    ├── transfers/           # 文件传输断点续传记录
    └── update/              # 自动更新下载临时区
```

数据根目录解析顺序（`removent-core::paths`）：
1. 环境变量 `REMOVENT_DATA_DIR`；
2. 开发构建（debug profile）：项目仓库根下 `userdata/`；
3. 发布版：`~/Library/Application Support/removent/userdata/`。

> 注（2026-08 修正）：发布版不再使用 `.app` 同级便携式目录——装进 /Applications 后普通用户不可写，
> 且 app/daemon/tray（含 launchd 启动的 daemon）必须在无环境变量时解析到同一路径；
> 托盘 Swift 侧固定走 Application Support，Rust 侧现已对齐。launchd LaunchAgent 会显式设置
> `REMOVENT_DATA_DIR` 保证一致。

多实例并发写保护：`userdata/.lock` 文件锁（flock），第二实例以"只读控制端"模式运行或退出。

## 9. 发版渠道与自动更新（GitHub Releases，开源路线）

详细流水线定义在 [release.md](./release.md)，此处为架构约定：

- **渠道**：GitHub Releases 为唯一官方渠道（开源项目）。tag 触发 CI 构建 → 签名公证 → 产物上传：
  - `Removent_<ver>_aarch64.app.tar.gz`（主产物）
  - `latest.json`（更新元数据：版本号、下载 URL、SHA256、ed25519 签名、release notes）
- **更新元数据签名**：发布私钥 ed25519 离线保存，客户端内置公钥；下载后验签 + 校验 codesign 有效再落地。
- **客户端检查**：启动时与每 24h 各查一次 `latest.json`（用户可在设置关闭）；有新版仅提示不强更，点击后后台下载到 `userdata/cache/update/`，校验通过后替换 .app 并重启。
- **版本语义**：SemVer；proto_version 与 app major 同步演进（protocol.md §8）。

## 10. 可观测性

- tracing 全局订阅：span 覆盖 `session.connect / video.encode / video.decode / net.send` 等；耗时直方图输出到日志。
- 会话 HUD 数据源：StatsReport 聚合结构（rtt/loss/fps/bitrate/jitter/decode_ms），UI 与日志共用。
- 崩溃报告：panic hook 写 `userdata/logs/panics/`，含线程名与最后 span，不含隐私内容。
