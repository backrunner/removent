# 按剩余带宽发送文字补块

原分区压缩实验按块数限制发送量，最大一批补块在 2 Mbps 链路上占用 63.46 ms。新调度按实际序列化字节分配额度。在同一段 720p30 混合桌面上，单批最大占用降至 8 ms。本机 TCP 三轮交叉顺序回放中，相对同一视频帧纯 HEVC 接收时间的额外时延 p95 从 1.07 秒降至 1.5–4.0 ms。[原实验](region-compression-2026-11-22.md)

## 调度规则

- 每个源时间点先结算上一段时间能够发送的字节，再扣本帧视频及其帧头。之前的字节尚未发完、链路阻塞或容量为零时，暂停可选补块。每批上限取下一视频帧期限前剩余容量和 8 ms 链路容量的较小值，并为下一帧预留 1 ms。空闲时间不会累积为可一次性发送的额度。有效容量是扣除其他业务后的应用层速率，可以随时间更新。[预算计算](../../packages/media-codec/examples/region_compression/pacing.rs)
- 发送端根据**完整批头、分片头和压缩后字节**决定每次能发送多少。大块可以跨多帧，先完成已经开始的一块。解码端只有在收到完整块并验证压缩数据后才覆盖视频画面。超低容量下连最小分片都装不下时，继续显示基础视频。[补块收发](../../packages/media-codec/examples/region_compression/tiles.rs)
- 新的视频帧若使已安装或正在传输的旧块失效，实验帧头中的一个标志让接收端在显示该视频前清空补块缓存。本次刷新增加零字节，也会撤销其他仍有效的块，后续按可用额度逐步补回。该标志只用于实验生成的媒体；线上原生连接继续使用既有编码和协议。

## 相同视频的逐帧比较

使用 M5 Max 的生产 HEVC 编解码器，1280×720、30 fps、120 帧，视频目标 2,000 kbps。场景为中文编辑器旁的小窗动画，第 24 帧修改一行文字。将视频单独发送、叠加旧补块和叠加新补块的各组具有一致的视频 PTS 和逐帧视频字节数。表中列出独立 FIFO 模型中，前序补块使**同一视频帧**延迟的最大值。

| 有效容量 | 旧调度最大额外等待 | 新调度最大额外等待 | 新调度单批峰值 | 四秒内新调度补块总量 |
| --- | ---: | ---: | ---: | ---: |
| 1 Mbps | 2,141.36 ms | 0 ms | 0 B | 0 B |
| 2 Mbps | 1,070.68 ms | 0 ms | 2,000 B | 74,000 B |
| 4 Mbps | 290.94 ms | 0 ms | 4,000 B | 334,951 B |

在 2 Mbps 下，旧策略使 116/120 个视频帧额外等待。新策略在启动大帧排空后，于源时间 1.4 秒首次发送修复。整次更新收齐的 p95 从旧策略的 1,403.58 ms 回到纯 HEVC 的 776.57 ms。基础视频自身仍有启动积压。4 Mbps 时，分块独立压缩、分片头和失效后的重新发送可以使补块总字节超过原先的整批压缩。[逐容量结果](../benchmarks/2026-11-22-region-compression/pacing/delivery.csv) [逐帧记录](../benchmarks/2026-11-22-region-compression/pacing/delivery-frames.csv)

动态链路在 1.5 秒时停止提供服务，1.9 秒恢复至 1 Mbps，2.8 秒升至 4 Mbps。新策略在阻塞和低速恢复期间均发送零补块，升速 500 ms 后首次重新获得额度。独立分段容量模型中，旧策略令 113 个视频帧额外等待，新策略为零。这个测试使用显式速率和阻塞信号，线上接入仍需由实际传输反馈提供这两个量。[链路时间表](../benchmarks/2026-11-22-region-compression/pacing/link-changes.csv) [动态逐帧结果](../benchmarks/2026-11-22-region-compression/pacing/dynamic-frames.csv)

## 真实时钟的回环接收

发送器按原始 PTS 释放实际编码的媒体字节，用户态以 2 Mbps 限速写入本机 TCP，接收线程独立记录每个视频或补块收齐的时刻并核验 SHA-256。三轮轮换组顺序，正式记录 1,080 个视频和 180 个补块。以下为视频从源 PTS 到接收完整视频的 p95，单位为毫秒。

