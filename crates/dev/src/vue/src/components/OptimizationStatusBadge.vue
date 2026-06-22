<template>
  <div
    class="optimization-badge"
    :class="badgeClass"
  >
    <span class="icon">{{ statusIcon }}</span>
    <span class="text">{{ statusText }}</span>
    <span
      v-if="showSavings && savings !== null"
      class="savings"
    >
      {{ formatSavings(savings) }}
    </span>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const properties = defineProps({
  status: {
    type: String,
    required: true,
    validator: (value: string) => ['optimal', 'minor', 'significant', 'critical', 'unknown'].includes(value)
  },
  savings: {
    type: Number,
    default: null
  },
  showSavings: {
    type: Boolean,
    default: true
  }
})

type StatusType = 'optimal' | 'minor' | 'significant' | 'critical' | 'unknown'

const statusConfig: Record<StatusType, { icon: string; text: string; class: string }> = {
  optimal: {
    icon: '✅',
    text: '已优化',
    class: 'badge-optimal'
  },
  minor: {
    icon: '👍',
    text: '可选优化',
    class: 'badge-minor'
  },
  significant: {
    icon: '⚠️',
    text: '建议优化',
    class: 'badge-significant'
  },
  critical: {
    icon: '❌',
    text: '必须优化',
    class: 'badge-critical'
  },
  unknown: {
    icon: '❓',
    text: '未分析',
    class: 'badge-unknown'
  }
}

const config = computed(() => statusConfig[properties.status as StatusType])
const statusIcon = computed(() => config.value.icon)
const statusText = computed(() => config.value.text)
const badgeClass = computed(() => config.value.class)

const formatSavings = (percent: number | null | undefined) => {
  if (percent === null || percent === undefined) return ''
  if (percent < 1) return '<1%↓'
  return `${String(Math.round(percent))}%↓`
}
</script>

<style scoped>
.optimization-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  transition: all 0.2s ease;
}

.optimization-badge:hover {
  transform: translateY(-1px);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
}

.badge-optimal {
  background: linear-gradient(135deg, #10b981, #059669);
  color: white;
  box-shadow: 0 2px 4px rgba(16, 185, 129, 0.2);
}

.badge-minor {
  background: linear-gradient(135deg, #3b82f6, #2563eb);
  color: white;
  box-shadow: 0 2px 4px rgba(59, 130, 246, 0.2);
}

.badge-significant {
  background: linear-gradient(135deg, #f59e0b, #d97706);
  color: white;
  box-shadow: 0 2px 4px rgba(245, 158, 11, 0.2);
}

.badge-critical {
  background: linear-gradient(135deg, #ef4444, #dc2626);
  color: white;
  box-shadow: 0 2px 4px rgba(239, 68, 68, 0.2);
}

.badge-unknown {
  background: linear-gradient(135deg, #6b7280, #4b5563);
  color: white;
  box-shadow: 0 2px 4px rgba(107, 114, 128, 0.2);
}

.icon {
  font-size: 14px;
  line-height: 1;
}

.text {
  font-size: 11px;
  letter-spacing: 0.3px;
}

.savings {
  font-size: 11px;
  font-weight: 700;
  opacity: 0.95;
  padding-left: 4px;
  border-left: 1px solid rgba(255, 255, 255, 0.3);
}

/* 深色模式支持 */
@media (prefers-color-scheme: dark) {
  .optimization-badge:hover {
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
  }
}
</style>
