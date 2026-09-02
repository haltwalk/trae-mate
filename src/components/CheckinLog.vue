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
        <button class="btn btn-sm btn-outline" @click="handleExport">
          <Icon name="download" :size="14" />
          <span>导出日志</span>
        </button>
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
              <span
                class="message-text"
                :title="`${log.errorCode != null ? `[${log.errorCode}] ` : ''}${log.message}`"
                @click="openDetail(log)"
              >
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

    <!-- 日志详情弹层 -->
    <Teleport to="body">
      <div v-if="detail" class="detail-mask" @click.self="detail = null">
        <div class="detail-card card">
          <div class="detail-head">
            <span class="detail-title">签到日志详情</span>
            <button class="detail-close" @click="detail = null">
              <Icon name="x-mark" :size="16" />
            </button>
          </div>
          <div class="detail-body" v-if="detail">
            <div class="detail-row">
              <span class="detail-label">账号</span>
              <span class="detail-value">{{ detail.accountName }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">时间</span>
              <span class="detail-value">{{ formatTime(detail.time) }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">结果</span>
              <span class="detail-value">
                <span class="badge" :class="statusBadge(detail.result)">
                  {{ statusText(detail.result) }}
                </span>
              </span>
            </div>
            <div class="detail-row">
              <span class="detail-label">错误码</span>
              <span class="detail-value">
                <template v-if="detail.errorCode != null">{{ detail.errorCode }}</template>
                <template v-else>-</template>
              </span>
            </div>
            <div class="detail-row">
              <span class="detail-label">详情信息</span>
              <span class="detail-value detail-msg">{{ detail.message }}</span>
            </div>
            <div class="detail-row" v-if="detail.pointsGained != null">
              <span class="detail-label">本次获得</span>
              <span class="detail-value">{{ detail.pointsGained }} 分</span>
            </div>
            <div class="detail-row" v-if="detail.pointsBalance != null">
              <span class="detail-label">可用余额</span>
              <span class="detail-value">{{ detail.pointsBalance }} 分</span>
            </div>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useAppStore } from '../stores/app'
import type { CheckinLog } from '../stores/app'
import Icon from './Icon.vue'

const store = useAppStore()
const emit = defineEmits(['notify'])
const filter = ref<'all' | 'success' | 'already' | 'failed'>('all')
const detail = ref<CheckinLog | null>(null)

function openDetail(log: CheckinLog) {
  detail.value = log
}

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

const exporting = ref(false)

async function handleExport() {
  if (exporting.value) return
  exporting.value = true
  try {
    const now = new Date()
    const stamp = `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}-${String(now.getHours()).padStart(2, '0')}${String(now.getMinutes()).padStart(2, '0')}`
    const name = `trae-checkin-export-${stamp}.json`
    const path = await store.exportDiagnostics(name)
    if (path) {
      emit('notify', `诊断数据已导出: ${path}`, 'success')
    }
  } catch (e: any) {
    emit('notify', '导出失败: ' + (e?.message || e), 'error')
  } finally {
    exporting.value = false
  }
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
  cursor: pointer;
}

.message-text:hover {
  color: var(--accent);
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

/* 日志详情弹层 */
.detail-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(2px);
  padding: 24px;
}

.detail-card {
  width: 460px;
  max-width: 100%;
  border-radius: var(--r-lg);
}

.detail-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border: none;
  box-shadow: inset 0 -2px 4px var(--shadow-light);
}

.detail-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--text-primary);
}

.detail-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: none;
  background: transparent;
  border-radius: var(--r-sm);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all var(--t-smooth);
}

.detail-close:hover {
  background: var(--surface-deep);
  color: var(--text-primary);
}

.detail-body {
  padding: 8px 18px 16px;
}

.detail-row {
  display: flex;
  gap: 12px;
  padding: 9px 0;
  border: none;
  box-shadow: inset 0 -1px 2px var(--shadow-light);
}

.detail-row:last-child {
  box-shadow: none;
}

.detail-label {
  width: 68px;
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-secondary);
  padding-top: 1px;
}

.detail-value {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  color: var(--text-primary);
  word-break: break-word;
}

.detail-msg {
  line-height: 1.5;
}
</style>
