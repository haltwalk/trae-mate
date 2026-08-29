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
          <span class="stat-value">{{ totalPoints }}</span>
          <span class="stat-label">总积分</span>
        </div>
      </div>
    </div>

    <!-- 账号列表 -->
    <div class="accounts-grid" v-if="store.accounts.length > 0">
      <AccountCard
        v-for="account in store.accounts"
        :key="account.id"
        :account="account"
        @toggle="handleToggle"
        @delete="handleDelete"
        @edit="handleEdit"
        @notify="handleNotify"
      />
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
import { computed } from 'vue'
import { useAppStore } from '../stores/app'
import AccountCard from './AccountCard.vue'
import Icon from './Icon.vue'

const store = useAppStore()
const emit = defineEmits(['add-account', 'notify'])

const totalPoints = computed(() => {
  return store.accounts.reduce((sum, a) => sum + (a.points || 0), 0)
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

.stat-label {
  font-size: 13px;
  color: var(--text-secondary);
  margin-top: 2px;
}

.accounts-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 18px;
}
</style>
