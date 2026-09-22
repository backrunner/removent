# RVP — Removent Protocol 设计

版本：RVP/1（2026-08-22） · 上游：[architecture.md](./architecture.md)

本文是协议唯一权威定义。所有常量、帧格式、消息编号以此为准。

2026-09-20 首发基线：保留当前全部实现与安全校验，统一标为 v1，不兼容未发布的开发版格式。用户输入地址统一为 `removent://域名或IP:端口`，IPv6 使用方括号；relay 的 QUIC / WebSocket 传输单独选择，内部载体地址不作为另一种用户协议。外层 relay ALPN 为 `removent-relay/1`，WebSocket 子协议为 `removent-relay.ws.v1`；设备挑战签名、受众绑定准入与防重放保持启用。正式发布后的版本演进遵循 §8。

## 1. 协议分层与设计原则

```
┌─────────────────────────────────────────────────────┐
│ 应用语义层：会话控制/输入/剪贴板/文件/统计              │  postcard 编码
├───────────────┬─────────────────┬───────────────────┤
│ Control 流     │ Media 流         │ （保留）Datagram   │
│ (QUIC bidi)   │ (QUIC uni ×N)    │                   │
├───────────────┴─────────────────┴───────────────────┤
│ QUIC v1 (quinn)：TLS1.3 mutual-auth、可靠有序、多路复用 │
├─────────────────────────────────────────────────────┤
│ mDNS 发现（UDP/5353 组播） + QUIC 数据面（UDP 48688）    │
└─────────────────────────────────────────────────────┘
```

**为什么基于 QUIC 而不是裸 UDP 自研 ARQ**：加密(TLS1.3)、丢包恢复、拥塞控制、流间无队头阻塞、连接迁移都是久经考验的现成件；我们的自研部分聚焦在发现、配对、会话语义、媒体成帧与自适应上——这正是差异化所在。LAN 场景 RTT 亚毫秒级，QUIC 开销可忽略。

## 2. 常量

| 常量 | 值 | 说明 |
| --- | --- | --- |
| `PROTOCOL_NAME` | `"RVP/1"` | ALPN |
| `PROTO_VERSION: u16` | `1` | 主版本，不兼容变更递增 |
| `DEFAULT_PORT: u16` | `48688` | QUIC 监听端口 |
| `MDNS_SERVICE` | `_removent._udp.local.` | 发现服务类型 |
| `MAGIC` | `[0x52,0x56,0x50,0x31]` ("RVP1") | 控制流首个握手块前缀 |
| `RESUME_WINDOW_SECS` | `30` | 断线快速恢复窗口 |

## 3. 发现层（mDNS）

被控端注册服务实例；控制端浏览。TXT 记录：

| key | 示例 | 说明 |
| --- | --- | --- |
| `v` | `1` | 发现记录版本 |
| `name` | `Studio-Mac` | 展示名（默认 = 计算机名） |
| `fp` | `a1b2c3d4e5f60718` | 证书指纹 SHA-256 前 8 字节 hex（防仿冒展示比对用） |
| `cap` | `video,audio,clip,file` | 能力位 |
| `busy` | `0/1` | 已有活跃会话 |

- 实例名冲突由 mdns-sd 自动改名解决。
- 条目 TTL 过期置灰（见 architecture.md §5.2）；连接前可用单播探测 ping 验活。

## 4. 身份、配对与准入

### 4.1 身份
每台安装生成 Ed25519 长期密钥 → rcgen 自签证书（CN=device name, SAN=指纹）。指纹 `FP = SHA256(DER(cert))` 即设备 ID。私钥仅存 `userdata/identity/device.key`。

### 4.2 TLS 与互认
quinn 双向 TLS 1.3：
- 服务端证书链 = 自签身份证书；
- 客户端同样出示客户端证书（同一套身份体系，用途区分 via extended key usage）；
- 校验规则：对端证书指纹存在于本端信任库（`peers.json`）→ 通过；否则进入配对流程。未知指纹在 TLS 完成后于应用层拒绝（TLS 层先允许完成以便展示指纹给用户裁决）。

### 4.3 配对流程（首次连接）
前提：双方已建立 QUIC 连接但互不相识。

```
Client                                Host
  │ PairingBegin{nonce_c, fp_c}        │
  │ ─────────────────────────────────▶ │ 弹窗显示：请求方名称、fp 短串(前8字节hex)、
  │                                    │ 6 位 PIN（随机生成，300s 有效）
  │ ◀───────────── PairingChallenge ── │ {nonce_h}
  │ 用户把 PIN 输入客户端                 │
  │ PairingVerify{spake2_msg_c}        │  SPAKE2(PIN=用户输入, id_c=fp_c, id_h=fp_h)
  │ ─────────────────────────────────▶ │  host 用其生成的 PIN 参与 SPAKE2
  │ ◀──────────── spake2_msg_h+MAC ─── │
  │ 双方导出 confirm_key 验证 MAC        │
  │ PairingComplete{sig_c(key_c)}      │ 双向签名交换确认身份绑定
  │ ◀──────────── sig_h(key_h) ─────── │
  ▼                                    ▼
 各自写入 peers.json（指纹+公钥+授予能力）
```

