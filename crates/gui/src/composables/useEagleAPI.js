/**
 * Eagle API 集成 Composable
 */

import { ref } from "vue";
import { logger, LOG_KEYS } from "../utils/logger";

export function useEagleAPI() {
  const isAvailable = ref(false);
  const items = ref([]);

  /**
   * 检测 Eagle API
   */
  const detect = () => {
    isAvailable.value = typeof window.eagle !== "undefined";
    logger.info(LOG_KEYS.EAGLE_API_CALL, "Eagle API detection", {
      available: isAvailable.value,
      mode: isAvailable.value ? "production" : "development",
    });
    return isAvailable.value;
  };

  /**
   * 获取选中的文件
   */
  const getSelectedItems = async () => {
    if (!isAvailable.value) {
      // 🔥 Mock 数据 - 包含图像、动图、视频
      return [
        {
          id: "1",
          name: "photo-1.jpg",
          ext: "jpg",
          filePath: "/mock/photo-1.jpg",
          path: "/mock/photo-1.jpg",
          thumbnail: "https://picsum.photos/200/200?random=1",
          width: 1920,
          height: 1080,
          size: 2048576,
        },
        {
          id: "2",
          name: "photo-2.png",
          ext: "png",
          filePath: "/mock/photo-2.png",
          path: "/mock/photo-2.png",
          thumbnail: "https://picsum.photos/200/200?random=2",
          width: 1280,
          height: 720,
          size: 1048576,
        },
        {
          id: "3",
          name: "animation.gif",
          ext: "gif",
          filePath: "/mock/animation.gif",
          path: "/mock/animation.gif",
          thumbnail: "https://picsum.photos/200/200?random=3",
          width: 800,
          height: 600,
          size: 5242880, // 5MB - 大型动图
        },
        {
          id: "4",
          name: "video-sample.mp4",
          ext: "mp4",
          filePath: "/mock/video-sample.mp4",
          path: "/mock/video-sample.mp4",
          thumbnail: "https://picsum.photos/200/200?random=4",
          width: 1920,
          height: 1080,
          size: 10485760, // 10MB
        },
        {
          id: "5",
          name: "animated-icon.apng",
          ext: "apng",
          filePath: "/mock/animated-icon.apng",
          path: "/mock/animated-icon.apng",
          thumbnail: "https://picsum.photos/200/200?random=5",
          width: 512,
          height: 512,
          size: 1048576,
        },
        {
          id: "6",
          name: "screen-record.mov",
          ext: "mov",
          filePath: "/mock/screen-record.mov",
          path: "/mock/screen-record.mov",
          thumbnail: "https://picsum.photos/200/200?random=6",
          width: 2560,
          height: 1440,
          size: 20971520, // 20MB
        },
      ];
    }

    try {
      const selected = await window.eagle.item.getSelected();
      items.value = selected.map((item) => {
        // 🔥 处理缩略图路径 (使用thumbnailURL而不是thumbnail)
        let thumbnail = null;
        if (item.thumbnailURL) {
          // 如果不是http或file://开头，添加file://前缀
          if (
            !item.thumbnailURL.startsWith("http") &&
            !item.thumbnailURL.startsWith("file://")
          ) {
            thumbnail = `file://${item.thumbnailURL}`;
          } else {
            thumbnail = item.thumbnailURL;
          }
        }

        return {
          id: item.id,
          name: item.name,
          // 🔧 Bug Fix: 清理扩展名中可能存在的引号
          ext: (item.ext || "").replace(/^["']|["']$/g, "").toLowerCase(),
          filePath: item.filePath,
          path: item.filePath, // 添加path别名
          thumbnail: thumbnail, // 使用处理后的缩略图路径
          width: item.width || 0,
          height: item.height || 0,
          size: item.size || 0,
        };
      });

      logger.info(LOG_KEYS.EAGLE_API_CALL, "Loaded Eagle files", {
        count: items.value.length,
        hasThumbnails: items.value.filter((i) => i.thumbnail).length,
      });

      return items.value;
    } catch (err) {
      logger.error(LOG_KEYS.EAGLE_API_ERROR, "Failed to get Eagle files", {
        error: err.message,
      });
      throw err;
    }
  };

  return {
    isAvailable,
    items,
    detect,
    getSelectedItems,
  };
}
