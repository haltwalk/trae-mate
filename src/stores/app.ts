import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export interface Account {
  id: string
  name: string
  cookie: string
  createdAt: number
  lastCheckinAt?: number
  lastCheckinResult?: 'success' | 'already' | 'failed' | 'pending'
  lastCheckinMessage?: string
  /** 上次签到的接口出入参(JSON 字符串,含 status/claim 的请求/响应),点击"上次签到"查看 */
  lastCheckinTrace?: string
  points?: number
  /** 总积分最近一次真实刷新时间(毫秒) */
  pointsUpdatedAt?: number
  /** 各类型可用积分余额明细(来源 user_current_entitlement_list) */
  pointsDetails?: PointsDetail[]
  /** user_current_entitlement_list 的原始出参(JSON 字符串) */
  pointsResponse?: string
  enabled: boolean
  desktopUserId?: string
  credentialStatus?: 'valid' | 'expiring' | 'expired'
  dataDir?: string
  machineId?: string
  /** 多开实例的签到设备ID(16 位数字 aha 设备ID,首次签到后由 TraeMate 持久化) */
  checkinDeviceId?: string
}

export interface CheckinLog {
  id: string
  accountId: string
  accountName: string
  time: number
  result: 'success' | 'already' | 'failed'
  message: string
  /** 服务端返回的业务错误码(如 9074 设备忙 / 9095 当日已签) */
  errorCode?: number
  pointsGained?: number
  /** 本次签到时的可用积分余额(签到后) */
  pointsBalance?: number
}

export interface CheckinResult {
  success: boolean
  message: string
  errorCode?: number
  points?: number
}

export interface PointsResult {
  success: boolean
  message: string
  totalPoints?: number
  details?: PointsDetail[]
}

/** 单类积分权益的可用余额 */
export interface PointsDetail {
  /** 权益名称/类型,如 "签到奖励" */
  name: string
  /** 总配额 */
  total: number
  /** 剩余可用 */
  remaining: number
  /** 到期时间(毫秒),未知时 0 */
  expireAt: number
}

export interface LaunchResult {
  dataDir: string
  machineId: string
  launched: boolean
}

export interface InstanceState {
  running: boolean
  source: 'none' | 'main' | 'tool'
  isMainAccount: boolean
}

export interface InstanceDirInfo {
  dataDir: string
  userId: string
  accountName: string
  /** token 过期时间(毫秒),0 表示未知 */
  expiresAt: number
  /** 含设备签名密钥(可刷新 token);false 则 token 过期后需重新登录 */
  hasSigningKey: boolean
  running: boolean
  /** 已被应用内账号绑定(同 dataDir 或同 userId) */
  bound: boolean
}

export interface AppSettings {
  autoCheckin: boolean
  checkinTime: string
  retryCount: number
  retryDelay: number
  notifyOnSuccess: boolean
  notifyOnFailed: boolean
  launchAtLogin: boolean
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
  const waitingLogin = ref(false) // 等待新实例登录导入(打开新实例登录后置 true,login-imported 事件后置 false)
  const instanceRefreshTick = ref(0) // 多开状态刷新信号:自增触发各卡片重查
  const nextRunTime = ref<string | null>(null)