要点：
- PIN 不作为长期凭据，只参与一次 SPAKE2 会话密钥推导，抗在线爆破（SPAKE2 无字典放大）。
- UI 同时展示两侧指纹短串供肉眼比对（防中间人），见 ui-design.md 配对弹窗。
- 「始终允许」= 授予能力集写入 peers.json；「仅本次」则 peers.json 只存身份不存授权。

### 4.4 准入（每次会话）
`SessionRequest` 携带请求能力集 {video, input, clipboard, file}。host 按准入模式（FR-06）：
- `ask`：弹窗（申请人/指纹/cap），30s 超时视为拒绝；
- `trusted-auto`：peers 中且能力 ⊆ 已授能力 → 直接放行；
- `deny-all`：一律拒绝。
会话建立后签发 `resume_token(128bit)`，RESUME_WINDOW 内重连免准入（§7.3）。

## 5. 连接建立与会话协商

### 5.1 控制流握手（bidi stream #0）
客户端连上后立即打开：

```
[ MAGIC "RVP1" ][u16 proto_version][u64 feature_bits]
Hello{ app_version, device_name, os_ver, caps[] }
HelloAck{ proto_version, caps[], resume_token_opt }
```
- `feature_bits`：按位声明扩展（bit0=file-transfer, bit1=two-way-audio, bit2=hdr, bit5=software AV1 temporal units, …）。
- 版本不兼容且无公共特性集 → `Error{code=VersionMismatch}` 后关流。

### 5.2 会话协商
`SessionRequest` → `SessionAccept`/`Reject{reason}` 之后：

```
Negotiate{
  displays:[{id,w,h,scale,dpi}],        // 可选目标列表
  video:{codec: HEVC|H264|AV1, max_fps, max_bitrate_kbps, scale_steps[]},
  audio:{enabled, sample_rate:48000, channels:2, frame_ms:10, bitrate_kbps},
  input_caps:{relative_pointer:bool},
  clock_base_us                          // 双端各自单调时钟基准换算说明见 §6.5
}
NegotiateAck{ chosen… }                   // 接收端裁剪后的最终参数
```
协商完成后 host 打开媒体 uni-stream 开始推流。

## 6. 数据面格式

### 6.1 视频帧（uni-stream，顺序写）

```text
offset  size  字段
0       1     stream_type = 0x01 (VIDEO)
1       8     frame_id: u64            // 发送侧单调递增
9       8     pts_us: i64              // host 单调时钟微秒
17      1     flags: bit0=keyframe bit1=config_changed(sps/pps/vps 内联) bit2=end_of_stream
18      1     codec_id: 0x01=H264 0x02=HEVC 0x03=AV1
19      2     width: u16 LE            // 仅 keyframe/config 帧有效
21      2     height: u16 LE
23      4     payload_len: u32 LE      // Annex-B NALU 流长度
27..          payload                  // H264/HEVC 为 Annex-B NALU 流；AV1 为完整 temporal-unit OBU 流
```
- 一帧一次 `write_all`；接收端按头解析后整段读取，零拷贝切分。
- 参数集变化必须随关键帧内联，客户端据此热重置解码器（支持运行中改分辨率）。
- host 对完整 BGRA 内容做逐字节去重：首帧、像素有变化的帧和强制关键帧必须编码发送；与最近一次**成功写入**完全相同的普通帧直接跳过。去重状态在编码失败、SendGate 丢帧或分辨率重建时不提交/重置，避免把接收端落后的画面误认为已同步。关键帧请求可复用最近缓存帧，因此静止画面也能立即恢复。
- H.264/HEVC 的帧间预测已经压缩了未变化区域；RVP/1 不额外传 raw pixel delta，避免破坏硬件编码器的全局预测和 QUIC 有序流语义。
- `codec_id=0x03` 使用完整 rav1e temporal-unit OBU payload。关键 temporal unit 自带 sequence-header OBU，不发送单独 `av1C`；只有双方握手都声明 `feature_bits::SOFTWARE_AV1` 时才可选择。AV1 默认 opt-in（`REMOVENT_VIDEO_CODEC=av1`）；协商前的软件 encoder 探测失败时回退 HEVC，已建立会话中的不可恢复编码错误按 `InternalError` 结束会话。

### 6.2 音频包（uni-stream）

