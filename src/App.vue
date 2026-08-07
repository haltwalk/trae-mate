<template>
  <div class="app-container">
    <div class="app-body">
    <!-- 侧边栏 -->
    <aside class="sidebar">
      <!-- 品牌面板 - 凸起卡片统一承载窗口控件与 logo,避免顶部松散 -->
      <div class="window-bar" data-tauri-drag-region>
        <div class="window-controls">
          <button class="win-btn minimize" title="最小化" @click="minimizeWindow"><Icon name="minus" :size="13" /></button>
          <button class="win-btn close" title="关闭" @click="closeWindow"><Icon name="x-mark" :size="13" /></button>
        </div>
        <button class="win-btn theme-toggle" :title="isDark ? '切换到亮色模式' : '切换到暗色模式'" @click="toggleTheme">
          <Icon :name="isDark ? 'sun' : 'moon'" :size="16" />
        </button>
      </div>
      <div class="sidebar-header">
        <div class="logo">
          <span class="logo-icon"><Icon name="check" :size="20" /></span>
          <span class="logo-text">TraeCheck</span>
        </div>
        <p class="logo-desc">每日自动签到工具</p>
      </div>

      <nav class="nav-menu">
        <div
          class="nav-item"
          :class="{ active: activeTab === 'accounts' }"
          @click="activeTab = 'accounts'"
        >
          <span class="nav-icon"><Icon name="users" :size="18" /></span>
          <span>账号管理</span>
          <span class="nav-badge" v-if="store.accounts.length">{{ store.accounts.length }}</span>
        </div>
        <div
          class="nav-item"
          :class="{ active: activeTab === 'logs' }"
          @click="activeTab = 'logs'"
        >
          <span class="nav-icon"><Icon name="clipboard-list" :size="18" /></span>
          <span>签到日志</span>
        </div>
        <div
          class="nav-item"
          :class="{ active: activeTab === 'settings' }"
          @click="activeTab = 'settings'"
        >
          <span class="nav-icon"><Icon name="cog" :size="18" /></span>
          <span>设置</span>
        </div>
      </nav>

      <div class="sidebar-footer">
        <div class="status-card">
          <div class="status-item">
            <span class="status-label">自动签到</span>
            <span class="status-value" :class="store.settings?.autoCheckin ? 'on' : 'off'">
              {{ store.settings?.autoCheckin ? '已开启' : '已关闭' }}
            </span>
          </div>
          <div class="status-item" v-if="store.settings?.autoCheckin">
            <span class="status-label">签到时间</span>
            <span class="status-value">{{ store.settings?.checkinTime }}</span>
          </div>
          <div class="status-item">
            <span class="status-label">今日已签</span>
            <span class="status-value success">{{ store.todayCheckedCount }} / {{ store.enabledAccounts.length }}</span>
          </div>
        </div>
      </div>
    </aside>

    <!-- 主内容区 -->
    <div v-if="toastMessage" class="top-toast" :class="toastType">{{ toastMessage }}</div>

    <main class="main-content">
      <!-- 顶部栏 -->
      <header class="top-bar" data-tauri-drag-region>
        <div class="top-bar-left">
          <h1 class="page-title">{{ currentPageTitle }}</h1>
          <p class="page-subtitle" v-if="activeTab === 'accounts'">管理你的 Trae Work 账号，一键签到</p>
          <p class="page-subtitle" v-else-if="activeTab === 'logs'">查看所有签到记录和结果</p>
          <p class="page-subtitle" v-else>配置自动签到和通知设置</p>
        </div>
        <div class="top-bar-right">
          <button
            class="btn btn-outline"
            @click="handleRefresh"
            :disabled="refreshing"
            v-if="activeTab === 'accounts'"
            title="刷新账号与多开实例状态"
          >
            <Icon name="arrow-path" :size="16" :class="{ spinning: refreshing }" />
            <span>刷新</span>
          </button>
          <button
            class="btn btn-primary"
            @click="handleCheckinAll"
            :disabled="store.checkingIn || store.enabledAccounts.length === 0"
            v-if="activeTab === 'accounts'"
          >
            <span v-if="store.checkingIn" class="spinner"></span>
            <span>{{ store.checkingIn ? '签到中...' : '一键签到全部' }}</span>
          </button>
          <button
            class="btn btn-primary"
            @click="showAddModal = true"
            v-if="activeTab === 'accounts'"
          >
            <Icon name="plus" :size="16" />
            <span>添加账号</span>
          </button>
        </div>
      </header>

      <!-- 内容区域 -->
      <div class="content-area">
        <AccountList v-if="activeTab === 'accounts'" @add-account="showAddModal = true" @notify="showToast" />
        <CheckinLog v-else-if="activeTab === 'logs'" />
        <SettingsPanel v-else @notify="showToast" />
      </div>
    </main>
    </div><!-- /app-body -->

    <!-- 添加账号弹窗 -->
    <AddAccountModal
      v-model:visible="showAddModal"
      @success="handleAddSuccess"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { listen } from '@tauri-apps/api/event'
