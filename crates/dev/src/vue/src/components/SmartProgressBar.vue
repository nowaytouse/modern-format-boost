<template>
  <div class="smart-progress-container">
    <div class="progress-track glass">
      <div 
        class="progress-fill" 
        :style="{ width: `${visualProgress}%` }"
        :class="[status, { 'pulse': isProcessing }]"
      >
        <!-- 动态光效 -->
        <div class="progress-shimmer" />
        <!-- 前端高亮 -->
        <div class="progress-head" />
      </div>
    </div>
    
    <div class="progress-meta">
      <div class="progress-text">
        <span
          v-if="statusText"
          class="status-text"
        >{{ statusText }}</span>
        <span class="percentage">{{ Math.round(visualProgress) }}%</span>
      </div>
      <div
        v-if="details"
        class="progress-details"
      >
        <span
          v-if="speed"
          class="speed"
        >{{ speed }}</span>
        <span
          v-if="eta"
          class="eta"
        >ETA: {{ eta }}</span>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, watch, computed, onMounted, onUnmounted } from 'vue';

const properties = defineProps({
  progress: {
    type: Number,
    default: 0
  },
  status: {
    type: String,
    default: 'idle' // idle, processing, success, error, paused
  },
  statusText: {
    type: String,
    default: ''
  },
  speed: {
    type: String,
    default: ''
  },
  eta: {
    type: String,
    default: ''
  },
  // 是否启用智能平滑 (Zeno mode)
  smartSmoothing: {
    type: Boolean,
    default: true
  }
});

const visualProgress = ref(0);
const animationFrame = ref(null);

const isProcessing = computed(() => properties.status === 'processing');

// 智能平滑算法
const updateVisualProgress = () => {
  if (!properties.smartSmoothing) {
    visualProgress.value = properties.progress;
    return;
  }

  const target = properties.progress;
  const current = visualProgress.value;
  const diff = target - current;

  if (Math.abs(diff) < 0.1) {
    visualProgress.value = target;
  } else {
    // 物理惯性模拟：距离越远速度越快，但有最大限制
    // 使用 lerp (线性插值) 实现平滑逼近
    // factor 0.1 意味着每帧移动差距的 10%
    const factor = 0.1; 
    visualProgress.value = current + diff * factor;
  }

  if (properties.status === 'processing' || Math.abs(target - visualProgress.value) > 0.1) {
    animationFrame.value = requestAnimationFrame(updateVisualProgress);
  }
};

watch(() => properties.progress, (newValue) => {
  if (properties.smartSmoothing) {
    cancelAnimationFrame(animationFrame.value);
    updateVisualProgress();
  } else {
    visualProgress.value = newValue;
  }
});

onMounted(() => {
  visualProgress.value = properties.progress;
  if (properties.smartSmoothing && properties.status === 'processing') {
    updateVisualProgress();
  }
});

onUnmounted(() => {
  cancelAnimationFrame(animationFrame.value);
});
</script>

<style scoped>
.smart-progress-container {
  width: 100%;
  position: relative;
}

.progress-track {
  height: 8px;
  background: rgba(0, 0, 0, 0.3);
  border-radius: 4px;
  overflow: hidden;
  position: relative;
  box-shadow: inset 0 1px 2px rgba(0,0,0,0.2);
}

.progress-fill {
  height: 100%;
  background: var(--gradient-primary);
  border-radius: 4px;
  position: relative;
  width: 0;
  transition: width 0.1s linear; /* 配合JS插值，使用极短的transition防止抖动 */
}

/* 状态颜色 */
.progress-fill.success { background: var(--color-success); box-shadow: 0 0 10px var(--color-success); }
.progress-fill.error { background: var(--color-danger); box-shadow: 0 0 10px var(--color-danger); }
.progress-fill.paused { background: var(--color-warning); filter: grayscale(0.5); }

/* 动态光效 (Shimmer) */
.progress-shimmer {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  width: 100%;
  background: linear-gradient(
    90deg,
    transparent 0%,
    rgba(255, 255, 255, 0.2) 25%,
    rgba(255, 255, 255, 0.5) 50%,
    rgba(255, 255, 255, 0.2) 75%,
    transparent 100%
  );
  background-size: 200% 100%;
  animation: shimmer 2s infinite linear;
  opacity: 0.6;
}

@keyframes shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

/* 前端高亮 (Head) */
.progress-head {
  position: absolute;
  right: 0;
  top: 0;
  bottom: 0;
  width: 4px;
  background: #fff;
  box-shadow: 0 0 10px #fff, 0 0 20px var(--color-primary);
  opacity: 0.8;
  border-radius: 0 4px 4px 0;
}

.progress-meta {
  display: flex;
  justify-content: space-between;
  margin-top: 6px;
  font-size: 11px;
  color: var(--text-secondary);
  font-weight: 500;
}

.progress-text {
  display: flex;
  gap: 8px;
}

.progress-details {
  display: flex;
  gap: 12px;
}

.speed {
  color: var(--text-dim);
}

.eta {
  color: var(--color-primary);
}
</style>