```text
0       1     stream_type = 0x02 (AUDIO)
1       2     seq: u16 LE              // 用于抖动缓冲排序/丢包统计
3       8     pts_us: i64
11      1     flags: bit0=DTX(舒适噪声) bit1=mic_direction(双向语音时)
12      2     payload_len: u16 LE
14..          payload                  // 单个 Opus 包（10ms@48kHz stereo）
```

### 6.3 控制消息（bidi 流，postcard）
外层：`[u32 len][postcard(ControlMsg)]`。

```rust
enum ControlMsg {
    // 会话
    SessionRequest { caps: Caps }, SessionAccept, SessionReject { reason },
    SessionEnd { reason }, Ping { ts_us }, Pong { ts_us },
    // 显示与质量
    DisplayListUpdate { displays: Vec<DisplayInfo> },
    SelectDisplay { id: u64 },
    QualityControl { bitrate_kbps: u32, fps: u8, scale: f32 },
    KeyframeRequest,
    StatsReport { rtt_ms: f32, loss_pct: f32, recv_kbps: u32,
                  jitter_ms: f32, decode_ms: f32, render_fps: f32 },
    // 输入（client→host，高频小包）
    MouseEvent { display_id: u64, x_px: f32, y_px: f32, buttons: u8, kind: MouseKind },
    ScrollEvent { display_id: u64, dx_mm: f32, dy_mm: f32, phase: ScrollPhase },
    KeyEvent { vk_code: u16, modifiers: u8, kind: KeyKind, unicode: Option<char> },
    // 光标（host→client）
    CursorShape { cursor_id: u32, w: u16, h: u16, hotspot_x: u8, hotspot_y: u8, rgba: Bytes },
    CursorPosition { x_px: f32, y_px: f32, visible: bool },
    // 剪贴板
    ClipboardSync { seq: u32, format: ClipFormat, data: Bytes }, // Text|Rtf|Html|Png|FileRefs
    ClipboardAck { seq: u32 },
    // 文件传输：控制流只走信令；数据块在独立 uni-stream 上传输
    // （FileChunkStream 帧：[u64 transfer_id][u64 offset][u32 len][data]），
    // 避免大流量阻塞控制流队头。
    FileOffer { … } FileAccept { … } FileProgress { … } FileComplete { … } FileCancel { … }
}
```
注：鼠标坐标为目标显示器像素空间（逻辑像素 × 缩放后）的 `f32`；枚举变体只增不改义（§8）。

原生会话的 `ClipboardSync` 由独立低优先级 bidi 流发送：`CLP1` 四字节魔数，
随后一条上述长度前缀消息，最后 FIN。每份快照最多 4 MiB、30 秒期限；每端同时
发送一份，待发送槽只保留最新快照。接收端在完整校验后交给同一会话的剪贴板处理，
`ClipboardAck` 仍走控制流。它与媒体、控制共用同一个双向认证的 RVP 连接，
不经过新的信任或认证路径。协商/配对完成后才接受这些附加 bidi 流。

### 6.4 自适应算法（发送端执行）
采用文字清晰优先策略，直连、QUIC relay、Cloudflare WSS relay 共用同一个端到端控制器。
- 接收端每 500ms 回报实际收包吞吐、到达抖动、未完成帧的等待和解码耗时；没有媒体活动时不制造健康样本。发送端每 250ms 合并有效反馈（750ms 内）与本地写入压力/QUIC 丢包。
- 丢包 >2%、抖动 >20ms、解码耗时超过 `max(40ms, 帧间隔)` 或发送阻塞 → 降档。先按比例降低 fps 和码率，保持每帧预算；15 fps 以下才在清晰预算内进一步降码率/帧率。
- `scale` 保持 1.0（相对于协商后的捕获尺寸）。每帧预算至少 `ceil(width*height/100)*8` bit；通常最低 2 fps，码率下限为对应预算且至少 64 kbps、不超过用户上限。1080p 对应 332 kbps / 2 fps。上限不足以负担一帧预算时，按 1 fps 和该上限尽力发送，不宣称可读性保证。
- H.264/HEVC 在编码器支持时设置 `MaxAllowedFrameQP=28`，AV1 显式固定 rav1e bitrate 模式的 quantizer 上限 100。QP 限制导致的跳帧反馈为压力，不把它误判成编码错误；不支持硬件 QP 上限时记录降级。
- 连续 5s 丢包 <0.5%、抖动 <10ms 且解码/发送健康后，码率 ×1.25；先恢复每帧细节预算，再增加 fps。调档间隔至少 2s，无证据或过期样本中断恢复。绝对 RTT 高、静态屏幕吞吐低均不单独触发降档。
- 质量变化由发送端刷新关键帧；接收端 `KeyframeRequest` 仅用于解码恢复。手动档位（FR-15）钳制上界；显示模式变化更新每帧预算。

