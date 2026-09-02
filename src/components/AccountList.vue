<template>
  <div class="account-list">
    <!-- 统计卡片 -->
    <div class="stats-row">
      <div class="stat-card">
        <div class="stat-icon total"><Icon name="users" :size="22" /></div>
        <div class="stat-info">
          <span class="stat-value">{{ store.accounts.length }}</span>
          <span class="stat-label">总账号数</span>
        </div>
      </div>
      <div class="stat-card">
        <div class="stat-icon enabled"><Icon name="check" :size="22" /></div>
        <div class="stat-info">
          <span class="stat-value">{{ store.enabledAccounts.length }}</span>
          <span class="stat-label">已启用</span>
        </div>
      </div>
      <div class="stat-card">
        <div class="stat-icon success"><Icon name="sparkles" :size="22" /></div>
        <div class="stat-info">
          <span class="stat-value">{{ store.todayCheckedCount }}</span>
          <span class="stat-label">今日已签到</span>
        </div>
      </div>
      <div class="stat-card">
        <div class="stat-icon points"><Icon name="banknotes" :size="22" /></div>
        <div class="stat-info">
          <div class="stat-value">{{ totalPoints }}</div>
          <div class="stat-meta">{{ lastPointsUpdatedText }}</div>
        </div>
      </div>
    </div>

    <!-- 账号列表 -->
    <div class="accounts-grid" v-if="displayOrdered.length > 0">
      <article
        v-for="account in displayOrdered"
        :key="account.id"
        class="card-drag"
        :data-accid="account.id"
        :class="{ dragging: elDragging === account.id, 'drop-target': dropTarget === account.id }"
        @mousedown="onPointerStart($event, account.id)"
      >
        <AccountCard
          :account="account"
          @toggle="handleToggle"
          @delete="handleDelete"
          @edit="handleEdit"
          @notify="handleNotify"
        />
      </article>
    </div>

    <!-- 空状态 -->
    <div class="empty-state" v-else>
      <Icon name="inbox" :size="48" class="empty-state-icon" />
      <p class="empty-state-text">还没有添加账号</p>
      <p class="empty-state-desc">点击右上角「添加账号」按钮开始使用</p>
      <button class="btn btn-primary" style="margin-top: 20px" @click="$emit('add-account')">
        <Icon name="plus" :size="16" />
        <span>添加第一个账号</span>
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useAppStore, type Account } from '../stores/app'
import AccountCard from './AccountCard.vue'
import Icon from './Icon.vue'

const store = useAppStore()
const emit = defineEmits(['add-account', 'notify'])

// 拖拽排序用的本地顺序(id 列表);跟随账号增删自动同步(dragOrder 外仍按后端原顺序插入)
const dragOrder = ref<string[]>([])

watch(
  () => store.accounts,
  (list) => {
    const ids = list.map(a => a.id)
    const known = new Set(ids)
    dragOrder.value = [
      ...dragOrder.value.filter(id => known.has(id)),
      ...ids.filter(id => !dragOrder.value.includes(id)),
    ]
  },
  { immediate: true }
)

// 按本地顺序取账号对象(空状态判定也依赖它)
const displayOrdered = computed(() => {
  const byId = new Map(store.accounts.map(a => [a.id, a]))
  return dragOrder.value
    .map(id => byId.get(id))
    .filter((a): a is Account => !!a)
})

// 拖动逻辑(mouse-based,规避 WebView2 原生 DnD 的不稳定)
// 设计:点击/选区文本不做任何拖拽(保持文本可选中);位移超过阈值后才真正进入拖拽,
// 避免卡住鼠标选择/拖动只为了排序时被误判为拖拽。
// 当前正在拖拽的卡片 id(原卡片保持占位,opacity 降低;实际移动用克隆副本)
const elDragging = ref<string | null>(null)
// 待判定拖拽的卡片 id(mousedown 先记录,位移超阈值后才真正进入拖拽)
let pendingDragId: string | null = null
// 跟随指针的拖动副本(挂到 body,脱离 grid,避免布局塌陷导致滚动条跳动)
let ghostEl: HTMLElement | null = null
// 当前落点卡片 id(虚线框高亮)
const dropTarget = ref<string | null>(null)
const DRAG_THRESHOLD = 5
let dragStartX = 0
let dragStartY = 0
let dragOffsetX = 0
let dragOffsetY = 0

