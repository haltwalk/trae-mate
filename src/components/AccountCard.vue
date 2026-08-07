<template>
  <div class="account-card card" :class="{ disabled: !account.enabled }">
    <!-- 卡片头部 -->
    <div class="card-header">
      <div class="account-info">
        <div class="avatar">{{ account.name.charAt(0).toUpperCase() }}</div>
        <div class="account-name-wrap">
          <h3 class="account-name">{{ account.name }}</h3>
          <span class="badge" :class="statusBadgeClass">
            {{ statusText }}
          </span>
        </div>
      </div>
      <div class="switch" :class="{ active: account.enabled }" @click="handleToggle">
      </div>
    </div>

    <!-- 卡片内容 -->
    <div class="card-body">
      <div class="info-row">
        <span class="info-label">上次签到</span>
        <span class="info-value">{{ lastCheckinText }}</span>
      </div>
      <div class="info-row">
        <span class="info-label">总积分</span>
        <span class="info-value points">
          {{ account.points || 0 }} 分
        </span>
      </div>
      <div class="info-row">
        <span class="info-label">添加时间</span>
        <span class="info-value">{{ formatDate(account.createdAt) }}</span>
      </div>
      <div class="info-row" v-if="account.credentialStatus">
        <span class="info-label">桌面凭证</span>
        <span class="info-value" :class="account.credentialStatus === 'expired' ? 'text-danger' : 'text-success'">
          {{ credentialStatusText }}
        </span>
      </div>
    </div>

    <!-- 卡片底部 -->
    <div class="card-footer">
      <button
        class="btn btn-sm btn-primary"
        @click="handleCheckin"
        :disabled="checkingIn || !account.enabled"
      >
        <span v-if="checkingIn" class="spinner"></span>
        <span>{{ checkingIn ? '签到中' : '立即签到' }}</span>
      </button>
      <button
        v-if="!instanceRunning"
        class="btn btn-sm btn-outline"
        @click="handleLaunchMulti"
        :disabled="launching"
        :title="account.dataDir ? '启动该账号的多开实例' : '首次多开:创建独立实例并启动'"
      >
        <span v-if="launching" class="spinner"></span>
        <span>{{ launching ? '程序正在启动,请稍后' : '多开' }}</span>
      </button>
      <button
        v-else
        class="btn btn-sm btn-outline"
        @click="handleFocus"
        title="该账号实例已运行,点击聚焦其窗口"
      >
        <span>聚焦</span>
      </button>
      <button type="button" class="btn btn-sm btn-outline" @click.stop="startEdit">
        编辑
      </button>
      <button class="btn btn-sm btn-outline danger" @click="handleDelete">
        删除
      </button>
    </div>
  </div>
  <div v-if="editing" class="edit-overlay" @click.self="cancelEdit">
    <div class="edit-dialog">
      <h3>编辑账号名称</h3>
      <input v-model="draftName" class="input" autofocus @keyup.enter="saveEdit" @keyup.esc="cancelEdit" />
      <div class="edit-actions"><button class="btn btn-sm btn-outline" @click="cancelEdit">取消</button><button class="btn btn-sm btn-primary" @click="saveEdit">保存</button></div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useAppStore, type Account } from '../stores/app'

const props = defineProps<{
  account: Account
}>()

const emit = defineEmits(['checkin', 'toggle', 'delete', 'edit', 'notify'])
const store = useAppStore()

const checkingIn = ref(false)
const launching = ref(false)
const instanceRunning = ref(false)

onMounted(async () => {
  instanceRunning.value = await store.isInstanceRunning(props.account.id)
})

// 监听全局刷新信号(顶部"刷新"按钮触发),重新查询本账号多开实例运行状态
watch(() => store.instanceRefreshTick, async () => {
  instanceRunning.value = await store.isInstanceRunning(props.account.id)
})
const editing = ref(false)
const draftName = ref(props.account.name)

const statusText = computed(() => {
  if (!props.account.enabled) return '已停用'
  if (!props.account.lastCheckinAt) return '未签到'

  const today = new Date().toDateString()
  const lastDate = new Date(props.account.lastCheckinAt).toDateString()

  if (lastDate === today) {
    return props.account.lastCheckinResult === 'success' ? '今日已签' : '今日失败'
  }
  return '待签到'
})

