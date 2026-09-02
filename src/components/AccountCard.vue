<template>
  <div class="account-card card" :class="{ disabled: !account.enabled }">
    <!-- 卡片头部 -->
    <div class="card-header">
      <div class="account-info">
        <div class="avatar">{{ account.name.charAt(0).toUpperCase() }}</div>
        <div class="account-name-wrap">
          <h3 class="account-name">
            {{ account.name }}
            <span v-if="instanceState.isMainAccount" class="badge-main">主账号</span>
          </h3>
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
        <span
          class="info-value points"
          :title="pointsTooltip"
          :class="{ clickable: !!account.pointsResponse }"
          @click="toggleResp($event)"
        >
          {{ account.points || 0 }} 分
        </span>
      </div>
      <!-- 各类型可用积分余额明细:仅存在多类时才展示,同类型已聚合去重 -->
      <div
        v-if="pointsGroups.length > 1"
        class="points-details"
      >
        <div
          v-for="(d, i) in pointsGroups"
          :key="i"
          class="points-detail-row"
          :title="detailTooltip(d)"
        >
          <span class="info-label">{{ d.name }}</span>
          <span class="info-value detail-points">{{ d.remaining }} 分</span>
        </div>
      </div>
      <div class="info-row">
        <span class="info-label">添加时间</span>
        <span class="info-value">{{ formatDate(account.createdAt) }}</span>
      </div>
      <div class="info-row" v-if="account.credentialStatus">
        <span class="info-label">桌面凭证</span>
        <span class="cred-value-wrap">
          <span class="info-value" :class="account.credentialStatus === 'expired' ? 'text-danger' : 'text-success'">
            {{ credentialStatusText }}
          </span>
          <button
            class="btn-refresh-cred"
            title="回读 TRAE 最新凭证(打开 TRAE 让它自行刷新后,点此同步)"
            @click="handleRefreshCred"
            :disabled="refreshingCred"
          >
            <Icon name="arrow-path" :size="11" :class="{ spinning: refreshingCred }" />
          </button>
        </span>
      </div>
      <!-- 多开实例:展示签到设备码(虚拟设备码标签在左,数字在右) -->
      <div class="info-row" v-if="account.dataDir">
        <span class="info-label">虚拟设备码</span>
        <span class="dev-value" title="本账号专属的隔离设备,签到用它与真实设备码隔离">
          <span class="dev-value__num">{{ account.checkinDeviceId || '首次签到后生成' }}</span>
        </span>
      </div>
      <!-- 主账号:无独立 data-dir,展示真实设备码(标签在左,数字在右) -->
      <div class="info-row" v-else>
        <span class="info-label">真实设备码</span>
        <span class="dev-value" title="本机真实设备码,所有账号默认共用的那个">
          <span class="dev-value__num">{{ account.checkinDeviceId || '主账号·真实设备码' }}</span>
        </span>
      </div>
    </div>

    <!-- 卡片底部 -->
    <div class="card-footer">
      <button
        class="btn btn-sm btn-primary"
        @click="handleCheckin"
        :disabled="checkingIn || store.checkingIn || !account.enabled"
      >
        <span v-if="checkingIn" class="spinner"></span>
        <span>{{ checkingIn ? '签到中' : '立即签到' }}</span>
      </button>
      <button
        v-if="instanceState.source === 'none'"
        class="btn btn-sm btn-outline"
        @click="handleLaunchMulti"
        :disabled="launching"
        :title="account.dataDir ? '启动该账号的独立实例' : '首次启动:创建独立实例并启动'"
      >
        <span v-if="launching" class="spinner"></span>
        <span>{{ launching ? '程序正在启动,请稍后' : '启动' }}</span>
      </button>
      <button
        v-else
        class="btn btn-sm btn-outline"
        @click="handleFocus"
        :title="instanceState.source === 'main' ? '该账号在主实例运行,点击聚焦主实例窗口' : '该账号工具实例已运行,点击聚焦其窗口'"
      >
        <span>聚焦</span>
      </button>
      <button type="button" class="btn btn-sm btn-outline" @click.stop="startEdit">
        编辑
      </button>
      <button class="btn btn-sm btn-outline danger" @click="showDeleteConfirm = true">
        删除
      </button>
    </div>
  </div>
  <!-- 删除确认弹窗 -->
  <div v-if="showDeleteConfirm" class="delete-overlay" @click.self="showDeleteConfirm = false">
    <div class="delete-dialog">
      <h3>删除实例</h3>
      <p class="delete-warning">
        确定要删除「{{ account.name }}」吗？此操作将删除该账号及其多开实例数据，不可恢复。
      </p>
      <div class="delete-actions">
        <button class="btn btn-sm btn-outline" @click="showDeleteConfirm = false">取消</button>
        <button class="btn btn-sm btn-outline danger" @click="confirmDelete">确认删除</button>
      </div>
    </div>
  </div>
  <div v-if="editing" class="edit-overlay" @click.self="cancelEdit">
    <div class="edit-dialog">
      <h3>编辑账号名称</h3>
      <input v-model="draftName" class="input" autofocus @keyup.enter="saveEdit" @keyup.esc="cancelEdit" />
      <div class="edit-actions"><button class="btn btn-sm btn-outline" @click="cancelEdit">取消</button><button class="btn btn-sm btn-primary" @click="saveEdit">保存</button></div>
    </div>
  </div>

  <!-- 积分查询接口出参悬浮层(美化格式化的 JSON) -->
  <Teleport to="body">
    <div
      v-if="respPopover.visible"
      class="resp-popover"
      :style="respPopover.style"
    >
      <div class="resp-popover-head">
        <span>积分查询接口出参</span>
        <button class="resp-popover-close" @click="respPopover.visible = false">×</button>
      </div>
      <pre class="resp-popover-body">{{ formatJson(account.pointsResponse) }}</pre>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useAppStore, type Account, type PointsDetail, type InstanceState } from '../stores/app'