function onPointerStart(e: MouseEvent, id: string) {
  const t = (e.target as HTMLElement)?.closest?.('a,button,input,select,textarea,.switch,.btn')
  if (t) return // 点击交互控件内部不触发拖拽
  const card = e.currentTarget as HTMLElement
  const rect = card.getBoundingClientRect()
  dragStartX = e.clientX
  dragStartY = e.clientY
  dragOffsetX = e.clientX - rect.left
  dragOffsetY = e.clientY - rect.top
  pendingDragId = id
  window.addEventListener('mousemove', onPointerMove)
  window.addEventListener('mouseup', onPointerUp)
}

function onPointerMove(e: MouseEvent) {
  // 尚未进入拖拽:只有位移超过阈值才算"拖动"开始;否则视为点击/选择文本,不拦截
  if (!elDragging.value) {
    if (!pendingDragId) return
    if (Math.abs(e.clientX - dragStartX) < DRAG_THRESHOLD &&
        Math.abs(e.clientY - dragStartY) < DRAG_THRESHOLD) return
    // 真正开始拖拽:取消文本选择,创建跟随指针的克隆副本;原卡片仍在 grid 占位(滚动条不跳动)
    document.getSelection()?.removeAllRanges()
    const card = document.querySelector(
      `.card-drag[data-accid="${pendingDragId}"]`
    ) as HTMLElement | null
    if (!card) return
    const rect = card.getBoundingClientRect()
    dragOffsetX = e.clientX - rect.left
    dragOffsetY = e.clientY - rect.top
    elDragging.value = pendingDragId
    dropTarget.value = pendingDragId
    const gh = card.cloneNode(true) as HTMLElement
    gh.classList.add('drag-ghost')
    gh.style.cssText =
      `position:fixed;left:${rect.left}px;top:${rect.top}px;` +
      `width:${rect.width}px;margin:0;pointer-events:none;z-index:1000;`
    document.body.appendChild(gh)
    ghostEl = gh
  }
  // 让副本跟随指针
  ghostEl!.style.left = `${e.clientX - dragOffsetX}px`
  ghostEl!.style.top = `${e.clientY - dragOffsetY}px`
  // 位移过小视为点击,不重排
  if (Math.abs(e.clientY - dragStartY) < 4) return
  const el = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null
  const card = el?.closest?.('[data-accid]') as HTMLElement | null
  const targetId = card?.dataset.accid
  if (!targetId) return
  // 拖动过程只标记落点(松手时一次性交换,避免拖动中乱序)
  dropTarget.value = targetId === elDragging.value ? null : targetId
}

function onPointerUp() {
  window.removeEventListener('mousemove', onPointerMove)
  window.removeEventListener('mouseup', onPointerUp)
  ghostEl?.remove()
  ghostEl = null
  const dragId = elDragging.value
  const targetId = dropTarget.value
  pendingDragId = null
  elDragging.value = null
  dropTarget.value = null
  // 若未真正进入拖拽(纯点击/文本选择),不交换
  if (dragId && targetId && targetId !== dragId) {
    swapWith(dragId, targetId)
    persistOrder()
  }
}

// 交换两个位置上的卡片
function swapWith(dragId: string, targetId: string) {
  const list = dragOrder.value
  const from = list.indexOf(dragId)
  const to = list.indexOf(targetId)
  if (from < 0 || to < 0 || from === to) return
  const next = [...list]
  ;[next[from], next[to]] = [next[to], next[from]]
  dragOrder.value = next
}

function persistOrder() {
  // 仅当顺序与当前生效值不同才触发后端保存,避免拖拽过程中重复请求
  const current = store.accounts.map(a => a.id)
  if (JSON.stringify(current) === JSON.stringify(dragOrder.value)) return
  void store.reorderAccounts([...dragOrder.value])
}