import { useAppStore } from './stores/app'
import AccountList from './components/AccountList.vue'
import CheckinLog from './components/CheckinLog.vue'
import SettingsPanel from './components/SettingsPanel.vue'
import AddAccountModal from './components/AddAccountModal.vue'
import Icon from './components/Icon.vue'

const store = useAppStore()
const activeTab = ref<'accounts' | 'logs' | 'settings'>('accounts')
const showAddModal = ref(false)
const toastMessage = ref('')
const toastType = ref<'success' | 'error'>('success')
let toastTimer: ReturnType<typeof setTimeout> | undefined

function showToast(message: string, type: 'success' | 'error' = 'success') {
  toastMessage.value = message
  toastType.value = type
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer = setTimeout(() => { toastMessage.value = '' }, 3500)
}

function closeWindow() {
  getCurrentWindow().close()
}

function minimizeWindow() {
  getCurrentWindow().minimize()
}

// 明暗主题切换(localStorage 持久化,main.ts 已完成初始设置)
const isDark = ref(document.documentElement.dataset.theme === 'dark')

function toggleTheme() {
  isDark.value = !isDark.value
  const theme = isDark.value ? 'dark' : 'light'
  document.documentElement.dataset.theme = theme
  localStorage.setItem('trae-check-theme', theme)
}

const currentPageTitle = computed(() => {
  switch (activeTab.value) {
    case 'accounts': return '账号管理'
    case 'logs': return '签到日志'
    case 'settings': return '设置'
    default: return ''
  }
})

async function handleCheckinAll() {
  if (store.enabledAccounts.length === 0) {
    alert('请先添加并启用账号')
    return
  }
  await store.checkinAll()
}

function handleAddSuccess() {
  showAddModal.value = false
}

const refreshing = ref(false)

// 刷新账号数据 + 触发各卡片重查多开实例运行状态(关闭实例后状态不自动更新,手动刷新)
async function handleRefresh() {
  if (refreshing.value) return
  refreshing.value = true
  try {
    void store.fetchAccounts()
    store.refreshInstances()
  } catch (e) {
    console.error('刷新失败:', e)
  } finally {
    setTimeout(() => { refreshing.value = false }, 600)
  }
}

onMounted(() => {
  store.init()
  // 监听系统托盘"一键签到"菜单
  listen('tray-checkin', () => {
    handleCheckinAll()
  })
})
</script>

<style scoped>
.app-container {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100vh;
  overflow: hidden;
  background: var(--surface);
}

.app-body {
  position: relative;
  z-index: 1;
  flex: 1;
  display: flex;
  min-height: 0;
}

/* 窗口操作栏 - 窗口控件左、主题切换右,两端对齐(独立于 logo 区,不共卡片) */
.window-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px 6px;
}

/* 窗口控件 - 橙色细线边框(参考小米台灯线条设计感),内嵌凸起圆钮 */
.window-controls {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px;
  width: fit-content;
  background: var(--surface);
  border: 2px solid #e60012;
  border-radius: var(--r-pill);
  box-shadow: none;
}

.win-btn {
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 50%;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--surface);
  color: var(--text-muted);
  box-shadow: var(--shadow-soft-raised);
  transition: box-shadow var(--t-press), transform var(--t-press), color var(--t-smooth);
}

.win-btn:hover {
  color: var(--text-primary);
  box-shadow: var(--shadow-md-raised);
}

