<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue"
import {
  AlertTriangle,
  CheckCircle2,
  LoaderCircle,
  Pause,
  Play,
  Plus,
  RadioTower,
  Trash2,
  Zap,
} from "lucide-vue-next"
import { searchPlayer } from "../api"
import { dispatchRpaNotify, matchExportBusy, useMatchExport } from "../matchExport"
import { notifyKey } from "../notifications"
import { searchServerLabel, searchServerOptions } from "../servers"
import type { ChampionSummaryItem, GameAssetBundle, RecentGame, SummonerInfo } from "../types"
import { riotId } from "../utils"
import MatchOverviewPanel from "./MatchOverviewPanel.vue"
import WxPushPanel from "./WxPushPanel.vue"

/**
 * 多账号监控面板：同时盯住一批账号，谁打完一局就自动导出详情图（可选并发影刀微信推送）。
 *
 * 轮询策略：单条自循环调度，每 tick 只检查「最久没查过」的那一个账号，
 * 所以 N 个账号天然错峰，不会在同一秒轰出一堆 SGP 请求；慢请求也不会拖住别人。
 * 导出走队列串行执行 —— 截图要独占剪贴板，不能并发。
 */

const MONITOR_DEPTH = 20
/** 同时最多几个检查在飞，避免账号多的时候把请求堆成一坨 */
const MAX_IN_FLIGHT = 2
/** 单个账号导出失败的队列内重试上限 */
const MAX_QUEUE_ATTEMPTS = 3

const ACCOUNTS_KEY = "lol-stats.monitor.accounts"
const SETTINGS_KEY = "lol-stats.monitor.settings"

type MonitorStatus = "idle" | "checking" | "ok" | "new" | "error"

type MonitorAccount = {
  id: string
  query: string
  sgpServerId: string
  /** 是否参与轮询 */
  enabled: boolean
  /** 命中新对局时是否自动导出并发微信 */
  autoExport: boolean
  /** 已建立基线的最新对局；null = 还没建立（刚加进来时只记基线，不导出历史对局） */
  baselineGameId: number | null
  baselineCreation: number
  lastCheckedAt: number
  lastFoundAt: number
  lastError: string
  foundCount: number
  status: MonitorStatus
  /** 展示名，查到召唤师信息后替换成真实 GameName#Tag */
  label: string
}

type MonitorSettings = {
  /** 调度节拍：每这么久检查一个账号 */
  tickMs: number
  /** 新账号加入时是否默认自动导出 */
  autoExportDefault: boolean
  /** 打开应用自动开始监控 */
  autoStart: boolean
}

type ExportJob = {
  accountId: string
  label: string
  game: RecentGame
  summoner?: SummonerInfo
  attempts: number
}

defineProps<{
  champions: Record<number, ChampionSummaryItem>
  gameAssets: GameAssetBundle
}>()

const notify = inject(notifyKey, () => 0)

const {
  exporting,
  exportGame,
  exportDetail,
  exportTeamId,
  exportOwnerLabel,
  exportOwnerPuuid,
  exportMatch,
  bindStage,
} = useMatchExport()

const DEFAULT_SETTINGS: MonitorSettings = {
  tickMs: 1200,
  autoExportDefault: true,
  autoStart: false,
}

function delay(ms: number) {
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

function readSettings(): MonitorSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY)
    if (!raw) return { ...DEFAULT_SETTINGS }
    const parsed = JSON.parse(raw) as Partial<MonitorSettings>
    return {
      tickMs: clampTick(parsed.tickMs),
      autoExportDefault: parsed.autoExportDefault ?? DEFAULT_SETTINGS.autoExportDefault,
      autoStart: parsed.autoStart ?? DEFAULT_SETTINGS.autoStart,
    }
  } catch {
    return { ...DEFAULT_SETTINGS }
  }
}