预算与量化上限是工程保护，不等同于任意字体/内容的可读性证明。实测边界、弱网首帧代价见 [清晰度报告](../docs/readable-quality-2026-09-20.md)。

### 6.5 时间戳与音画同步
- 各流 pts 为**发送端单调时钟**；握手时双端交换 `clock_base_us`（各自 boot-time 基准）仅供诊断，不做墙钟同步。
- 音画对齐在接收端：以视频最新渲染 pts 为参考轴，音频抖动缓冲动态伸缩（±5% 重采样微调，rubato）把偏差拉回 ±40ms 内。

## 7. 异常与恢复

### 7.1 丢包语义与恢复（按流类型）

QUIC 保证**最终送达且有序**，因此协议里不存在"应用层丢包重传"；丢包处理 = 三件事：
传输层交给 QUIC 重传、发送端不让过期数据进流、接收端对迟到数据按播放时限丢弃。

| 流 | 丢包后果 | 处理机制 |
| --- | --- | --- |
| Control (bidi) | 丢包需要可靠重传，仍会增加延迟 | 最高流优先级；收发独立推进，发送阻塞不挡输入接收；只合并相邻同语义鼠标移动，保留按下、松开和坐标几何的顺序。会话终止时释放所持按键/按钮 |
| Video uni-stream | 重传期间后续帧被队头阻塞，到达时已"过时" | ① 发送端 **drop-before-send**：写缓冲积压 >2 帧（≈33ms@60fps）即丢弃待发的非关键帧、>8 帧触发降档——过期帧根本不进流；② 接收端检测**卡顿**（decode 队列空转后突发积压 / 帧到达间隔抖动超阈值）→ 发 `KeyframeRequest`，host 立刻插 IDR 收敛画质与延迟；③ VT 解码器异常时同样以最近 IDR 重置 |
| Audio uni-stream | 重传后包迟到 | 抖动缓冲按 playout deadline 丢弃 `seq` 已过播放头的包；真缺包走 Opus PLC（解码器内 concealment），并开启 inband FEC 提高连续丢包下的可懂度 |

补充约束：
- **丢包率信号来源**：QUIC 层不向应用暴露原始丢包，直接读 quinn `Connection::stats()` 的
  `path.lost_packets` / `sent_bytes` 差分计算 loss_pct 与实际带宽（§6.4 的输入即此）；音频 seq 只用于缓冲排序统计，不作为网络丢包依据。
- **IDR 代价**：一次强制 IDR 约增加 100–300kbps 瞬时码率，KeyframeRequest 最小间隔限流 500ms，防止恶性循环。
- **未来演进**（不进 v1）：delta 帧改走 QUIC Datagram（不可靠）+ 帧依赖标志，配合丢包即请求 IDR 的 WebRTC 式恢复。前提是自管拥塞与优先级，收益在 >1% 丢包的劣质 Wi-Fi 上才明显，LAN 场景暂无必要。

### 7.2 QUIC 参数建议
`idle_timeout=10s, keep_alive=2s`。RVP 与原生 QUIC relay 使用 Quinn BBR，保留拥塞控制与 pacing。
控制流优先级 `10`，媒体 `-10`，剪贴板 `-20`。初始发送内存预算 512 KiB；
被控端根据已选码率 × 基础 RTT + 32 KiB 调整为 64 KiB 至 2 MiB，
不使用排队膨胀后的 RTT。视频按 4 KiB 写入并让出调度，避免单个大分配迟迟不释放发送额度。
relay 的外层 DATAGRAM 收发队列各 32 KiB；WebSocket 每方向 16 条消息、单条最多 2056 字节。
这些边界降低积压，不消除公网传播、QUIC 重传或 WebSocket/TCP 队头阻塞。
实测方法与限制见 [弱网评估](../docs/wan-latency-2026-09-20.md)。

### 7.3 错误码（Error 消息复用）
`VersionMismatch / Unauthorized / Busy / DisplayGone / PermissionLost / Internal`——UI 必须为每个码准备人话文案。

### 7.4 快速恢复
断线后 client 用 `resume_token` 重连：跳过配对与准入，服务端 15s 心跳清理僵尸会话（architecture.md §5.2）。30s 有效窗口从**会话结束**（最后一次见到该 peer）起算，而非签发时；轮换消息可能丢失，校验容忍上一代 token 一次性使用，新会话签发新 token。

## 8. 版本演进纪律

1. `PROTO_VERSION` 递增当且仅当不兼容：删除/改变既有字段语义、必填项新增。
2. 枚举只追加变体；新增消息走未占用编号段；接收端遇到未知变体必须**跳过而非报错**（len 前缀天然支持）。
3. 媒体二进制头保留 `flags` 扩展位；codec_id 预留 AV1。
4. 发布流程中 `latest.json` 附带 `min_compatible_proto`，升级器据此提示两端需同升的场景。
