<template>
  <div class="checkin-log">
    <!-- 工具栏 -->
    <div class="toolbar">
      <div class="toolbar-left">
        <div class="filter-tabs">
          <button
            class="filter-tab"
            :class="{ active: filter === 'all' }"
            @click="filter = 'all'"
          >
            全部
            <span class="count">{{ store.logs.length }}</span>
          </button>
          <button
            class="filter-tab"
            :class="{ active: filter === 'success' }"
            @click="filter = 'success'"
          >
            成功
            <span class="count success">{{ successCount }}</span>
          </button>
          <button
            class="filter-tab"
            :class="{ active: filter === 'already' }"
            @click="filter = 'already'"
          >
            已签到
            <span class="count already">{{ alreadyCount }}</span>
          </button>
          <button
            class="filter-tab"
            :class="{ active: filter === 'failed' }"
            @click="filter = 'failed'"
          >
            失败
            <span class="count danger">{{ failedCount }}</span>
          </button>
        </div>
      </div>
      <div class="toolbar-right">
        <button class="btn btn-sm btn-outline" @click="handleRefresh">
          <Icon name="arrow-path" :size="14" />
          <span>刷新</span>
        </button>
        <button
          class="btn btn-sm btn-outline danger"
          @click="handleClear"
          :disabled="store.logs.length === 0"
        >
          <Icon name="trash" :size="14" />
          <span>清空日志</span>
        </button>
      </div>
    </div>

    <!-- 日志列表 -->
    <div class="log-list card" v-if="filteredLogs.length > 0">
      <div class="log-table">
        <div class="log-header">
          <div class="col-time">时间</div>
          <div class="col-account">账号</div>
          <div class="col-result">结果</div>
          <div class="col-message">详情</div>
          <div class="col-points">可用积分</div>
        </div>
        <div class="log-body">
          <div
            class="log-row"
            v-for="log in filteredLogs"
            :key="log.id"
          >
            <div class="col-time">{{ formatTime(log.time) }}</div>
            <div class="col-account">
              <span class="account-tag">{{ log.accountName }}</span>
            </div>
            <div class="col-result">
              <span class="badge" :class="statusBadge(log.result)">
                {{ statusText(log.result) }}
              </span>
            </div>
            <div class="col-message">
              <span class="message-text" :title="log.message">
                {{ log.message }}
              </span>
            </div>
            <div class="col-points">
              <template v-if="log.pointsBalance != null">
                <span
                  class="points-balance"
                  :title="`签到后可用积分(usage_summary)`"
                >{{ log.pointsBalance }}</span>
              </template>
              <span v-else class="points-none">-</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 空状态 -->
    <div class="empty-state" v-else>
      <Icon name="document-text" :size="48" class="empty-state-icon" />
      <p class="empty-state-text">暂无签到记录</p>
      <p class="empty-state-desc">执行签到后会在这里显示记录</p>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useAppStore } from '../stores/app'
import Icon from './Icon.vue'

const store = useAppStore()
const filter = ref<'all' | 'success' | 'already' | 'failed'>('all')

const filteredLogs = computed(() => {
  if (filter.value === 'all') return store.logs
  return store.logs.filter(l => l.result === filter.value)
})

const successCount = computed(() => store.logs.filter(l => l.result === 'success').length)
const alreadyCount = computed(() => store.logs.filter(l => l.result === 'already').length)
const failedCount = computed(() => store.logs.filter(l => l.result === 'failed').length)

function statusText(result: string): string {
  if (result === 'success') return '成功'
  if (result === 'already') return '已签到'
  return '失败'
}

function statusBadge(result: string): string {
  if (result === 'success') return 'badge-success'
  if (result === 'already') return 'badge-info'
  return 'badge-danger'
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp)
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')
  return `${month}-${day} ${hours}:${minutes}:${seconds}`
}

async function handleRefresh() {
  await store.fetchLogs()
}

async function handleClear() {
  if (confirm('确定要清空所有签到日志吗？此操作不可恢复。')) {
    await store.clearLogs()
  }
}

onMounted(() => {
  store.fetchLogs()
})
</script>

<style scoped>
.checkin-log {
  max-width: 100%;
}

.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
}

/* 过滤标签栏 - 凹槽容器,激活项凸起 + 强调色文字 */
.filter-tabs {
  display: flex;
  gap: 4px;
  padding: 5px;
  background: var(--surface);
  border: none;
  border-radius: var(--r-md);
  box-shadow: var(--shadow-soft-inset);
}

.filter-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 14px;
  border: none;
  background: transparent;
  border-radius: var(--r-sm);
  cursor: pointer;
  font-family: inherit;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-secondary);
  transition: all var(--t-smooth);
}

.filter-tab:hover {
  color: var(--text-primary);
}

.filter-tab.active {
  background: var(--surface);
  color: var(--accent);
  box-shadow: var(--shadow-soft-raised);
}

.filter-tab .count {
  font-size: 11px;
  padding: 1px 7px;
  border-radius: var(--r-pill);
  background: var(--surface);
  box-shadow: var(--shadow-soft-flat);
  color: var(--text-muted);
  font-weight: 700;
}

.filter-tab.active .count {
  color: var(--accent);
}

.filter-tab .count.success {
  color: var(--success);
}

.filter-tab .count.already {
  color: var(--accent);
}

.filter-tab .count.danger {
  color: var(--danger);
}

.toolbar-right {
  display: flex;
  gap: 8px;
}

.btn-outline.danger {
  color: var(--danger);
}

.btn-outline.danger:hover:not(:disabled) {
  color: var(--danger);
  box-shadow: var(--shadow-soft-raised);
}

.btn-outline.danger:active:not(:disabled) {
  box-shadow: var(--shadow-soft-inset);
}

.log-list {
  overflow: hidden;
}

.log-table {
  width: 100%;
}

/* 表头 - 凹陷背景 */
.log-header {
  display: flex;
  padding: 12px 16px;
  background: var(--surface-deep);
  border: none;
  box-shadow: inset 0 -2px 4px var(--shadow-light);
  font-size: 12px;
  font-weight: 700;
  color: var(--text-secondary);
  letter-spacing: 0.04em;
}

.log-body {
  max-height: calc(100vh - 280px);
  overflow-y: auto;
}

.log-row {
  display: flex;
  padding: 12px 16px;
  border: none;
  font-size: 13px;
  transition: background var(--t-smooth);
}

.log-row:not(:last-child) {
  box-shadow: inset 0 -1px 2px var(--shadow-light);
}

.log-row:hover {
  background: var(--surface-deep);
}

.col-time {
  width: 140px;
  flex-shrink: 0;
  color: var(--text-secondary);
  font-family: 'SF Mono', Monaco, monospace;
  font-size: 12px;
}

.col-account {
  width: 120px;
  flex-shrink: 0;
}

.account-tag {
  display: inline-block;
  padding: 2px 9px;
  background: var(--surface);
  color: var(--accent);
  border: none;
  border-radius: var(--r-sm);
  font-size: 12px;
  font-weight: 600;
  box-shadow: var(--shadow-soft-flat);
}

.col-result {
  width: 80px;
  flex-shrink: 0;
}

.col-message {
  flex: 1;
  min-width: 0;
  padding-right: 16px;
}

.message-text {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
}

.col-points {
  width: 110px;
  flex-shrink: 0;
  text-align: right;
}

.points-balance {
  color: var(--warning);
  font-weight: 700;
}

.points-none {
  color: var(--text-muted);
}
</style>
