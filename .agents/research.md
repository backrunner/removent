# 技术选型调研与 License 合规

版本：v1.0（2026-08-22，基于当期 crates.io / Apple 文档核实） · 上游：[architecture.md](./architecture.md)

## 1. 选型总表

| 领域 | 选型 | 版本参考 | License | 备选 | 结论理由 |
| --- | --- | --- | --- | --- | --- |
| UI 框架 | gpui | 0.2.x（crates.io 官方发布） | Apache-2.0 | — | 项目既定；macOS Metal 渲染成熟 |
| 组件库 | gpui-component (longbridge) | 0.5.x | Apache-2.0 | 自研薄层 | 60+ 组件、shadcn 风格主题、i18n 内建、生产验证（Longbridge Pro） |
| 传输 | quinn (QUIC) | 0.11.x+ | MIT/Apache | 裸 UDP 自研 ARQ | 见 protocol.md §1；注意跟进安全通告（曾出过 Initial 包 DoS CVE，锁定已修复版本） |
| 发现 | mdns-sd | 0.13+ | MIT/Apache | 系统_dns-sd FFI | 纯 Rust 无 Bonjour 依赖；自带冲突改名 |
| 身份/证书 | rcgen + rustls(aws-lc-rs) | — | MIT/Apache 等 | ring 后端 | 自签证书与 quinn 默认栈一致 |
| 配对 PAKE | spake2 | — | MIT/Apache | SRP-6a | SPAKE2 更简单且无字典攻击面 |
| 屏幕采集 | screencapturekit crate (doom-fish/svtlabs) | v8.x，活跃维护 | MIT/Apache | objc2-screen-capture-kit(官方生成绑定) | 高层 API 含音频采集/IOSurface 零拷贝/异步支持；objc2 系作为兜底 |
| 视频编解码 | videotoolbox crate（同作者）或 objc2-video-toolbox | — | MIT/Apache | shiguredo/video-toolbox-rs(Apache) | 硬编硬解 HEVC/H264；优先 doom-fish 系（与 SCK 绑定同生态）；不行就 objc2 直写 |
| 音频编解码 | opus (opus-rs, SpaceManiac) | 0.3.x | MIT/Apache | audiopus; rusty-opus(纯Rust新秀,观察) | libopus 成熟绑定；FEC/PLC 全支持 |
| 音频 IO | cpal | 0.15+ | MIT/Apache | objc2 CoreAudio 直写 | 播放够用；若实时回调约束不满足再下沉 |
| 重采样 | rubato | — | MIT/Apache | — | 抖动缓冲微伸缩用 |
| 序列化 | postcard + serde | 1.x | MIT/Apache | protobuf(prost) | 控制消息紧凑；媒体帧手写头（protocol.md §6） |
| 平台绑定 | objc2 + objc2-app-kit + core-graphics | — | MIT/Zlib/Apache | swift-rs 桥 | NSStatusItem/NSPasteboard/CGEvent 都走这里 |
| 日志 | tracing + tracing-subscriber | — | MIT/Apache | log | span 化性能观测 |
| 异步运行时 | tokio | 1.x | MIT | smol | quinn 默认搭配 |

## 2. macOS 系统 API 要点

### 2.1 ScreenCaptureKit
- 基础能力 macOS 12.3+；**系统音频采集需 13.0+**（本项目最低版本线由此确定 = 13.0 Ventura）。
- 采集是纯 TCC 门禁（"屏幕与系统录音"权限），无 entitlement 可替代；分发必须带 `NSScreenCaptureUsageDescription`。
- 性能实测参考：60fps + 48kHz 双声道全链路约占单核 ~2%（Apple Silicon），1080p 首帧 30–100ms。
- 关键实践：
  - `SCContentFilter` **排除本 app 自身窗口**防套娃；
  - 音频 `excludesCurrentProcessAudio=true` 防自环；
  - 帧回调只入队不处理；BGRA 路径上传最快；
  - `minimumFrameInterval` 控制静态画面功耗。