.win-btn:active {
  box-shadow: var(--shadow-soft-inset);
  transform: scale(0.92);
}

.win-btn.close:hover {
  color: var(--danger);
}

/* 主题切换钮 - 略大于窗口控件,强调功能 */
.theme-toggle {
  width: 30px;
  height: 30px;
}

/* 侧边栏 - 同一表面,右侧柔和凹槽分隔 */
.sidebar {
  position: relative;
  z-index: 1;
  width: 250px;
  display: flex;
  flex-direction: column;
  background: var(--surface);
  box-shadow: none;
}

.sidebar-header {
  margin: 12px 14px 16px;
  padding: 18px;
  background: var(--surface);
  border: none;
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-soft-raised);
}

.logo {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 8px;
}

.logo-icon {
  width: 36px;
  height: 36px;
  background: var(--accent);
  border-radius: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  font-size: 20px;
  color: #fff;
  box-shadow: var(--shadow-soft-flat), var(--accent-glow);
}

.logo-text {
  font-family: 'Nunito', 'Inter', sans-serif;
  font-size: 19px;
  font-weight: 800;
  letter-spacing: 0.2px;
  color: var(--text-primary);
}

.logo-desc {
  font-size: 12px;
  color: var(--text-secondary);
  margin-left: 48px;
}

.nav-menu {
  flex: 1;
  padding: 14px 12px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 11px 14px;
  border-radius: 12px;
  cursor: pointer;
  transition: all var(--t-smooth);
  color: var(--text-secondary);
  font-size: 14px;
  font-weight: 600;
  margin-bottom: 6px;
  background: transparent;
  box-shadow: none;
}

.nav-item:hover {
  color: var(--text-primary);
  background: var(--surface);
  box-shadow: var(--shadow-soft-flat);
}

.nav-item.active {
  color: var(--accent);
  background: var(--surface);
  box-shadow: var(--shadow-soft-inset);
  font-weight: 700;
}

.nav-icon {
  display: inline-flex;
  align-items: center;
}

.nav-badge {
  margin-left: auto;
  background: var(--accent);
  color: #fff;
  font-size: 11px;
  padding: 2px 9px;
  border-radius: var(--r-pill);
  font-weight: 700;
  box-shadow: var(--accent-glow);
}

.sidebar-footer {
  padding: 16px;
}

.status-card {
  background: var(--surface);
  border: none;
  border-radius: var(--r-md);
  padding: 14px;
  box-shadow: var(--shadow-soft-raised);
}

.status-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 10px;
}

.status-item:last-child {
  margin-bottom: 0;
}

.status-label {
  font-size: 12px;
  color: var(--text-secondary);
}

.status-value {
  font-size: 13px;
  font-weight: 600;
}

.status-value.on {
  color: var(--success);
}

.status-value.off {
  color: var(--text-muted);
}

.status-value.success {
  color: var(--success);
}

/* 主内容区 */
.main-content {
  position: relative;
  z-index: 1;
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.top-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 18px 24px;
  margin: 14px 16px 0;
  background: var(--surface);
  border: none;
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-soft-raised);
}

.page-title {
  font-size: 21px;
  font-weight: 800;
  color: var(--text-primary);
  margin-bottom: 4px;
  letter-spacing: -0.01em;
}

.page-subtitle {
  font-size: 13px;
  color: var(--text-secondary);
}

.top-bar-right {
  display: flex;
  gap: 10px;
}

.content-area {
  flex: 1;
  overflow-y: auto;
  padding: 22px;
  margin: 14px 16px 16px;
  background: var(--surface);
  border: none;
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-soft-inset);
}

/* 顶部通知 - 彩色填充(信息通知,非装饰) */
.top-toast {
  position: fixed;
  top: 20px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 2000;
  padding: 12px 22px;
  border-radius: var(--r-md);
  color: #fff;
  border: none;
  font-weight: 600;
  box-shadow: var(--shadow-md-raised);
}

.top-toast.success {
  background: var(--success);
}

.top-toast.error {
  background: var(--danger);
}

/* 刷新按钮图标旋转动画(:deep 穿透到 Icon 根 svg) */
:deep(.spinning) {
  animation: app-spin 0.8s linear infinite;
}

@keyframes app-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