function clampTick(value: unknown) {
  const number = Number(value)
  if (!Number.isFinite(number)) return DEFAULT_SETTINGS.tickMs
  return Math.min(10_000, Math.max(400, Math.round(number)))
}

function newId() {
  const hasRandomUuid =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
  return hasRandomUuid ? crypto.randomUUID() : `acc-${Date.now()}-${Math.random().toString(36).slice(2)}`
}

function createAccount(query: string, sgpServerId: string, autoExport: boolean): MonitorAccount {
  return {
    id: newId(),
    query,
    sgpServerId,
    enabled: true,
    autoExport,
    baselineGameId: null,
    baselineCreation: 0,
    lastCheckedAt: 0,
    lastFoundAt: 0,
    lastError: "",
    foundCount: 0,
    status: "idle",
    label: query,
  }
}

function readAccounts(autoExportDefault: boolean): MonitorAccount[] {
  try {
    const raw = localStorage.getItem(ACCOUNTS_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as Array<Partial<MonitorAccount>>
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((item) => typeof item?.query === "string" && item.query.trim())
      .map((item) => {
        const query = item.query!.trim()
        return {
          ...createAccount(query, item.sgpServerId || "TENCENT_HN1", item.autoExport ?? autoExportDefault),
          id: item.id || newId(),
          enabled: item.enabled ?? true,
          baselineGameId: typeof item.baselineGameId === "number" ? item.baselineGameId : null,
          baselineCreation: typeof item.baselineCreation === "number" ? item.baselineCreation : 0,
          foundCount: typeof item.foundCount === "number" ? item.foundCount : 0,
          lastFoundAt: typeof item.lastFoundAt === "number" ? item.lastFoundAt : 0,
          label: item.label || query,
        }
      })
  } catch {
    return []
  }
}

const settings = ref<MonitorSettings>(readSettings())
const accounts = ref<MonitorAccount[]>(readAccounts(settings.value.autoExportDefault))
const monitoring = ref(false)

const draftQuery = ref("")
const draftServerId = ref("TENCENT_HN1")

let schedulerTimer = 0
let inFlight = 0
let lastTickAt = 0
const exportQueue: ExportJob[] = []
let queueRunning = false
let persistTimer = 0

const enabledAccounts = computed(() => accounts.value.filter((account) => account.enabled))
const totalFound = computed(() => accounts.value.reduce((sum, account) => sum + account.foundCount, 0))
const busyText = computed(() => {
  if (exporting.value) return "正在导出截图"
  if (exportQueue.length) return `导出队列 ${exportQueue.length} 条`
  return ""
})

const tickSeconds = computed({
  get: () => settings.value.tickMs / 1000,
  set: (value: number) => {
    settings.value.tickMs = clampTick(value * 1000)
    if (monitoring.value) schedule()
  },
})

/** 账号全部检查一轮的大致耗时，用来给用户一个「每个账号多久查一次」的直觉 */
const fullCycleText = computed(() => {
  const count = enabledAccounts.value.length || 1
  const seconds = (settings.value.tickMs * count) / 1000
  return seconds >= 60 ? `${(seconds / 60).toFixed(1)} 分钟` : `${seconds.toFixed(1)} 秒`
})

function persistNow() {
  localStorage.setItem(
    ACCOUNTS_KEY,
    JSON.stringify(
      accounts.value.map((account) => ({
        id: account.id,
        query: account.query,
        sgpServerId: account.sgpServerId,
        enabled: account.enabled,
        autoExport: account.autoExport,
        baselineGameId: account.baselineGameId,
        baselineCreation: account.baselineCreation,
        foundCount: account.foundCount,
        lastFoundAt: account.lastFoundAt,
        label: account.label,
      })),
    ),
  )
  localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings.value))
}

function schedulePersist() {
  window.clearTimeout(persistTimer)
  persistTimer = window.setTimeout(persistNow, 500)
}

function findAccount(id: string) {
  return accounts.value.find((account) => account.id === id) || null
}

