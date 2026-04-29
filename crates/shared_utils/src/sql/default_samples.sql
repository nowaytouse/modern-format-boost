-- V7 Seed Samples - 边界逻辑引导数据集
-- 这些样本用于在数据库初始化时，为 KNN 引擎提供第一批“认知边界”。
-- 包含了：极简表情包、高帧率贴纸、高价值艺术循环、以及干扰项（短视频）。

INSERT INTO samples (
    file_hash, source_path, file_name, source_ext, 
    width, height, duration_secs, frame_count, file_size_bytes, fps,
    temporal_bpp, spatial_bpp, has_transparency, has_embedded_icc,
    is_meme_platform, is_human_semantic_name, is_native_gif, is_high_value_source,
    loss_tolerance, loop_verdict, labeled_by, aspect_ratio, total_pixels
) VALUES 
-- 1. 经典表情包边界 (LoopStrong): 短、快、体积小、低熵
(
    'seed_hash_001_meme_classic', '/seed/memes/cat_vibing.gif', 'cat_vibing.gif', 'gif',
    240, 240, 1.2, 18, 450000, 15.0,
    0.02, 3.5, false, false,
    true, true, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 57600
),

-- 2. 透明贴纸边界 (LoopStrong): 具有透明通道、通常是极短的动作
(
    'seed_hash_002_sticker_alpha', '/seed/stickers/sparkle_anim.gif', 'sparkle.gif', 'gif',
    128, 128, 0.8, 24, 120000, 30.0,
    0.01, 2.1, true, false,
    true, false, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 16384
),

-- 3. 高价值艺术循环 (LoopStrong): 高分辨率、有颜色配置文件(ICC)、艺术目录
(
    'seed_hash_003_art_loop', '/seed/gallery/pixel_city_night.gif', 'pixel_city.gif', 'webp',
    1080, 1080, 8.0, 192, 15000000, 24.0,
    0.08, 8.2, false, true,
    false, true, true, true,
    'low', 'LoopStrong', 'seed_v7', 1.0, 1166400
),

-- 4. 干扰边界: 看起来像循环的短视频 (LoopWeak): 实际上是录制的短片段，通常有音轨
(
    'seed_hash_004_short_video', '/seed/clips/garden_record.mp4', 'garden.mp4', 'mp4',
    1920, 1080, 4.5, 135, 8000000, 30.0,
    0.15, 12.5, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.777, 2073600
),

-- 5. 长视频边界 (LoopWeak): 时长超过阈值、高复杂度、非原生 GIF
(
    'seed_hash_005_long_content', '/seed/videos/vlog_segment.mov', 'vlog_01.mov', 'mov',
    1280, 720, 25.0, 750, 45000000, 30.0,
    0.25, 15.0, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.777, 921600
),

-- 6. 极端高频循环 (LoopStrong): 帧率极高但时长极短，常见于高品质动画贴纸
(
    'seed_hash_006_high_fps_sticker', '/seed/stickers/fast_spin.webp', 'fast_spin.webp', 'webp',
    512, 512, 0.5, 30, 800000, 60.0,
    0.05, 4.0, true, false,
    true, false, false, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 262144
),

-- 7. 静态感循环 (LoopStrong): 类似壁纸，变化极微小，bpp 极低
(
    'seed_hash_007_static_ambient', '/seed/gallery/lofi_study.gif', 'lofi_study.gif', 'gif',
    1280, 720, 15.0, 180, 12000000, 12.0,
    0.005, 5.5, false, true,
    false, true, true, true,
    'low', 'LoopStrong', 'seed_v7', 1.777, 921600
),

-- 8. 游戏精彩瞬间 (LoopStrong): 高帧率、高质量、循环录制
(
    'seed_hash_008_game_loop', '/seed/gaming/valorant_ace.mp4', 'ace_loop.mp4', 'mp4',
    1920, 1080, 5.0, 300, 12000000, 60.0,
    0.12, 10.0, false, false,
    false, true, false, true,
    'low', 'LoopStrong', 'seed_v7', 1.777, 2073600
),

-- 9. 低质量“糊”图表情 (LoopStrong): 极端压缩、大量噪点、高循环意图
(
    'seed_hash_009_deep_fried', '/seed/memes/fried_sponge.gif', 'sponge.gif', 'gif',
    150, 150, 0.5, 5, 50000, 10.0,
    0.05, 1.2, false, false,
    true, true, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 22500
),

