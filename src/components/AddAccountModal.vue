<template>
  <div class="modal-overlay" v-if="visible" @click.self="handleClose">
    <div class="modal-content">
      <div class="modal-header">
        <h2>添加账号</h2>
        <button class="close-btn" @click="handleClose"><Icon name="x-mark" :size="18" /></button>
      </div>

      <div class="modal-body">
        <div class="form-group">
          <label class="form-label">账号备注名</label>
          <input
            v-model="form.name"
            type="text"
            class="input"
            placeholder="例如：主号、小号1"
          />
        </div>

        <div class="tip-box">
          <div class="tip-icon"><Icon name="clock" :size="20" /></div>
          <div class="tip-content">
            <p class="tip-title">导入当前 TRAE 桌面账号</p>
            <p class="tip-desc">先在 TRAE 客户端切换到目标账号，再点击下方导入。凭证仅加密保存在本机。</p>
          </div>
        </div>

        <!-- 新增实例前置提示:免劫持后无需再要求关闭所有 TRAE,此处已移除 -->

        <div class="form-group">
          <label class="form-checkbox">
            <input type="checkbox" v-model="form.enabled" checked />
            <span>启用自动签到</span>
          </label>
        </div>

        <!-- 扫描已有多开目录 -->
        <div class="form-group" v-if="scannedDirs !== null">
          <label class="form-label">已有多开目录({{ scannedDirs.length }})</label>
          <p v-if="scannedDirs.length === 0" class="scan-empty">
            未发现含登录信息的多开目录(%APPDATA%\TRAE SOLO CN_*)
          </p>
          <div v-for="d in scannedDirs" :key="d.dataDir" class="scan-row">
            <div class="scan-info">
              <div class="scan-name">
                {{ d.accountName || d.userId || '(未命名)' }}
                <span v-if="d.bound" class="scan-badge">已导入</span>
              </div>
              <div class="scan-meta">
                {{ shortDir(d.dataDir) }} · {{ d.running ? '运行中' : '未运行' }} ·
                {{ expireText(d.expiresAt) }}
              </div>
            </div>
            <button
              class="btn btn-sm btn-outline"
              @click="importDir(d)"
              :disabled="importingDir !== null"
            >
              {{ importingDir === d.dataDir ? '导入中' : d.bound ? '更新' : '导入' }}
            </button>
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <div class="footer-row">
        <button class="btn btn-outline" @click="scanDirs" :disabled="scanning">
          <span v-if="scanning" class="spinner"></span>
          <span>{{ scanning ? '扫描中...' : '扫描已有多开目录' }}</span>
        </button>
        <button
          class="btn btn-outline"
          @click="openNewLoginInstance"
          :disabled="submitting"
        >
          打开新实例登录
        </button>
      </div>
        <div class="footer-row">
          <button class="btn btn-secondary" @click="handleClose">取消</button>
          <button class="btn btn-primary" @click="importDesktopAccount" :disabled="submitting">
            <span v-if="submitting" class="spinner"></span>
            <span>{{ submitting ? '导入中...' : '导入当前 TRAE 桌面账号' }}</span>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { useAppStore, type InstanceDirInfo } from '../stores/app'
import Icon from './Icon.vue'

const props = defineProps<{
  visible: boolean
}>()

const emit = defineEmits(['update:visible', 'success', 'notify'])

const store = useAppStore()

const form = ref({
  name: '',
  enabled: true
})

const submitting = ref(false)
// 扫描已有多开目录:null 未扫描,[] 扫描后无结果
const scannedDirs = ref<InstanceDirInfo[] | null>(null)
const scanning = ref(false)
const importingDir = ref<string | null>(null)

watch(() => props.visible, (val) => {
  if (val) {
    form.value = { name: '', enabled: true }
    scannedDirs.value = null
  }
})

function handleClose() {
  emit('update:visible', false)
}

async function importDesktopAccount() {
  submitting.value = true
  try {
    await store.importDesktopAccount()
    emit('success')
  } catch (e: any) {
    alert('导入失败: ' + (e.message || e))
  } finally {
    submitting.value = false
  }
}

async function openNewLoginInstance() {
  emit('update:visible', false)
  try {
    // 免劫持登录,签到设备隔离由后端自动生成独立设备兜底
    await store.openNewLoginInstance()
  } catch (e: any) {
    emit('notify', '打开实例失败: ' + (e?.message || e), 'error')
  }
}