  // 计算属性
  const enabledAccounts = computed(() => accounts.value.filter(a => a.enabled))
  const todayCheckedCount = computed(() => {
    const today = new Date().toDateString()
    return accounts.value.filter(a => {
      if (!a.lastCheckinAt) return false
      // 'success' 本次签到成功;'already' 今日已签到(先前已成功签到,本次无需再签)
      const ok = a.lastCheckinResult === 'success' || a.lastCheckinResult === 'already'
      return ok && new Date(a.lastCheckinAt).toDateString() === today
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

  // 保存账号卡片的展示顺序(拖拽排序后调用)
  async function reorderAccounts(order: string[]) {
    try {
      await invoke('reorder_accounts', { order })
      await fetchAccounts()
    } catch (e) {
      console.error('保存账号顺序失败:', e)
      throw e
    }
  }

  // 在途签到的账号 id(防连点重复请求触发服务端限频)
  const checkingIds = new Set<string>()

  async function checkinAccount(id: string) {
    if (checkingIds.has(id)) {
      return { success: false, message: '该账号签到进行中，请稍候' } as CheckinResult
    }
    // 一键签到进行中不接受单账号签到,避免并发请求触发限频
    if (checkingIn.value) {
      return { success: false, message: '一键签到进行中，请稍候' } as CheckinResult
    }
    checkingIds.add(id)
    try {
      checkingIn.value = true
      const result = await invoke<CheckinResult>('checkin_account', { id })
      await fetchAccounts()
      await fetchLogs()
      return result
    } finally {
      checkingIn.value = false
      checkingIds.delete(id)
    }
  }

  // 强制签到(直连领取):跳过状态预检,直接调领取接口。一次性手工补签/状态预检异常时用。
  async function forceCheckinAccount(id: string) {
    if (checkingIn.value) return { success: false, message: '签到进行中，请稍候' } as CheckinResult
    checkingIds.add(id)
    try {
      checkingIn.value = true
      const result = await invoke<CheckinResult>('force_checkin_account', { id })
      await fetchAccounts()
      await fetchLogs()
      return result
    } finally {
      checkingIn.value = false
      checkingIds.delete(id)
    }
  }

  async function checkinAll() {
    let unlisten: UnlistenFn | undefined
    try {
      checkingIn.value = true
      // 后端并行签到:每个账号完成推送 checkin-progress,即时刷新该账号与日志
      unlisten = await listen('checkin-progress', () => {
        fetchAccounts()
        fetchLogs()
      })
      const result = await invoke<Array<[Account, CheckinResult]>>('checkin_all')
      await fetchAccounts()
      await fetchLogs()
      // 签到完成后查询各账号可用积分,与「立即签到」保持行为一致
      const ids = result
        .filter(([, r]) => r?.success)
        .map(([a]) => a.id)
      await Promise.all(ids.map(id => getAccountPoints(id).catch(() => null)))
      await fetchAccounts()
      return result
    } finally {
      unlisten?.()
      unlisten = undefined
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

  /** 导出诊断数据(所有日志 + 账号设备信息)到用户选择的文件,返回导出路径 */
  async function exportDiagnostics(suggestedName: string): Promise<string | null> {
    const { save } = await import('@tauri-apps/plugin-dialog')
    const path = await save({
      title: '导出诊断数据',
      defaultPath: suggestedName,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
    if (!path) return null
    const saved = await invoke<string>('export_logs', { path })
    return saved
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

  // 打开新的空白 TRAE 实例供用户登录(登录后后端自动导入账号并 emit login-imported 事件)
  // waitingLogin 标记等待状态,供界面显示持续提示;login-imported 事件回调中复位
  // 免劫持:客户端正常登录(机器码),签到设备隔离由后端 get_or_create_checkin_device_id
  // 检测机器码相同而改用独立设备自动兜底。
  async function openNewLoginInstance() {
    waitingLogin.value = true
    try {
      await invoke('open_new_login_instance')
    } catch (e) {
      waitingLogin.value = false
      throw e
    }
  }

  // 扫描 %APPDATA% 下已存在的多开/登录临时目录(含登录信息的)
  async function scanInstanceDirs(): Promise<InstanceDirInfo[]> {
    try {
      return await invoke<InstanceDirInfo[]>('scan_instance_dirs')
    } catch (e) {
      console.error('扫描多开目录失败:', e)
      return []
    }
  }

  // 从已有多开目录导入账号(同 userId 已存在则更新凭据并绑定目录)
  async function importAccountFromDir(dataDir: string) {
    const result = await invoke<Account>('import_account_from_dir', { dataDir })
    await fetchAccounts()
    return result
  }

  // 手动刷新账号凭证(实例目录回读 + ExchangeToken 刷新 + 回写)
  async function refreshCredential(id: string) {
    const result = await invoke<Account>('refresh_account_credential', { id })
    await fetchAccounts()
    return result
  }

  // 查询账号实例运行状态(含来源:主实例/工具实例/未运行)
  async function getInstanceState(id: string): Promise<InstanceState> {
    try {
      return await invoke<InstanceState>('get_account_instance_state', { id })
    } catch (e) {
      console.error('查询实例状态失败:', e)
      return { running: false, source: 'none', isMainAccount: false }
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
    waitingLogin,
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
    reorderAccounts,
    checkinAccount,
    forceCheckinAccount,
    checkinAll,
    getAccountPoints,
    fetchLogs,
    clearLogs,
    exportDiagnostics,
    fetchSettings,
    saveSettings,
    fetchNextRunTime,
    launchMulti,
    openNewLoginInstance,
    scanInstanceDirs,
    importAccountFromDir,
    refreshCredential,
    getInstanceState,
    focusInstance,
    refreshInstances,
    getTraeExePath,
    setTraeExePath,
    scanTraeExePath,
    init
  }
})