### 2.2 VideoToolbox
- `VTCompressionSession`：RealTime=true、latency 优先、CABAC、按需 IDR（`ForceKeyFrame`）；HEVC Main 优先、H264 High 回退；Apple Silicon 全系硬编。
- `VTDecompressionSession`：输出 CVPixelBuffer → 转 BGRA 喂 GPUI RenderImage；分辨率变更需重建会话（协议侧以 config_changed 标志联动，protocol.md §6.1）。
- Intel Mac 硬编对实时流受限（CBR 支持差）→ 软件回退路径仅保编译（NFR-09/Q1）。

### 2.3 输入注入
- `CGEventPost`（HID session）+ 辅助功能权限；虚拟键码随事件携带；坐标换算到目标显示器像素空间。
- 注意 Secure Event Input 冲突（密码框场景）：检测 `CGEventSourceSecondsSinceLastEventType` 异常时降级提示。

### 2.4 TCC 权限自动化（开发期）
- CI 无法自动授予屏幕录制/辅助功能 → 单元测试只覆盖非 TCC 路径；集成验收用手动脚本 + `tccutil` 重置流程写入 CONTRIBUTING。

## 3. License 合规策略

| 关注点 | 结论 |
| --- | --- |
| RustDesk（AGPL-3.0） | **Clean-room**：团队不得阅读其源码；仅允许依据公开的产品行为、公开文档与通用协议知识自行实现。任何"参考其实现"的 PR 直接拒绝。本仓库所有设计文档不引用 rustdesk 代码路径 |
| GPUI/gpui-component/quinn/mdns-sd/SCK 绑定/VT 绑定/opus | 均 MIT/Apache-2.0 系，商用友好；THIRD-PARTY 清单由 CI (`cargo deny`) 维护 |
| HEVC/H.264 专利 | 我们通过调用 OS 编解码器获得专利授权保护（Apple 作为实现方），不自含编码实现；禁止引入 openh264/x264 静态链接以免扩大专利暴露面 |
| Opus | ISC，无附加义务 |
| 本项目自身 | 建议 MIT 或 Apache-2.0 双许可（开源路线，见 release.md） |

## 4. 风险登记簿

| # | 风险 | 等级 | 缓解 |
| --- | --- | --- | --- |
| R1 | GPUI pre-1.0 API 破坏性变更 | 高 | 锁定精确版本 + 升级走独立分支灰度；UI 层薄封装隔离 |
| R2 | RenderImage 每帧 CPU 上传带宽不足（4K60 场景） | 中 | MVP 用 BGRA→RenderImage 路径先达标 1440p60；升级路径：IOSurface 直接映射为 CAMetalLayer 子视图叠加（仍属 Rust 实现，GPUI 承担 chrome），预留 `media-present` trait 切换点 |
| R3 | quinn 安全通告 | 中 | dependabot + cargo audit；关注 quinn-rs 公告 |
| R4 | macOS 15+ Local Network / Pasteboard 隐私门禁变化 | 中 | 权限体检页集中引导（FR-55）；剪贴板聚焦拉取策略（architecture §7.3） |
| R5 | SCK 行为随 macOS 大版本漂移（本项目开发机已是 macOS 27 beta） | 中 | media-capture trait 隔离；CI 固定最低支持版本 runner |
| R6 | HEVC 在部分老接收机硬解不可用 | 低 | 能力协商回退 H264（协议已留 codec_id） |
| R7 | mDNS 被企业路由器/AP 抑制 | 低 | FR-02 手动 IP 直连兜底 |

## 5. 明确不引入的东西

- FFmpeg/libav（体积、license 与专利面）、WebRTC 栈（超出 LAN 需求且重）、Electron/WRY WebView（违背全 Rust 目标，gpui-component 的 webview feature 不启用）。
- 任何 GPL/AGPL 组件进入依赖树（cargo deny 强制）。
