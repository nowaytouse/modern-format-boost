# v5.71 预检查模块处理逻辑

## 🎯 核心决策树

```
输入视频
  ↓
【优先级1】文件异常检测
  ├─ 分辨率 <16px 或 >16K → ❌ CannotProcess
  ├─ 时长 <0.05s → ❌ CannotProcess
  └─ FPS ≤0 或 >10000 → ❌ CannotProcess
  ↓
【优先级2】古老编解码器检测 ⭐
  ├─ Theora/RealVideo/VP6/VP7/WMV/MJPEG/Cinepak/Indeo
  └─ → 🔥 StronglyRecommended（强烈建议转换！）
  ↓
【优先级3】现代编解码器检测
  ├─ HEVC/H.265/AV1
  └─ → ⚠️ NotRecommended（警告但继续）
  ↓
【优先级4】编解码器自适应阈值
  ├─ 计算编码效率因子
  │  ├─ H.264: 1.0 | HEVC: 0.65 | AV1: 0.5 | 古老: 2.0-3.0
  ├─ 调整比特率/BPP 阈值
  ├─ 极低BPP(<0.05) + 低比特率 → 🔵 Optional
  └─ 低BPP(<0.10) + 低比特率 → ✅ Recommended
  ↓
【优先级5】默认情况
  └─ → ✅ Recommended（建议处理）
```

## 📊 FPS 分类（4个等级）

| 等级 | 范围 | 说明 | 图标 |
|------|------|------|------|
| Normal | 1-240 fps | 电影24fps、视频30/60fps、高刷240fps | 🟢 |
| Extended | 240-2000 fps | 高速摄影、特殊软件导出 | 🟡 |
| Extreme | 2000-10000 fps | Live2D、3D软件、超高速摄影 | 🟠 |
| Invalid | >10000 fps | 元数据错误 | 🔴 |

## 💾 压缩潜力（5个等级）

| 等级 | 条件 | 说明 | 图标 |
|------|------|------|------|
| VeryHigh | 古老编解码器 或 BPP>0.50 | 10-50倍压缩提升 | 🔥 |
| High | BPP > 0.30 | 高压缩空间 | ✅ |
| Medium | 0.15 ≤ BPP ≤ 0.30 | 中等压缩空间 | 🔵 |
| Low | BPP < 0.15 | 文件已优化 | ⚠️ |
| VeryLow | 现代编解码器 | 重编码有质量损失 | ⛔ |

## 🎯 处理建议（5个等级）

| 等级 | 触发条件 | 行为 | 图标 |
|------|---------|------|------|
| StronglyRecommended | 古老编解码器 | **继续处理** - 最佳升级目标 | 🔥 |
| Recommended | 标准编解码器（H.264等） | **继续处理** - 有提升空间 | ✅ |
| Optional | 已有一定优化 | **继续处理** - 收益有限 | 🔵 |
| NotRecommended | 现代编解码器（HEVC/AV1） | **警告但继续** - 可能质量损失 | ⚠️ |
| CannotProcess | 文件异常 | **强制停止** - 无法处理 | ❌ |

## 🔧 编码效率因子

用于调整比特率/BPP 阈值的编码器效率系数：

```
基准公式：
expected_min_bitrate = 2500 kbps (1080p@30fps H.264)
                     × (分辨率因子)
                     × (FPS因子)
                     × (编码效率因子)

编码效率因子：
- H.264: 1.0 (基准)
- HEVC: 0.65 (高效，节省35%)
- AV1: 0.5 (最高效，节省50%)
- Theora: 2.5 (低效，需要2.5倍bitrate)
- MJPEG: 3.0 (极低效)
```

## 📋 古老编解码器列表

### 2000-2010年代
- **Theora** - 开源视频，WebM前身
- **RealVideo** (rv30/rv40) - 曾经的流媒体标准
- **VP6/VP7** - Flash Video时代
- **WMV** (wmv1/wmv2/wmv3) - Windows Media Video
- **MSMPEG4** (msmpeg4v1/v2/v3) - DivX前身

### 90年代
- **Cinepak** - CD-ROM时代
- **Intel Indeo** (iv31/iv32/iv41/iv50)
- **Sorenson Video** (svq1/svq3) - QuickTime
- **Flash Video H.263** (flv1)
- **Microsoft Video 1** (msvideo1)
- **QuickTime** (8bps/qtrle/rpza)

### 低效中间格式
- **Motion JPEG** (mjpeg/mjpegb) - 每帧独立JPEG，极低效
- **HuffYUV** - 无损但体积大

## 🎬 实际应用示例

### 示例1：古老编解码器（Theora）
```
输入: video.ogv (Theora, 1080p30, 50 Mbps)
  ↓
【优先级2】检测到 Theora
  ↓
🔥 StronglyRecommended
   "检测到Theora（开源视频，WebM前身），
    强烈建议升级到现代编解码器（可获得10-50倍压缩率提升）"
  ↓
✅ 继续处理 → 转换为 HEVC/AV1
```

### 示例2：现代编解码器（HEVC）
```
输入: video.mp4 (HEVC, 1080p30, 5 Mbps)
  ↓
【优先级3】检测到 HEVC
  ↓
⚠️ NotRecommended
   "源文件已使用现代高效编解码器（HEVC或AV1），
    重新编码可能导致质量损失"
  ↓
⚠️ 警告但继续 → 用户可选择是否处理
```

### 示例3：高FPS视频（Live2D）
```
输入: animation.mp4 (H.264, 1080p, 3000 fps)
  ↓
【优先级1】检查FPS
  ├─ FPS = 3000 > 10000? 否
  ├─ FPS 分类: Extreme (2000-10000 fps)
  └─ ✅ 有效，继续
  ↓
【优先级4】编码效率调整
  ├─ 编码效率因子: 1.0 (H.264)
  ├─ FPS因子: 3000/30 = 100
  ├─ 期望最小比特率: 2500 × 1.0 × 100 × 1.0 = 250000 kbps
  └─ 如果实际 < 250000 kbps → Optional/Recommended
  ↓
✅ 继续处理
```

## 🚀 使用建议

1. **古老编解码器** → 优先处理，收益最大
2. **现代编解码器** → 谨慎处理，可能质量损失
3. **高FPS视频** → 正常处理，FPS 分类已支持
4. **HDR内容** → 自动检测，保留色彩空间和位深度
5. **已高度压缩** → 可选处理，收益有限