const props = defineProps<{
  account: Account
}>()

const emit = defineEmits(['toggle', 'delete', 'edit', 'notify'])
const store = useAppStore()

const checkingIn = ref(false)
const launching = ref(false)
const refreshingCred = ref(false)
const instanceState = ref<InstanceState>({ running: false, source: 'none', isMainAccount: false })

// ===== 积分查询接口出参悬浮 =====
const respPopover = ref<{ visible: boolean; style: { top: string; left: string } }>({
  visible: false,
  style: { top: '0', left: '0' },
})

function toggleResp(e: MouseEvent) {
  if (!props.account.pointsResponse) return
  if (respPopover.value.visible) {
    respPopover.value.visible = false
    return
  }
  respPopover.value.visible = true
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  const offset = 8
  const height = 420
  const width = Math.min(560, window.innerWidth - offset * 2)
  let top = rect.bottom + offset
  if (top + height > window.innerHeight) top = rect.top - offset - height
  // 水平方向 clamp,避免超出右缘被遮挡
  let left = rect.left
  if (left + width + offset > window.innerWidth) left = window.innerWidth - width - offset
  if (left < offset) left = offset
  respPopover.value.style = {
    top: `${Math.max(offset, top)}px`,
    left: `${left}px`,
  }
}

function formatJson(s: string | undefined): string {
  if (!s) return ''
  try {
    return JSON.stringify(JSON.parse(s), null, 2)
  } catch {
    return s
  }
}

onMounted(async () => {
  instanceState.value = await store.getInstanceState(props.account.id)
})

// 监听全局刷新信号(顶部"刷新"按钮触发),重新查询本账号实例运行状态(含主/工具来源)
watch(() => store.instanceRefreshTick, async () => {
  instanceState.value = await store.getInstanceState(props.account.id)
})
const editing = ref(false)
const draftName = ref(props.account.name)

const statusText = computed(() => {
  if (!props.account.enabled) return '已停用'
  if (!props.account.lastCheckinAt) return '未签到'

  const today = new Date().toDateString()
  const lastDate = new Date(props.account.lastCheckinAt).toDateString()

  if (lastDate === today) {
    const r = props.account.lastCheckinResult
    if (r === 'success') return '今日已签'
    if (r === 'already') return '今日已签到'
    return '今日失败'
  }
  return '待签到'
})