function addAccount() {
  const query = draftQuery.value.trim()
  if (!query) return

  if (accounts.value.some((account) => account.query.toLowerCase() === query.toLowerCase() && account.sgpServerId === draftServerId.value)) {
    notify({ kind: "info", title: "已经在监控列表里", message: `${query} 无需重复添加` })
    return
  }

  accounts.value.push(createAccount(query, draftServerId.value, settings.value.autoExportDefault))
  draftQuery.value = ""
  notify({
    kind: "success",
    title: "已加入监控",
    message: `${query}（${searchServerLabel(draftServerId.value)}）首轮只记录基线，之后的下一局才会触发导出`,
  })
  schedulePersist()
}

function removeAccount(account: MonitorAccount) {
  accounts.value = accounts.value.filter((item) => item.id !== account.id)
  schedulePersist()
}

function clearAll() {
  if (!accounts.value.length) return
  accounts.value = []
  schedulePersist()
  notify({ kind: "info", title: "已清空监控列表", message: "账号列表已删除" })
}

function startMonitoring() {
  if (monitoring.value) return
  if (!enabledAccounts.value.length) {
    notify({ kind: "info", title: "没有可监控的账号", message: "先在下面添加账号并勾选「监控」" })
    return
  }
  monitoring.value = true
  lastTickAt = 0
  notify({
    kind: "info",
    title: "监控已开启",
    message: `共 ${enabledAccounts.value.length} 个账号，每个约 ${fullCycleText.value} 检查一次，发现新对局自动导出`,
  })
  void tick()
}

function stopMonitoring(silent = false) {
  if (!monitoring.value && !schedulerTimer) return
  monitoring.value = false
  if (schedulerTimer) {
    window.clearTimeout(schedulerTimer)
    schedulerTimer = 0
  }
  for (const account of accounts.value) {
    if (account.status === "checking") account.status = "idle"
  }
  if (silent) return
  notify({ kind: "info", title: "监控已停止", message: "不再检查这些账号的新对局" })
}

function schedule() {
  if (!monitoring.value) return
  window.clearTimeout(schedulerTimer)
  const elapsed = lastTickAt ? Date.now() - lastTickAt : 0
  const wait = elapsed > settings.value.tickMs * 2 ? 0 : Math.max(0, settings.value.tickMs - elapsed)
  schedulerTimer = window.setTimeout(() => {
    void tick()
  }, wait)
}

async function tick() {
  if (!monitoring.value) return
  lastTickAt = Date.now()

  const list = enabledAccounts.value
  if (!list.length) {
    schedule()
    return
  }
  if (inFlight >= MAX_IN_FLIGHT) {
    schedule()
    return
  }

  // 挑「最久没检查」的账号，慢的那几个不会饿死别人
  const target = list.reduce((oldest, item) =>
    (item.lastCheckedAt || 0) < (oldest.lastCheckedAt || 0) ? item : oldest,
  )

  inFlight += 1
  void checkAccount(target).finally(() => {
    inFlight -= 1
  })
  schedule()
}

async function checkAccount(account: MonitorAccount) {
  const current = findAccount(account.id)
  if (!current || !current.enabled) return

  current.status = "checking"
  try {
    const stats = await searchPlayer(current.query, MONITOR_DEPTH, current.sgpServerId, true, false)
    const live = findAccount(account.id)
    if (!live) return

    live.lastCheckedAt = Date.now()
    live.lastError = ""
    if (stats.summoner) live.label = riotId(stats.summoner)

    const first = stats.recentGames?.[0]
    if (!first) {
      live.status = "ok"
      return
    }

    if (live.baselineGameId === null) {
      // 首次只记基线：把「加入之前就已经打完的那一局」当成新对局发出去会很吵
      live.baselineGameId = first.gameId
      live.baselineCreation = first.gameCreation
      live.status = "ok"
      return
    }

    if (first.gameId === live.baselineGameId || first.gameCreation < live.baselineCreation) {
      live.status = "ok"
      return
    }

    live.baselineGameId = first.gameId
    live.baselineCreation = first.gameCreation
    live.foundCount += 1
    live.lastFoundAt = Date.now()
    live.status = "new"

    if (live.autoExport) {
      enqueueExport({ accountId: live.id, label: live.label, game: first, summoner: stats.summoner, attempts: 0 })
    }
  } catch (error) {
    const live = findAccount(account.id)
    if (!live) return
    live.status = "error"
    live.lastCheckedAt = Date.now()
    live.lastError = error instanceof Error ? error.message : String(error)
  }
}