const totalPoints = computed(() => {
  return store.accounts.reduce((sum, a) => sum + (a.points || 0), 0)
})

const lastPointsUpdatedText = computed(() => {
  const times = store.accounts.map(a => a.pointsUpdatedAt).filter((t): t is number => !!t)
  if (times.length === 0) return '总积分'
  const latest = Math.max(...times)
  const now = Date.now()
  const diff = now - latest
  let label: string
  if (diff < 60000) label = '刚刚'
  else if (diff < 3600000) label = `${Math.floor(diff / 60000)} 分钟前`
  else if (diff < 86400000) label = `${Math.floor(diff / 3600000)} 小时前`
  else {
    const d = new Date(latest)
    label = `${d.getMonth() + 1}/${d.getDate()}`
  }
  return `更新于 ${label}`
})

function handleNotify(message: string, type: 'success' | 'error') {
  emit('notify', message, type)
}

async function handleToggle(id: string, enabled: boolean) {
  await store.updateAccount(id, { enabled })
}

async function handleDelete(id: string) {
  await store.deleteAccount(id)
}

function handleEdit(id: string, newName?: string) {
  const account = store.accounts.find(item => item.id === id)
  if (!account) return
  if (newName) { void store.updateAccount(id, { name: newName }); return }
  const name = window.prompt('请输入账号名称', account.name)?.trim()
  if (!name || name === account.name) return
  void store.updateAccount(id, { name })
}
</script>

<style scoped>
.account-list {
  max-width: 100%;
}

.stats-row {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: 18px;
  margin-bottom: 24px;
}

/* 统计卡片 - 凸起,hover 上浮 */
.stat-card {
  background: var(--surface);
  border: none;
  border-radius: var(--r-md);
  padding: 18px;
  display: flex;
  align-items: center;
  gap: 14px;
  box-shadow: var(--shadow-soft-raised);
  transition: box-shadow var(--t-smooth), transform var(--t-smooth);
}

.stat-card:hover {
  transform: translateY(-3px);
  box-shadow: var(--shadow-md-raised);
}

/* 统计图标 - 凸起小方块,图标用状态色(小面积着色) */
.stat-icon {
  width: 48px;
  height: 48px;
  border-radius: 14px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--surface);
  box-shadow: var(--shadow-soft-raised);
  flex-shrink: 0;
}

.stat-icon.total {
  color: var(--accent);
}

.stat-icon.enabled {
  color: var(--success);
}

.stat-icon.success {
  color: var(--warning);
}

.stat-icon.points {
  color: var(--danger);
}

.stat-info {
  display: flex;
  flex-direction: column;
}

.stat-value {
  font-family: 'Nunito', 'Inter', sans-serif;
  font-size: 24px;
  font-weight: 800;
  color: var(--text-primary);
  line-height: 1.2;
}

.stat-label,
.stat-meta {
  font-size: 13px;
  color: var(--text-secondary);
  margin-top: 2px;
}

.accounts-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 18px;
}

.card-drag {
  margin: 0;
  cursor: grab;
  transition: transform var(--t-fast, 0.12s ease), opacity var(--t-fast, 0.12s ease);
}

.card-drag:active {
  cursor: grabbing;
}

.card-drag:hover {
  transform: translateY(-2px);
}

/* 拖拽中:原卡片降透明保留占位,克隆副本负责"跟随指针"的视觉效果 */
.card-drag.dragging {
  cursor: grabbing;
  opacity: 0.4;
}

/* 跟随指针的拖动副本(挂 body,fixed 定位由 JS 设置) */
:deep(.drag-ghost) {
  cursor: grabbing;
  transform: scale(1.03);
  border-radius: var(--r-md);
  box-shadow: var(--shadow-soft-raised, 0 16px 40px rgba(0, 0, 0, 0.28));
}

/* 落点卡片:虚线框提示将插入位置 */
.card-drag.drop-target {
  outline: 2px dashed var(--accent);
  outline-offset: -2px;
  border-radius: var(--r-md);
}
</style>
