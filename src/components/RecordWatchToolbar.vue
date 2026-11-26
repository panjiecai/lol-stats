<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue"
import { BellRing, ClipboardCopy, LoaderCircle, RadioTower } from "lucide-vue-next"
import { loadSelfStats, searchPlayer } from "../api"
import { dispatchRpaNotify, EXPORT_RECENT_DEPTH, useMatchExport } from "../matchExport"
import { notifyKey } from "../notifications"
import { rpaNotifyReady } from "../rpaNotify"
import type { ChampionSummaryItem, GameAssetBundle, RecentGame, SummonerInfo } from "../types"
import MatchOverviewPanel from "./MatchOverviewPanel.vue"

const RECENT_DEPTH = EXPORT_RECENT_DEPTH
const POLL_INTERVAL_MS = 3000
const POLL_INTERVAL_TEXT = `${POLL_INTERVAL_MS / 1000} 秒`
/**
 * 看门狗：单次检查正常几秒内就该返回，超过这个时长说明底层请求卡住了
 * （IPC 挂起 / SGP 无响应）。这时强行放行下一轮，避免整个轮询被一次
 * 挂死的请求永久堵住 —— 这正是「明明有新战绩却不动，要停止再启动」的成因之一。
 */
const POLL_BUSY_TIMEOUT_MS = 25_000
/** 同一局导出连续失败这么多次就放弃重试并推进基线，避免卡在同一局空转 */
const MAX_EXPORT_ATTEMPTS = 3

const props = defineProps<{
  accountQuery?: string
  sgpServerId?: string
  champions: Record<number, ChampionSummaryItem>
  gameAssets: GameAssetBundle
  isSelf?: boolean
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

const pollingActive = ref(false)
const pollingBusy = ref(false)
const lastCheckAt = ref(0)
let pollTimer = 0
let lastSeenGameId: number | null = null
let lastSeenCreation = 0
/** 上一次真正发起检查的时刻，用于识别后台节流 / 系统休眠造成的「很久没跑」 */
let lastTickAt = 0
let busySince = 0
/** 正在等待重试的那一局 */
let pendingGameId: number | null = null
let pendingAttempts = 0

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function latestGameOf(forceRefresh = false) {
  if (props.isSelf) {
    return loadSelfStats(RECENT_DEPTH, forceRefresh, false)
  }
  if (!props.accountQuery) throw new Error("缺少账号查询信息")
  return searchPlayer(props.accountQuery, RECENT_DEPTH, props.sgpServerId, forceRefresh, false)
}

const canOperate = computed(() => props.isSelf || Boolean(props.accountQuery))
const pollHint = computed(() => {
  const base = `每 ${POLL_INTERVAL_TEXT}检查该账号是否打了新对局，发现后自动复制图片`
  return rpaNotifyReady() ? `${base}并发到微信` : base
})

const lastCheckText = computed(() => {
  if (!lastCheckAt.value) return "尚未检查"
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(lastCheckAt.value))
})

const pollButtonTitle = computed(() => {
  if (!pollingActive.value) return pollHint.value
  return `停止每 ${POLL_INTERVAL_TEXT}轮询（上次检查 ${lastCheckText.value}）`
})

/**
 * 导出该账号最新一局（或轮询刚发现的这一局）的己方详情图。
 * 轮询命中时把已经拿到的对局信息传进来，省掉一次重复请求。
 */
async function exportLatestMatch(
  detected = false,
  known?: { game?: RecentGame; summoner?: SummonerInfo },
) {
  if (exporting.value) return false

  const outcome = await exportMatch(
    {
      query: props.accountQuery,
      sgpServerId: props.sgpServerId,
      isSelf: props.isSelf,
      depth: RECENT_DEPTH,
    },
    { known },
  )

  if (!outcome.ok) {
    notify({
      kind: "error",
      title: "导出图片失败",
      message: outcome.message,
      duration: 7000,
    })
    return false
  }

  // 截图已进剪贴板，交给影刀发到微信；这一步失败不影响导出结果
  const rpaText = await dispatchRpaNotify(notify)
  notify({
    kind: "success",
    title: detected ? "发现新对局，图片已复制" : "最新对局图片已复制",
    message: `${outcome.message}${rpaText}，可直接粘贴`,
  })
  return true
}