function enqueueExport(job: ExportJob) {
  exportQueue.push(job)
  void drainQueue()
}

/** 等手动导出/别的队列让出全局导出锁，避免两个截图抢剪贴板 */
async function waitExportIdle(timeoutMs = 40_000) {
  const start = Date.now()
  while (matchExportBusy() && Date.now() - start < timeoutMs) {
    await delay(400)
  }
}

async function drainQueue() {
  if (queueRunning) return
  queueRunning = true
  try {
    while (exportQueue.length) {
      const job = exportQueue[0]
      const account = findAccount(job.accountId)
      if (!account) {
        exportQueue.shift()
        continue
      }

      await waitExportIdle()
      const outcome = await exportMatch(
        { query: account.query, sgpServerId: account.sgpServerId, depth: MONITOR_DEPTH },
        { known: { game: job.game, summoner: job.summoner } },
      )

      if (!outcome.ok) {
        job.attempts += 1
        if (job.attempts < MAX_QUEUE_ATTEMPTS) {
          await delay(2500)
          continue
        }
        exportQueue.shift()
        notify({
          kind: "error",
          title: `${job.label} 导出失败`,
          message: outcome.message,
          duration: 7000,
        })
        continue
      }

      exportQueue.shift()
      const rpaText = await dispatchRpaNotify(notify)
      notify({
        kind: "success",
        title: `发现新对局：${job.label}`,
        message: `${outcome.message}${rpaText}，可直接粘贴`,
        duration: 8000,
      })
    }
  } finally {
    queueRunning = false
  }
}

/** 单账号手动导出一局（不依赖监控是否开启） */
async function exportOne(account: MonitorAccount) {
  if (exporting.value) {
    notify({ kind: "info", title: "正在导出中", message: "等当前截图完成再试" })
    return
  }
  await waitExportIdle()
  const outcome = await exportMatch({ query: account.query, sgpServerId: account.sgpServerId, depth: MONITOR_DEPTH })
  if (!outcome.ok) {
    notify({ kind: "error", title: `${account.label} 导出失败`, message: outcome.message, duration: 7000 })
    return
  }
  const rpaText = await dispatchRpaNotify(notify)
  notify({
    kind: "success",
    title: `${account.label} 最新对局已复制`,
    message: `${outcome.message}${rpaText}，可直接粘贴`,
  })
}

function statusMeta(account: MonitorAccount) {
  if (!account.enabled) return { tone: "off", text: "已暂停", icon: Pause }
  if (account.status === "checking") return { tone: "checking", text: "检查中", icon: LoaderCircle }
  if (account.status === "error") return { tone: "error", text: "查询出错", icon: AlertTriangle }
  if (account.status === "new") return { tone: "new", text: "发现新对局", icon: Zap }
  if (!account.baselineGameId) return { tone: "idle", text: "等待建立基线", icon: RadioTower }
  return { tone: "ok", text: "监控中", icon: CheckCircle2 }
}

function formatClock(timestamp: number) {
  if (!timestamp) return "-"
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(timestamp))
}

function resumeAfterHidden() {
  if (!monitoring.value) return
  if (!lastTickAt || Date.now() - lastTickAt >= settings.value.tickMs) {
    lastTickAt = Date.now()
    void tick()
  } else {
    schedule()
  }
}

function handleVisibilityChange() {
  if (document.visibilityState === "visible") resumeAfterHidden()
}