-- 10. 电影电影感剪辑 (LoopWeak): 24fps、窄画幅、复杂光影
(
    'seed_hash_010_movie_clip', '/seed/clips/noir_scene.mp4', 'noir.mp4', 'mp4',
    1920, 816, 12.0, 288, 18000000, 24.0,
    0.20, 14.5, false, false,
    false, true, false, false,
    'video', 'LoopWeak', 'seed_v7', 2.35, 1566720
),

-- 11. 社交媒体短片 (LoopWeak): 竖屏、带水印、时长尴尬 (3s)
(
    'seed_hash_011_tiktok_fail', '/seed/social/fail_clip.mp4', 'fail.mp4', 'mp4',
    720, 1280, 3.2, 96, 3000000, 30.0,
    0.18, 11.0, false, false,
    true, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 0.56, 921600
),

-- 12. 简约 Logo 动画 (LoopStrong): 纯色背景、矢量风格、极低 entropy
(
    'seed_hash_012_logo_anim', '/seed/branding/logo_spin.gif', 'logo.gif', 'gif',
    512, 512, 2.0, 60, 300000, 30.0,
    0.002, 0.8, true, false,
    false, true, true, true,
    'low', 'LoopStrong', 'seed_v7', 1.0, 262144
),

-- 13. Discord APNG 贴纸 (LoopStrong): 小型、高保真、透明
(
    'seed_hash_013_discord_sticker', '/seed/stickers/wave.png', 'wave.png', 'png',
    320, 320, 1.5, 45, 600000, 30.0,
    0.04, 3.2, true, false,
    true, false, false, true,
    'high', 'LoopStrong', 'seed_v7', 1.0, 102400
),

-- 14. 监控摄像头画面 (LoopWeak): 灰度、噪点多、低帧率、超长
(
    'seed_hash_014_cctv_feed', '/seed/security/cam_01.mp4', 'cam_01.mp4', 'mp4',
    640, 480, 60.0, 600, 5000000, 10.0,
    0.30, 4.5, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.33, 307200
),

-- 15. AI 生成幻觉循环 (LoopStrong): 画面闪烁、高频变化、但逻辑循环
(
    'seed_hash_015_ai_dream', '/seed/ai/mandelbrot.mp4', 'ai_dream.mp4', 'mp4',
    1024, 1024, 4.0, 120, 10000000, 30.0,
    0.12, 18.0, false, false,
    false, true, false, true,
    'low', 'LoopStrong', 'seed_v7', 1.0, 1048576
),

-- 16. 怀旧 VFR 动图 (LoopStrong): 帧率不稳、手动制作痕迹
(
    'seed_hash_016_vfr_retro', '/seed/retro/pixel_dance.gif', 'dance.gif', 'gif',
    100, 100, 2.5, 12, 150000, 4.8,
    0.03, 2.5, false, false,
    true, true, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 10000
),

-- 17. Cinemagraph 静态照片局部动 (LoopStrong): 绝大部分静态、局部循环
(
    'seed_hash_017_cinemagraph', '/seed/gallery/coffee_steam.gif', 'coffee.gif', 'gif',
    1920, 1080, 10.0, 240, 25000000, 24.0,
    0.001, 12.0, false, true,
    false, true, true, true,
    'low', 'LoopStrong', 'seed_v7', 1.777, 2073600
),

-- 18. 极长无声循环 (LoopStrong): 即使 20s 也是循环意图
(
    'seed_hash_018_long_ambient', '/seed/gallery/train_window.mp4', 'train.mp4', 'mp4',
    1280, 720, 22.0, 660, 35000000, 30.0,
    0.015, 9.0, false, false,
    false, true, false, true,
    'low', 'LoopStrong', 'seed_v7', 1.777, 921600
),

-- 19. 错误采集的网页背景 (LoopStrong): 极小、平铺、纯色变化
(
    'seed_hash_019_bg_tile', '/seed/web/bg_mesh.gif', 'bg.gif', 'gif',
    64, 64, 5.0, 150, 20000, 30.0,
    0.0005, 0.5, false, false,
    false, false, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 4096
),