function togglePolling() {
  if (pollingActive.value) {
    stopPolling()
    return
  }
  void startPolling()
}

async function startPolling() {
  if (pollingActive.value) return
  if (exporting.value || pollingBusy.value) {
    notify({ kind: "info", title: "正在等待当前操作完成", message: "稍后再开始轮询" })
    return
  }

  pollingBusy.value = true
  busySince = Date.now()
  try {
    const stats = await latestGameOf(true)
    const first = stats.recentGames?.[0]
    lastSeenGameId = first?.gameId ?? null
    lastSeenCreation = first?.gameCreation ?? 0
  } catch (error) {
    notify({
      kind: "error",
      title: "获取最新战绩失败",
      message: errorMessage(error),
      duration: 7000,
    })
    return
  } finally {
    pollingBusy.value = false
  }

  pendingGameId = null
  pendingAttempts = 0
  lastTickAt = Date.now()
  lastCheckAt.value = lastTickAt
  pollingActive.value = true
  notify({
    kind: "info",
    title: "轮询已开启",
    message: `每 ${POLL_INTERVAL_TEXT}检查一次最新战绩，发现新对局会自动复制图片`,
  })
  schedulePoll()
}

/**
 * 下一轮延迟：正常按固定周期排；如果距上次检查已经超过两个周期
 * （窗口被后台节流、系统休眠），立刻补一次，不让轮询「装死」。
 */
function pollDelay() {
  if (!lastTickAt) return POLL_INTERVAL_MS
  const elapsed = Date.now() - lastTickAt
  if (elapsed > POLL_INTERVAL_MS * 2) return 0
  return Math.max(0, POLL_INTERVAL_MS - elapsed)
}

function schedulePoll() {
  if (!pollingActive.value) return
  window.clearTimeout(pollTimer)
  pollTimer = window.setTimeout(() => {
    void pollTick()
  }, pollDelay())
}

async function pollTick() {
  if (!pollingActive.value) return
  lastTickAt = Date.now()

  // 上一轮还卡着且没超时 → 顺延一轮；超时就当卡死，强行放行
  if (pollingBusy.value && Date.now() - busySince < POLL_BUSY_TIMEOUT_MS) {
    schedulePoll()
    return
  }

  pollingBusy.value = true
  busySince = Date.now()
  try {
    if (exporting.value) return

    const stats = await latestGameOf(true)
    const first = stats.recentGames?.[0]
    if (!first) return
    if (first.gameId === lastSeenGameId) return
    if (first.gameCreation < lastSeenCreation) return

    if (pendingGameId !== first.gameId) {
      pendingGameId = first.gameId
      pendingAttempts = 0
    }
    pendingAttempts += 1

    // 基线只在导出成功（或重试到上限）后推进：失败不丢帧，下一轮还能补上
    const ok = await exportLatestMatch(true, { game: first, summoner: stats.summoner })
    if (ok || pendingAttempts >= MAX_EXPORT_ATTEMPTS) {
      lastSeenGameId = first.gameId
      lastSeenCreation = first.gameCreation
      pendingGameId = null
      pendingAttempts = 0
    }
  } catch {
    // 单次轮询失败不打断定时任务，下一次继续
  } finally {
    pollingBusy.value = false
    lastCheckAt.value = Date.now()
    schedulePoll()
  }
}

function stopPolling(options: { silent?: boolean } = {}) {
  if (!pollingActive.value && !pollTimer) return
  pollingActive.value = false
  if (pollTimer) {
    window.clearTimeout(pollTimer)
    pollTimer = 0
  }
  if (options.silent) return
  notify({ kind: "info", title: "轮询已停止", message: "已停止检查最新战绩" })
}

/**
 * 窗口从后台回到前台时补检查：WebView 在窗口隐藏期间会把定时器节流到分钟级
 * 甚至暂停，回到前台若不主动补一次，看上去就跟「卡住了」一样。
 */