watch(
  accounts,
  () => {
    schedulePersist()
  },
  { deep: true },
)

watch(
  settings,
  () => {
    schedulePersist()
    if (monitoring.value) schedule()
  },
  { deep: true },
)

onMounted(() => {
  document.addEventListener("visibilitychange", handleVisibilityChange)
  window.addEventListener("focus", resumeAfterHidden)
  if (settings.value.autoStart && enabledAccounts.value.length) {
    startMonitoring()
  }
})

onBeforeUnmount(() => {
  document.removeEventListener("visibilitychange", handleVisibilityChange)
  window.removeEventListener("focus", resumeAfterHidden)
  window.clearTimeout(persistTimer)
  stopMonitoring(true)
})
</script>

<template>
  <section class="monitor-panel">
    <header class="page-header">
      <div>
        <div class="eyebrow">Multi Account Watch</div>
        <h2>监控面板</h2>
      </div>
      <div class="header-actions">
        <span class="busy-chip" v-if="busyText">{{ busyText }}</span>
        <button class="primary-action" :class="{ danger: monitoring }" @click="monitoring ? stopMonitoring() : startMonitoring()">
          <Pause v-if="monitoring" :size="16" />
          <Play v-else :size="16" />
          {{ monitoring ? "停止监控" : "开始监控" }}
        </button>
      </div>
    </header>

    <section class="monitor-stats">
      <div class="stat">
        <span>监控账号</span>
        <strong>{{ enabledAccounts.length }} / {{ accounts.length }}</strong>
      </div>
      <div class="stat">
        <span>累计发现</span>
        <strong>{{ totalFound }} 场</strong>
      </div>
      <div class="stat">
        <span>单账号间隔</span>
        <strong>约 {{ fullCycleText }}</strong>
      </div>
      <label class="stat-control">
        <span>调度节拍</span>
        <input v-model.number="tickSeconds" type="number" min="0.4" max="10" step="0.1" />
        <em>秒 / 次</em>
      </label>
      <label class="stat-toggle">
        <input v-model="settings.autoExportDefault" type="checkbox" />
        <span>新账号默认自动导出</span>
      </label>
      <label class="stat-toggle">
        <input v-model="settings.autoStart" type="checkbox" />
        <span>打开软件自动开始</span>
      </label>
    </section>

    <section class="add-row">
      <select v-model="draftServerId" aria-label="区服">
        <option v-for="server in searchServerOptions" :key="server.id" :value="server.id">
          {{ server.label }}
        </option>
      </select>
      <input
        v-model="draftQuery"
        placeholder="GameName#Tag 或 PUUID"
        @keyup.enter="addAccount"
      />
      <button class="add-button" :disabled="!draftQuery.trim()" @click="addAccount">
        <Plus :size="15" />
        添加账号
      </button>
    </section>

    <p class="hint">
      区服必须和账号实际所在大区一致，选错会静默返回空战绩；每添加一个账号，第一次检查只记录基线，
      之后打完的新对局才会触发导出。导出和微信推送沿用「轮询新战绩」那套流程，多个账号命中时按顺序逐个截图。
    </p>

    <WxPushPanel />

    <section class="account-list">
      <div class="list-head" v-if="accounts.length">
        <span>账号</span>
        <span>状态</span>
        <span>上次检查</span>
        <span>发现</span>
        <span>操作</span>
      </div>

      <p class="empty" v-if="!accounts.length">还没有监控账号，先在上面添加。</p>

      <article
        v-for="account in accounts"
        :key="account.id"
        class="account-row"
        :class="[`tone-${statusMeta(account).tone}`]"
      >
        <div class="cell-account">
          <strong>{{ account.label }}</strong>
          <em>{{ searchServerLabel(account.sgpServerId) }} · {{ account.query }}</em>
        </div>

        <div class="cell-status">
          <span class="status-chip">
            <component :is="statusMeta(account).icon" :size="13" :class="{ spin: account.status === 'checking' }" />
            {{ statusMeta(account).text }}
          </span>
          <em class="error-text" v-if="account.lastError" :title="account.lastError">{{ account.lastError }}</em>
        </div>

        <div class="cell-clock">
          <span>{{ formatClock(account.lastCheckedAt) }}</span>
          <em v-if="account.lastFoundAt">最近命中 {{ formatClock(account.lastFoundAt) }}</em>
        </div>

        <div class="cell-found">{{ account.foundCount }}</div>

        <div class="cell-actions">
          <label class="mini-toggle" :title="'是否参与轮询'">
            <input v-model="account.enabled" type="checkbox" />
            <span>监控</span>
          </label>
          <label class="mini-toggle" :title="'命中新对局时是否自动导出并推送到微信'">
            <input v-model="account.autoExport" type="checkbox" />
            <span>自动导出</span>
          </label>
          <button class="icon-button" :disabled="exporting" title="立即导出该账号最新一局" @click="exportOne(account)">
            <Zap :size="14" />
          </button>
          <button class="icon-button danger" title="从监控列表移除" @click="removeAccount(account)">
            <Trash2 :size="14" />
          </button>
        </div>
      </article>
    </section>

    <div class="list-footer" v-if="accounts.length">
      <button class="ghost-button" @click="clearAll">清空列表</button>
    </div>

    <Teleport to="body">
      <div v-if="exportGame && exportDetail" :ref="bindStage" class="export-stage" aria-hidden="true">
        <MatchOverviewPanel
          :game="exportGame"
          :match-detail="exportDetail"
          :loading="false"
          :error="''"
          :champions="champions"
          :game-assets="gameAssets"
          :owner-label="exportOwnerLabel"
          :owner-puuid="exportOwnerPuuid"
          :team-only="exportTeamId"
          compact-columns
          hide-actions
        />
      </div>
    </Teleport>
  </section>