async function scanDirs() {
  scanning.value = true
  try {
    scannedDirs.value = await store.scanInstanceDirs()
  } finally {
    scanning.value = false
  }
}

async function importDir(d: InstanceDirInfo) {
  importingDir.value = d.dataDir
  try {
    await store.importAccountFromDir(d.dataDir)
    emit('notify', `账号「${d.accountName || d.userId}」已从目录导入`, 'success')
    emit('success')
    // 重新扫描刷新"已导入"标记
    scannedDirs.value = await store.scanInstanceDirs()
  } catch (e: any) {
    emit('notify', '导入失败: ' + (e?.message || e), 'error')
  } finally {
    importingDir.value = null
  }
}

// 目录全路径过长,仅显示最后一段
function shortDir(dir: string): string {
  const parts = dir.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] || dir
}

function expireText(ms: number): string {
  if (!ms) return '过期时间未知'
  const d = new Date(ms)
  const diff = ms - Date.now()
  const dateStr = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
  return diff <= 0 ? `已过期(${dateStr})` : `有效期至 ${dateStr}`
}
</script>

<style scoped>
.modal-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(15, 23, 42, 0.4);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

/* 弹窗 - 强凸起卡片 */
.modal-content {
  background: var(--surface);
  border: none;
  border-radius: var(--r-xl);
  width: 520px;
  max-width: 90vw;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  box-shadow: var(--shadow-bold-raised);
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 24px;
  box-shadow: inset 0 -2px 5px var(--shadow-light);
}

.modal-header h2 {
  font-size: 18px;
  font-weight: 700;
  color: var(--text-primary);
}

/* 关闭按钮 - 凸起图标钮,按下凹陷 */
.close-btn {
  width: 32px;
  height: 32px;
  background: var(--surface);
  border: none;
  cursor: pointer;
  color: var(--text-muted);
  border-radius: var(--r-sm);
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: var(--shadow-soft-flat);
  transition: all var(--t-smooth);
}

.close-btn:hover {
  color: var(--text-primary);
  box-shadow: var(--shadow-soft-raised);
}

.close-btn:active {
  box-shadow: var(--shadow-soft-inset);
}

.modal-body {
  padding: 24px;
  overflow-y: auto;
  flex: 1;
}

.form-group {
  margin-bottom: 20px;
}

.form-group:last-child {
  margin-bottom: 0;
}

.form-label {
  display: block;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 8px;
}

.form-checkbox {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-primary);
}

.form-checkbox input {
  width: 16px;
  height: 16px;
  cursor: pointer;
  accent-color: var(--accent);
}

/* 提示框 - 凹槽,强调色图标与标题 */
.tip-box {
  display: flex;
  gap: 12px;
  padding: 14px;
  background: var(--surface);
  border: none;
  border-radius: var(--r-sm);
  box-shadow: var(--shadow-soft-inset);
  margin-bottom: 20px;
}

.tip-icon {
  color: var(--accent);
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

.tip-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--accent);
  margin-bottom: 4px;
}

.tip-desc {
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.5;
}

.modal-footer {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 16px 24px;
  box-shadow: inset 0 2px 5px var(--shadow-light);
}

/* 两行布局:上行次级操作,下行取消+主操作;行内按钮等宽铺满 */
.footer-row {
  display: flex;
  gap: 10px;
}

.footer-row .btn {
  flex: 1;
}

/* 扫描结果列表 - 凹槽行,Neumorphism 软阴影 */
.scan-empty {
  font-size: 12px;
  color: var(--text-secondary);
}

.scan-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: var(--r-sm);
  background: var(--surface);
  box-shadow: var(--shadow-soft-inset);
  margin-bottom: 8px;
}

.scan-row:last-child {
  margin-bottom: 0;
}

.scan-info {
  min-width: 0;
  flex: 1;
}

.scan-name {
  font-size: 13px;
  font-weight: 700;
  color: var(--text-primary);
  display: flex;
  align-items: center;
  gap: 6px;
}

.scan-badge {
  font-size: 10px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: var(--r-pill, 999px);
  color: var(--accent);
  background: var(--surface);
  box-shadow: var(--shadow-soft-flat);
}

.scan-meta {
  font-size: 11px;
  color: var(--text-secondary);
  margin-top: 3px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.scan-warn {
  color: var(--warning);
}
</style>