| 轮次 | 纯 HEVC | 旧调度 | 新调度 | 新调度相对纯 HEVC 的同帧增量 p95 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 777.5 | 1,407.0 | 779.3 | 2.4 |
| 2 | 782.3 | 1,408.2 | 778.6 | 1.5 |
| 3 | 801.1 | 1,409.3 | 785.2 | 4.0 |

最后一列先按同一源帧做差，再取分位数。三轮中，新策略最大同帧增量为 4.9 ms，纯 HEVC 自身跨轮的同帧时延极差 p95 为 23.8 ms。另保留与其他编译和实验并发的第 0 轮，该轮新策略的同帧增量 p95 为 78.3 ms。回环复播采用预生成的调度决定，结果包括 CPU 调度和 TCP 收发开销。线上 QUIC 带宽估计、跨设备及屏幕呈现仍需单独验证。[逐记录接收时间和摘要校验](../benchmarks/2026-11-22-region-compression/pacing/loopback.csv)

## 文字清晰度与撤销

2 Mbps 下，第 24 帧编辑当帧可从基础视频读到新句。新策略在第 49 帧将全部 3,262 个发生变化的像素修复到源值，第 51 帧整行逐像素一致，距编辑 **900 ms 源时间**。旧策略用时 100 ms，但当时还有视频等待发送。三组编码结果在 15 个 OCR 样本上均与原图识别文本一致。带宽被视频占满时，文字可能长期保持有损。20 秒混合样本的文字区域 PSNR 为纯 HEVC 38.60 dB、新策略 40.54 dB，运动区域两组均为 31.77 dB。[文字逐帧收敛](../benchmarks/2026-11-22-region-compression/pacing/edit-repair.csv) [OCR 记录](../benchmarks/2026-11-22-region-compression/pacing/ocr.json) [20 秒样本](../benchmarks/2026-11-22-region-compression/pacing/mixed-20s.csv)

另用十秒静态文稿先安装 207 个补块，再于第 300 帧改字、反转主题并开始滚动。此帧的视频为 133,994 B，补块额度和补块字节均为零。清除之前有 742,336 个像素与纯 HEVC 解码结果有别，清除当帧和下一帧的全部 921,600 个像素均与纯 HEVC 一致。全屏游戏样式动画和全屏滚动对照都产生零补块。[重置标志与像素校验](../benchmarks/2026-11-22-region-compression/pacing/late-edit.json) [四场景对照](../benchmarks/2026-11-22-region-compression/pacing/scenes.csv)

## 复现

最后两个位置参数分别是高度和链路有效容量，视频目标码率是场景名后的第一个参数。设置 `REMOVENT_REGION_TRACE_DIR` 导出应用层媒体字节及长度记录；改用 `REMOVENT_REGION_REPLAY_DIR` 还会保存解码 BGRA 和源时间戳，供 OCR 和像素比较使用。以下命令在仓库根目录运行，Python 图像检查使用 Pillow 和系统 Vision。

```sh
rtk cargo build --locked --release -p removent-media-codec --example region_compression
REMOVENT_REGION_REPLAY_DIR=target/region-pacing/mixed rtk proxy target/release/examples/region_compression mixed 2000 120 30 1280 720 2000
rtk proxy python3 scripts/fixtures/region_delivery.py --require-identical-video 2000 target/region-pacing/mixed/mixed-{baseline,hybrid,paced}.csv
rtk proxy python3 scripts/qa/region_pacing_loopback.py --trace-dir target/region-pacing/mixed --kbps 2000 --trials 3 --output target/region-pacing/loopback.csv
rtk proxy python3 scripts/qa/region_compression_ocr.py target/region-pacing/mixed target/region-pacing/ocr --variants baseline hybrid paced --repair
REMOVENT_REGION_REPLAY_DIR=target/region-pacing/edit rtk proxy target/release/examples/region_compression late-edit 2000 315 30 1280 720 2000
rtk proxy python3 scripts/qa/region_late_edit.py target/region-pacing/edit
changes=docs/benchmarks/2026-11-22-region-compression/pacing/link-changes.csv
REMOVENT_REGION_LINK_TRACE=$changes REMOVENT_REGION_TRACE_DIR=target/region-pacing/dynamic rtk proxy target/release/examples/region_compression mixed 2000 120 30 1280 720 2000
rtk proxy python3 scripts/fixtures/region_delivery.py --link-trace "$changes" --require-identical-video 2000 target/region-pacing/dynamic/mixed-{baseline,hybrid,paced}.csv
rtk cargo test --locked --release -p removent-media-codec --example region_compression
```