function resumeAfterHidden() {
  if (!pollingActive.value) return
  if (!lastTickAt || Date.now() - lastTickAt >= POLL_INTERVAL_MS) {
    lastTickAt = Date.now()
    void pollTick()
  } else {
    schedulePoll()
  }
}

function handleVisibilityChange() {
  if (document.visibilityState === "visible") resumeAfterHidden()
}

onMounted(() => {
  document.addEventListener("visibilitychange", handleVisibilityChange)
  window.addEventListener("focus", resumeAfterHidden)
})

// 切换账号或区服时重置轮询，避免用旧账号的基线继续判断新对局。
watch(
  () => [props.accountQuery, props.sgpServerId, props.isSelf],
  () => {
    lastSeenGameId = null
    lastSeenCreation = 0
    pendingGameId = null
    pendingAttempts = 0
    stopPolling({ silent: true })
  },
)

onBeforeUnmount(() => {
  document.removeEventListener("visibilitychange", handleVisibilityChange)
  window.removeEventListener("focus", resumeAfterHidden)
  stopPolling({ silent: true })
})
</script>

<template>
  <div class="record-watch">
    <button
      class="record-action"
      :disabled="exporting || !canOperate"
      :title="'导出最新一局中该账号所在队伍的详情图片到剪贴板'"
      @click="exportLatestMatch(false)"
    >
      <LoaderCircle v-if="exporting" class="spin" :size="14" />
      <ClipboardCopy v-else class="action-icon" :size="14" />
      {{ exporting ? "生成中" : "导出最新对局" }}
    </button>
    <button
      class="record-action polling"
      :class="{ active: pollingActive }"
      :disabled="!canOperate"
      :title="pollButtonTitle"
      @click="togglePolling"
    >
      <RadioTower v-if="pollingActive" class="pulse" :size="14" />
      <BellRing v-else class="action-icon" :size="14" />
      {{ pollingActive ? "停止轮询" : "轮询新战绩" }}
    </button>

    <Teleport to="body">
      <div
        v-if="exportGame && exportDetail"
        :ref="bindStage"
        class="export-stage"
        aria-hidden="true"
      >
        <MatchOverviewPanel
          :game="exportGame"
          :match-detail="exportDetail"
          :loading="false"
          :error="''"
          :champions="champions"
          :game-assets="gameAssets"
          :sgp-server-id="sgpServerId"
          :owner-label="exportOwnerLabel"
          :owner-puuid="exportOwnerPuuid"
          :team-only="exportTeamId"
          compact-columns
          hide-actions
        />
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.record-watch {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
}

.record-action {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border: 1px solid #d6e5e1;
  border-radius: 8px;
  color: #315f58;
  background: #ffffff;
  cursor: pointer;
  font-size: 13px;
  font-weight: 800;
  padding: 7px 10px;
}

.record-action:hover:not(:disabled),
.record-action.polling.active {
  color: #ffffff;
  background: #1f5f56;
}

.record-action.polling.active:hover {
  color: #ffffff;
  background: #a94745;
}

.record-action:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.action-icon {
  flex: 0 0 auto;
}

.pulse {
  animation: record-pulse 1.4s ease-in-out infinite;
}

@keyframes record-pulse {
  0%,
  100% {
    opacity: 1;
  }

  50% {
    opacity: 0.35;
  }
}

.spin {
  animation: record-spin 0.9s linear infinite;
}

@keyframes record-spin {
  to {
    transform: rotate(360deg);
  }
}

/*
 * 导出舞台宽度必须装得下 MatchOverviewPanel 的紧凑列，否则 team-block 的
 * overflow-x 会把右侧的伤害列裁掉。紧凑列 = 160 + 22 + 59 + 96 + 3 个 4px 间距 = 349，
 * 面板 padding 12/边框 1 + team-block padding 8/边框 1 共 44 → 393，取 420 留余量。
 * （与 src/matchExport.ts 的 EXPORT_STAGE_WIDTH 保持一致）
 */
.export-stage {
  position: fixed;
  left: 0;
  top: 0;
  z-index: 2147483000;
  width: 420px;
  background: #f6faf9;
}
</style>