</template>

<style scoped>
/* 外层 .workspace 已经给了 26px 内边距，这里不再叠加左右 padding */
.monitor-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.page-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
}

.eyebrow {
  color: #6b8a84;
  font-size: 12px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.page-header h2 {
  margin: 4px 0 0;
  color: #12312c;
  font-size: 22px;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.busy-chip {
  border-radius: 999px;
  color: #1c5a70;
  background: #dcf0f8;
  font-size: 12px;
  font-weight: 800;
  padding: 5px 10px;
}

.primary-action {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: none;
  border-radius: 9px;
  color: #ffffff;
  background: #1f5f56;
  cursor: pointer;
  font-size: 13px;
  font-weight: 800;
  padding: 9px 16px;
}

.primary-action.danger {
  background: #a94745;
}

.monitor-stats {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px 18px;
  border: 1px solid #d6e5e1;
  border-radius: 12px;
  background: #ffffff;
  padding: 12px 14px;
}

.stat {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 92px;
}

.stat span {
  color: #6b8a84;
  font-size: 11px;
  font-weight: 700;
}

.stat strong {
  color: #12312c;
  font-size: 16px;
}

.stat-control,
.stat-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: #315f58;
  font-size: 12px;
  font-weight: 700;
}

.stat-control input {
  width: 72px;
  border: 1px solid #d6e5e1;
  border-radius: 7px;
  color: #12312c;
  font-size: 13px;
  font-weight: 700;
  padding: 5px 8px;
}

.stat-control em {
  color: #8aa39d;
  font-style: normal;
  font-size: 11px;
}

.add-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.add-row select,
.add-row input {
  border: 1px solid #d6e5e1;
  border-radius: 9px;
  color: #12312c;
  background: #ffffff;
  font-size: 13px;
  padding: 9px 10px;
}

.add-row select {
  min-width: 120px;
}

.add-row input {
  flex: 1 1 240px;
  min-width: 200px;
}

.add-button {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border: 1px solid #1f5f56;
  border-radius: 9px;
  color: #ffffff;
  background: #1f5f56;
  cursor: pointer;
  font-size: 13px;
  font-weight: 800;
  padding: 9px 14px;
}

.add-button:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.hint {
  margin: 0;
  color: #6b8a84;
  font-size: 12px;
  line-height: 1.7;
}

.account-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.list-head {
  display: grid;
  grid-template-columns: minmax(180px, 1.6fr) 130px 150px 60px minmax(260px, 1.4fr);
  gap: 10px;
  color: #8aa39d;
  font-size: 11px;
  font-weight: 800;
  letter-spacing: 0.04em;
  padding: 0 12px;
}

.empty {
  margin: 0;
  border: 1px dashed #d6e5e1;
  border-radius: 12px;
  color: #8aa39d;
  font-size: 13px;
  padding: 24px;
  text-align: center;
}

.account-row {
  display: grid;
  grid-template-columns: minmax(180px, 1.6fr) 130px 150px 60px minmax(260px, 1.4fr);
  gap: 10px;
  align-items: center;
  border: 1px solid #d6e5e1;
  border-left: 3px solid #cbdad6;
  border-radius: 12px;
  background: #ffffff;
  padding: 10px 12px;
}

.account-row.tone-ok {
  border-left-color: #1f5f56;
}

.account-row.tone-checking {
  border-left-color: #2b7fa8;
}

.account-row.tone-new {
  border-left-color: #d98b18;
  background: #fffaf0;
}

.account-row.tone-error {
  border-left-color: #a94745;
  background: #fff7f6;
}

.account-row.tone-off {
  opacity: 0.62;
}

.cell-account {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.cell-account strong {
  overflow: hidden;
  color: #12312c;
  font-size: 14px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-account em {
  overflow: hidden;
  color: #8aa39d;
  font-style: normal;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-status {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.status-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  width: fit-content;
  border-radius: 999px;
  color: #315f58;
  background: #eef5f3;
  font-size: 12px;
  font-weight: 800;
  padding: 4px 9px;
}

.tone-new .status-chip {
  color: #8a5a08;
  background: #fdf0d5;
}

.tone-error .status-chip {
  color: #8d3b39;
  background: #fbe3e2;
}

.tone-checking .status-chip {
  color: #1c5a70;
  background: #dcf0f8;
}

.error-text {
  overflow: hidden;
  color: #8d3b39;
  font-style: normal;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-clock {
  display: flex;
  flex-direction: column;
  gap: 2px;
  color: #315f58;
  font-size: 13px;
  font-weight: 700;
}

.cell-clock em {
  color: #8aa39d;
  font-style: normal;
  font-size: 11px;
  font-weight: 600;
}

.cell-found {
  color: #12312c;
  font-size: 16px;
  font-weight: 800;
}

.cell-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.mini-toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: #315f58;
  font-size: 12px;
  font-weight: 700;
}

.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid #d6e5e1;
  border-radius: 8px;
  color: #315f58;
  background: #ffffff;
  cursor: pointer;
  padding: 6px 8px;
}

.icon-button:hover:not(:disabled) {
  color: #ffffff;
  background: #1f5f56;
}

.icon-button.danger:hover {
  background: #a94745;
}

.icon-button:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.list-footer {
  display: flex;
  justify-content: flex-end;
}

.ghost-button {
  border: 1px solid #d6e5e1;
  border-radius: 9px;
  color: #6b8a84;
  background: #ffffff;
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
  padding: 7px 12px;
}

.spin {
  animation: monitor-spin 0.9s linear infinite;
}

@keyframes monitor-spin {
  to {
    transform: rotate(360deg);
  }
}

/* 导出舞台宽度与 .export-stage 保持一致，见 RecordWatchToolbar.vue 的说明 */
.export-stage {
  position: fixed;
  left: 0;
  top: 0;
  z-index: 2147483000;
  width: 420px;
  background: #f6faf9;
}
</style>