-- 20. 快速预览缩略图 (LoopWeak): 这种虽然短且快，但不是循环
(
    'seed_hash_020_preview_strip', '/seed/system/preview.mp4', 'preview.mp4', 'mp4',
    320, 180, 2.0, 60, 150000, 30.0,
    0.35, 6.0, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.777, 57600
),

-- 21. 慢动作艺术 (LoopStrong): 高帧率压制、极慢速循环
(
    'seed_hash_021_slow_mo_art', '/seed/art/ink_drop.mp4', 'ink.mp4', 'mp4',
    2160, 2160, 10.0, 600, 55000000, 60.0,
    0.05, 22.0, false, true,
    false, true, false, true,
    'low', 'LoopStrong', 'seed_v7', 1.0, 4665600
),

-- 22. 手机实况照片导出 (LoopWeak): 带有典型的人物晃动和背景杂音
(
    'seed_hash_022_live_photo', '/seed/photos/IMG_4562.mov', 'IMG_4562.mov', 'mov',
    1440, 1080, 3.0, 90, 5000000, 30.0,
    0.22, 13.0, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.33, 1555200
),

-- 23. PPT 翻页导出 (LoopWeak): 画面长期静止，突然剧烈变化
(
    'seed_hash_023_ppt_export', '/seed/docs/presentation.mp4', 'slides.mp4', 'mp4',
    1920, 1080, 45.0, 45, 2000000, 1.0,
    0.005, 10.0, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.777, 2073600
),

-- 24. 极简像素贴纸 (LoopStrong): 只有几颗像素动
(
    'seed_hash_024_pixel_star', '/seed/stickers/star.gif', 'star.gif', 'gif',
    16, 16, 1.0, 4, 2000, 4.0,
    0.001, 0.2, true, false,
    true, true, true, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 256
),

-- 25. 录屏教程片段 (LoopWeak): 鼠标移动、菜单弹出
(
    'seed_hash_025_screen_record', '/seed/tutorial/click_demo.mp4', 'click.mp4', 'mp4',
    2560, 1440, 15.0, 450, 12000000, 30.0,
    0.04, 18.0, false, false,
    false, false, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.777, 3686400
),

-- 26. Animated WebP disguised as static (LoopStrong): VP8X header, shallow probe returns WebpStatic
(
    'seed_hash_026_anim_webp_disguised', '/seed/stickers/emoji_dance.webp', 'emoji_dance.webp', 'webp',
    480, 480, 2.0, 48, 950000, 24.0,
    0.04, 5.5, true, false,
    true, true, false, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 230400
),

-- 27. Animated AVIF sticker (LoopStrong): modern format that can be animated
(
    'seed_hash_027_avif_animated', '/seed/stickers/heart_pulse.avif', 'heart.avif', 'avif',
    256, 256, 1.5, 36, 180000, 24.0,
    0.03, 3.0, true, false,
    true, false, false, true,
    'high', 'LoopStrong', 'seed_v7', 1.0, 65536
),

-- 28. HEIC animated burst (LoopStrong): Apple HEIC sequence
(
    'seed_hash_028_heic_burst', '/seed/apple/burst_firework.heic', 'firework.heic', 'heic',
    1920, 1080, 3.0, 72, 8000000, 24.0,
    0.06, 9.0, false, false,
    false, true, false, true,
    'low', 'LoopStrong', 'seed_v7', 1.777, 2073600
),

-- 29. APNG masquerading as .png (LoopStrong): animated PNG with .png extension
(
    'seed_hash_029_apng_as_png', '/seed/stickers/wink.png', 'wink.png', 'png',
    200, 200, 1.0, 20, 400000, 20.0,
    0.02, 2.8, true, false,
    true, false, false, false,
    'high', 'LoopStrong', 'seed_v7', 1.0, 40000
),

-- 30. Large WebP with VP8X extended header (LoopWeak): high-res static WebP misidentified by shallow probe
(
    'seed_hash_030_webp_vp8x_large', '/seed/photos/landscape_hdr.webp', 'landscape.webp', 'webp',
    4032, 3024, 0.0, 1, 4500000, 0.0,
    0.0, 2.8, false, true,
    false, true, false, false,
    'video', 'LoopWeak', 'seed_v7', 1.33, 12192768
)
ON CONFLICT (file_hash) DO NOTHING;

