import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'

export interface Account {
  id: string
  name: string
  cookie: string
  createdAt: number
  lastCheckinAt?: number
  lastCheckinResult?: 'success' | 'failed' | 'pending'
  lastCheckinMessage?: string
  points?: number
  enabled: boolean
  desktopUserId?: string
  credentialStatus?: 'valid' | 'expiring' | 'expired'
  dataDir?: string
  machineId?: string
}

export interface CheckinLog {
  id: string
  accountId: string
  accountName: string
  time: number
  result: 'success' | 'failed'
  message: string
  pointsGained?: number
}

export interface CheckinResult {
  success: boolean
  message: string
  points?: number
}

export interface PointsResult {
  success: boolean
  message: string
  totalPoints?: number
}

export interface LaunchResult {
  dataDir: string
  machineId: string
  launched: boolean
}

export interface AppSettings {
  autoCheckin: boolean
  checkinTime: string
  retryCount: number
  retryDelay: number
  notifyOnSuccess: boolean
  notifyOnFailed: boolean
  // 以下字段为兼容旧 UI(已隐藏),desktop 模式下后端不再使用
  checkinMode?: 'webview' | 'api'
  apiConfig?: {
    checkinUrl: string
    method: 'GET' | 'POST'
    headers: Record<string, string>
    body?: Record<string, any>
  }
}

export const useAppStore = defineStore('app', () => {
  // 状态
  const accounts = ref<Account[]>([])
  const logs = ref<CheckinLog[]>([])
  const settings = ref<AppSettings | null>(null)
  const loading = ref(false)
  const checkingIn = ref(false)
  const launching = ref(false) // 多开启动中
  const instanceRefreshTick = ref(0) // 多开状态刷新信号:自增触发各卡片重查
  const nextRunTime = ref<string | null>(null)

  // 计算属性
  const enabledAccounts = computed(() => accounts.value.filter(a => a.enabled))
  const todayCheckedCount = computed(() => {
    const today = new Date().toDateString()
    return accounts.value.filter(a => {
      if (!a.lastCheckinAt) return false
      return new Date(a.lastCheckinAt).toDateString() === today && a.lastCheckinResult === 'success'
    }).length
  })

  // 方法
  async function fetchAccounts() {
    try {
      const result = await invoke<Account[]>('get_accounts')
      accounts.value = result || []
    } catch (e) {
      console.error('获取账号列表失败:', e)
    }
  }

  async function importDesktopAccount() {
    const result = await invoke('import_desktop_account')
    await fetchAccounts()
    return result
  }

  async function updateAccount(id: string, updates: Partial<Account>) {
    try {
      const result = await invoke('update_account', { id, updates })
      await fetchAccounts()
      return result
    } catch (e) {
      console.error('更新账号失败:', e)
      throw e
    }
  }

  async function deleteAccount(id: string) {
    try {
      await invoke('delete_account', { id })
      await fetchAccounts()
    } catch (e) {
      console.error('删除账号失败:', e)
      throw e
    }
  }

  async function checkinAccount(id: string) {
    try {
      checkingIn.value = true
      const result = await invoke<CheckinResult>('checkin_account', { id })
      await fetchAccounts()
      await fetchLogs()
      return result
    } finally {
      checkingIn.value = false
    }
  }

  async function checkinAll() {
    try {
      checkingIn.value = true
      const result = await invoke<Array<[Account, CheckinResult]>>('checkin_all')
      await fetchAccounts()
      await fetchLogs()
      return result
    } finally {
      checkingIn.value = false
    }
  }

  async function getAccountPoints(id: string) {
    try {
      const result = await invoke<PointsResult>('get_account_points', { id })
      await fetchAccounts()
      return result
    } catch (e) {
      console.error('获取积分失败:', e)
      throw e
    }
  }

  async function fetchLogs(limit = 100) {
    try {
      const result = await invoke<CheckinLog[]>('get_logs', { limit })
      logs.value = result || []
    } catch (e) {
      console.error('获取日志失败:', e)
    }
  }

  async function clearLogs() {
    try {
      await invoke('clear_logs')
      await fetchLogs()
    } catch (e) {
      console.error('清空日志失败:', e)
      throw e
    }
  }

  async function fetchSettings() {
    try {
      const result = await invoke<AppSettings>('get_settings')
      settings.value = result
    } catch (e) {
      console.error('获取设置失败:', e)
    }
  }

  async function saveSettings(newSettings: Partial<AppSettings>) {
    try {
      const result = await invoke<AppSettings>('save_settings', { settings: newSettings })
      settings.value = result
      await fetchNextRunTime()
      return result
    } catch (e) {
      console.error('保存设置失败:', e)
      throw e
    }
  }

  async function fetchNextRunTime() {
    try {
      const result = await invoke<string | null>('get_next_run_time')
      nextRunTime.value = result
    } catch (e) {
      console.error('获取下次执行时间失败:', e)
    }
  }

  // 多开实例:用账号凭据启动免登录的独立 TRAE 实例
  async function launchMulti(id: string) {
    try {
      launching.value = true
      const result = await invoke<LaunchResult>('launch_account_multi', { id })
      await fetchAccounts()
      return result
    } finally {
      launching.value = false
    }
  }

  // 查询账号多开实例是否在运行
  async function isInstanceRunning(id: string): Promise<boolean> {
    try {
      return await invoke<boolean>('is_account_instance_running', { id })
    } catch (e) {
      console.error('查询实例运行状态失败:', e)
      return false
    }
  }

  // 触发所有账号卡片重新查询多开实例运行状态(关闭实例后状态不自动更新,手动刷新用)
  function refreshInstances() {
    instanceRefreshTick.value++
  }

  // 聚焦账号多开实例的窗口(提到前台)
  async function focusInstance(id: string) {
    await invoke('focus_account_instance', { id })
  }

  // TRAE 客户端路径
  async function getTraeExePath(): Promise<string | null> {
    try {
      return await invoke<string | null>('get_trae_exe_path')
    } catch (e) {
      console.error('获取 TRAE 路径失败:', e)
      return null
    }
  }

  async function setTraeExePath(path: string) {
    await invoke('set_trae_exe_path', { path })
  }

  async function scanTraeExePath(): Promise<string> {
    return await invoke<string>('scan_trae_exe_path')
  }

  // 初始化
  async function init() {
    loading.value = true
    try {
      await Promise.all([
        fetchAccounts(),
        fetchLogs(),
        fetchSettings(),
        fetchNextRunTime()
      ])
    } finally {
      loading.value = false
    }
  }

  return {
    // 状态
    accounts,
    logs,
    settings,
    loading,
    checkingIn,
    launching,
    instanceRefreshTick,
    nextRunTime,
    // 计算属性
    enabledAccounts,
    todayCheckedCount,
    // 方法
    fetchAccounts,
    importDesktopAccount,
    updateAccount,
    deleteAccount,
    checkinAccount,
    checkinAll,
    getAccountPoints,
    fetchLogs,
    clearLogs,
    fetchSettings,
    saveSettings,
    fetchNextRunTime,
    launchMulti,
    isInstanceRunning,
    focusInstance,
    refreshInstances,
    getTraeExePath,
    setTraeExePath,
    scanTraeExePath,
    init
  }
})