const statusBadgeClass = computed(() => {
  if (!props.account.enabled) return 'badge-muted'
  if (!props.account.lastCheckinAt) return 'badge-warning'

  const today = new Date().toDateString()
  const lastDate = new Date(props.account.lastCheckinAt).toDateString()

  if (lastDate === today) {
    return props.account.lastCheckinResult === 'success' ? 'badge-success' : 'badge-danger'
  }
  return 'badge-info'
})

const lastCheckinText = computed(() => {
  if (!props.account.lastCheckinAt) return '从未签到'
  return formatDateTime(props.account.lastCheckinAt)
})

const credentialStatusText = computed(() => {
  if (props.account.credentialStatus === 'expired') return '凭证已失效，请重新导入'
  if (props.account.credentialStatus === 'expiring') return '凭证即将续期'
  return '凭证有效'
})

function handleToggle() {
  emit('toggle', props.account.id, !props.account.enabled)
}

async function handleCheckin() {
  checkingIn.value = true
  try {
    emit('checkin', props.account.id)
  } finally {
    setTimeout(() => {
      checkingIn.value = false
    }, 1000)
  }
}

async function handleLaunchMulti() {
  launching.value = true
  try {
    await store.launchMulti(props.account.id)
    instanceRunning.value = true
    emit('notify', '多开实例已启动', 'success')
  } catch (e: any) {
    emit('notify', '多开启动失败: ' + (e?.message || e), 'error')
  } finally {
    launching.value = false
  }
}

async function handleFocus() {
  try {
    await store.focusInstance(props.account.id)
  } catch (e: any) {
    emit('notify', '聚焦失败: ' + (e?.message || e), 'error')
  }
}

function handleDelete() {
  emit('delete', props.account.id)
}

function startEdit() { draftName.value = props.account.name; editing.value = true }
function cancelEdit() { editing.value = false }
function saveEdit() { const name = draftName.value.trim(); if (name && name !== props.account.name) emit('edit', props.account.id, name); editing.value = false }


function formatDate(timestamp: number): string {
  const date = new Date(timestamp)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

function formatDateTime(timestamp: number): string {
  const date = new Date(timestamp)
  const now = new Date()
  const diff = now.getTime() - date.getTime()

  if (diff < 60000) return '刚刚'
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`

  return `${date.getMonth() + 1}/${date.getDate()} ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}
</script>

<style scoped>
/* 账号卡片 - 凸起,hover 上浮 */
.account-card {
  padding: 20px;
  transition: box-shadow var(--t-smooth), transform var(--t-smooth);
  background: var(--surface);
  border: none;
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-soft-raised);
}

.account-card:hover {
  box-shadow: var(--shadow-md-raised);
  transform: translateY(-3px);
}

.account-card.disabled {
  opacity: 0.55;
}

.card-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  margin-bottom: 16px;
}

.account-info {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: 1;
  min-width: 0;
}

/* 头像 - 强调色填充 + 白字(品牌标识,小面积) */
.avatar {
  width: 44px;
  height: 44px;
  border-radius: 14px;
  background: var(--accent);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 18px;
  font-weight: 700;
  font-family: 'Nunito', 'Inter', sans-serif;
  box-shadow: var(--shadow-soft-flat), var(--accent-glow);
  flex-shrink: 0;
}

.account-name-wrap {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.account-name {
  font-size: 15px;
  font-weight: 700;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.badge {
  width: fit-content;
  align-self: flex-start;
  white-space: nowrap;
}

/* 卡片内容区 - 纯靠间距与底部按钮区分隔,不用边框线(Neumorphism 无 border) */
.card-body {
  margin-bottom: 18px;
}

.info-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 0;
}

.info-label {
  font-size: 13px;
  color: var(--text-secondary);
}

.info-value {
  font-size: 13px;
  color: var(--text-primary);
  font-weight: 600;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.info-value.points {
  color: var(--warning);
  font-weight: 700;
}

.text-success {
  color: var(--success);
}

.text-danger {
  color: var(--danger);
}

.card-footer {
  display: flex;
  gap: 8px;
}

.card-footer .btn {
  flex: 1;
}

/* 危险轮廓按钮 - neu 版 */
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

/* 编辑弹窗 */
.edit-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(15, 23, 42, 0.35);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
}

.edit-dialog {
  width: 360px;
  padding: 22px;
  border-radius: var(--r-lg);
  background: var(--surface);
  border: none;
  box-shadow: var(--shadow-bold-raised);
}

.edit-dialog h3 {
  font-size: 16px;
  font-weight: 700;
  color: var(--text-primary);
}

.edit-dialog .input {
  width: 100%;
  box-sizing: border-box;
  margin: 16px 0;
}

.edit-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