const statusBadgeClass = computed(() => {
  if (!props.account.enabled) return 'badge-muted'
  if (!props.account.lastCheckinAt) return 'badge-warning'

  const today = new Date().toDateString()
  const lastDate = new Date(props.account.lastCheckinAt).toDateString()

  if (lastDate === today) {
    const r = props.account.lastCheckinResult
    if (r === 'success') return 'badge-success'
    if (r === 'already') return 'badge-info'
    return 'badge-danger'
  }
  return 'badge-info'
})

const lastCheckinText = computed(() => {
  if (!props.account.lastCheckinAt) return '从未签到'
  return formatDateTime(props.account.lastCheckinAt)
})

const pointsTooltip = computed(() => {
  if (!props.account.pointsUpdatedAt) return ''
  return `积分更新于 ${pointsUpdatedText.value}`
})

const pointsUpdatedText = computed(() => {
  if (!props.account.pointsUpdatedAt) return ''
  const d = new Date(props.account.pointsUpdatedAt)
  const now = Date.now()
  const diff = now - props.account.pointsUpdatedAt
  if (diff < 60000) return '刚刚'
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`
  return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
})

// 各类型可用积分明细:按类型名聚合(同批重复权益合并),丢弃剩余 0 的行
const pointsGroups = computed<PointsDetail[]>(() => {
  const list = props.account.pointsDetails || []
  const map = new Map<string, PointsDetail>()
  for (const d of list) {
    if (d.remaining <= 0) continue
    const cur = map.get(d.name)
    if (cur) {
      cur.remaining += d.remaining
      cur.total += d.total
      if (d.expireAt && (!cur.expireAt || d.expireAt < cur.expireAt)) cur.expireAt = d.expireAt
    } else {
      map.set(d.name, { ...d })
    }
  }
  return [...map.values()]
})

// 单类积分 tooltip:剩余/总额/到期时间(未知时不显示到期)
const detailTooltip = (d: PointsDetail): string => {
  const base = `${d.name}: 剩余 ${d.remaining}，总额 ${d.total}`
  if (d.expireAt && d.expireAt > 0) return `${base}，到期 ${formatDateTime(d.expireAt)}`
  return base
}

const credentialStatusText = computed(() => {
  if (props.account.credentialStatus === 'expired') return '凭证已失效，请打开 TRAE 实例刷新'
  if (props.account.credentialStatus === 'expiring') return '凭证即将过期，打开 TRAE 实例续期'
  return '凭证有效'
})

function handleToggle() {
  emit('toggle', props.account.id, !props.account.enabled)
}

// 签到由卡片自身执行并等待完成,按钮在整个请求期间保持禁用(防止连点触发服务端限频)
async function handleCheckin() {
  if (checkingIn.value) return
  checkingIn.value = true
  try {
    const result = await store.checkinAccount(props.account.id)
    let msg = result?.message || (result?.success ? '签到成功' : '签到失败')
    if (msg.includes('频繁')) msg += ',请稍候一分钟再试'
    emit('notify', msg, result?.success ? 'success' : 'error')
    if (result?.success) {
      const points = await store.getAccountPoints(props.account.id)
      emit('notify',
        points?.success ? '签到后总积分已更新' : (points?.message || '签到成功，但总积分查询失败'),
        points?.success ? 'success' : 'error')
    }
  } catch (e: any) {
    emit('notify', '签到失败: ' + (e?.message || e), 'error')
  } finally {
    checkingIn.value = false
  }
}

async function handleLaunchMulti() {
  launching.value = true
  try {
    await store.launchMulti(props.account.id)
    instanceState.value = { running: true, source: 'tool', isMainAccount: instanceState.value.isMainAccount }
    emit('notify', '实例已启动', 'success')
  } catch (e: any) {
    emit('notify', '启动失败: ' + (e?.message || e), 'error')
    // 失败可能因状态过时(如该账号已在主实例运行),重新查询修正显示
    instanceState.value = await store.getInstanceState(props.account.id)
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

const showDeleteConfirm = ref(false)

function confirmDelete() {
  showDeleteConfirm.value = false
  emit('delete', props.account.id)
}

// 回读最新凭证:打开 TRAE 实例让它自行刷新 token,点此同步到应用(应用不主动刷新 token)
async function handleRefreshCred() {
  if (refreshingCred.value) return
  refreshingCred.value = true
  try {
    await store.refreshCredential(props.account.id)
    emit('notify', '已回读 TRAE 最新凭证', 'success')
  } catch (e: any) {
    emit('notify', '回读凭证失败: ' + (e?.message || e), 'error')
  } finally {
    refreshingCred.value = false
  }
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

.badge-main {
  display: inline-block;
  margin-left: 8px;
  padding: 2px 8px;
  font-size: 11px;
  font-weight: 600;
  color: #fff;
  background: var(--accent);
  border-radius: 6px;
  vertical-align: middle;
  box-shadow: var(--accent-glow);
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

/* 签到设备码数字:右侧值,限制宽度避免溢出 */
.dev-value {
  display: flex;
  align-items: center;
  min-width: 0;
}

.dev-value__num {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 总积分:单行,黄色强调 */
.points {
  color: var(--warning);
  font-weight: 700;
}

/* 有积分查询出参时可点击查看 */
.points.clickable {
  cursor: pointer;
}

/* 积分查询接口出参悬浮层 */
.resp-popover {
  position: fixed;
  z-index: 9999;
  width: 560px;
  max-width: calc(100vw - 32px);
  max-height: 420px;
  display: flex;
  flex-direction: column;
  background: var(--surface-elevated, var(--surface));
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  box-shadow: var(--shadow-modal, 0 8px 30px rgba(0, 0, 0, 0.25));
  overflow: hidden;
}

.resp-popover-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  font-size: 12px;
  font-weight: 700;
  color: var(--text-secondary);
  background: var(--surface-deep);
  border-bottom: 1px solid var(--border);
}

.resp-popover-close {
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
}

.resp-popover-close:hover {
  color: var(--text-primary);
}

.resp-popover-body {
  flex: 1;
  overflow: auto;
  margin: 0;
  padding: 12px 14px;
  font-family: 'SF Mono', Monaco, Consolas, monospace;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-primary);
  white-space: pre;
}

/* 各类型可用积分明细:缩进,行高更紧凑,弱化层级 */
.points-details {
  margin: 2px 0 2px 16px;
  border-left: 2px solid var(--text-muted);
  padding-left: 10px;
}

.points-detail-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 3px 0;
}

.detail-points {
  font-weight: 600;
}

/* 设备码数值:与其它信息行一致,不加额外颜色 */

/* 桌面凭证行:状态 + 手动刷新小按钮 */
.cred-value-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
}

.btn-refresh-cred {
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 50%;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--surface);
  color: var(--text-muted);
  box-shadow: var(--shadow-soft-raised);
  transition: box-shadow var(--t-press), color var(--t-smooth);
  flex-shrink: 0;
}

.btn-refresh-cred:hover:not(:disabled) {
  color: var(--accent);
}

.btn-refresh-cred:active:not(:disabled) {
  box-shadow: var(--shadow-soft-inset);
}

.btn-refresh-cred:disabled {
  cursor: default;
  color: var(--accent);
}

.spinning {
  animation: cred-spin 0.8s linear infinite;
}

@keyframes cred-spin {
  to {
    transform: rotate(360deg);
  }
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

/* 删除确认弹窗 - 复用编辑弹窗的浮层风格,提示色用危险色 */
.delete-overlay {
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

.delete-dialog {
  width: 360px;
  padding: 22px;
  border-radius: var(--r-lg);
  background: var(--surface);
  border: none;
  box-shadow: var(--shadow-bold-raised);
}

.delete-dialog h3 {
  font-size: 16px;
  font-weight: 700;
  color: var(--text-primary);
}

.delete-warning {
  margin: 16px 0;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary);
}

.delete-actions {
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
