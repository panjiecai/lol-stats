<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, ref, watch } from "vue"
import { BellRing, ClipboardCopy, LoaderCircle, RadioTower } from "lucide-vue-next"
import { loadMatchDetail, loadSelfStats, searchPlayer } from "../api"
import { copyElementAsPng } from "../imageShare"
import { notifyKey } from "../notifications"
import type {
  ChampionSummaryItem,
  GameAssetBundle,
  MatchDetailResponse,
  RecentGame,
} from "../types"
import { riotId } from "../utils"
import MatchOverviewPanel from "./MatchOverviewPanel.vue"

const RECENT_DEPTH = 20
const POLL_INTERVAL_MS = 3000
const EXPORT_RENDER_WAIT_MS = 250
const POLL_INTERVAL_TEXT = `${POLL_INTERVAL_MS / 1000} 秒`

const props = defineProps<{
  accountQuery?: string
  sgpServerId?: string
  champions: Record<number, ChampionSummaryItem>
  gameAssets: GameAssetBundle
  isSelf?: boolean
}>()

const notify = inject(notifyKey, () => 0)

const exporting = ref(false)
const exportStage = ref<HTMLElement | null>(null)
const exportGame = ref<RecentGame | null>(null)
const exportDetail = ref<MatchDetailResponse | null>(null)
const exportTeamId = ref<number | undefined>(undefined)
const exportOwnerLabel = ref("")
const exportOwnerPuuid = ref("")

const pollingActive = ref(false)
const pollingBusy = ref(false)
let pollTimer = 0
let lastSeenGameId: number | null = null
let lastSeenCreation = 0

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
const pollHint = computed(
  () => `每 ${POLL_INTERVAL_TEXT}检查该账号是否打了新对局，发现后自动复制图片`,
)

async function exportLatestMatch(detected = false) {
  if (exporting.value) return false

  exporting.value = true
  try {
    const stats = await latestGameOf(true)
    const summoner = stats.summoner
    const latest = stats.recentGames?.[0]
    if (!summoner || !latest) {
      notify({ kind: "error", title: "暂时没有战绩", message: "请确认该账号有对局记录" })
      return false
    }

    const detail = await loadMatchDetail(latest.gameId, props.sgpServerId)
    const ownerTeam = detail.teams.find((team) =>
      team.players.some((player) => player.puuid === summoner.puuid),
    )
    if (!ownerTeam) {
      notify({ kind: "error", title: "导出失败", message: "无法在该对局中找到此账号" })
      return false
    }

    exportGame.value = latest
    exportDetail.value = detail
    exportTeamId.value = ownerTeam.teamId
    exportOwnerLabel.value = riotId(summoner)
    exportOwnerPuuid.value = summoner.puuid
    await nextTick()
    await document.fonts?.ready
    await new Promise((resolve) => window.setTimeout(resolve, EXPORT_RENDER_WAIT_MS))

    const target = exportStage.value
    if (!target) throw new Error("导出区域未渲染")

    const size = await copyElementAsPng(target, {
      backgroundColor: "#f6faf9",
      pixelRatio: 2,
      filter: (node) => !(node instanceof HTMLElement && node.classList.contains("overview-actions")),
    })

    // 截图完成立即隐藏覆盖层，避免遮挡界面
    exportGame.value = null
    exportDetail.value = null
    exportTeamId.value = undefined
    await nextTick()

    notify({
      kind: "success",
      title: detected ? "发现新对局，图片已复制" : "最新对局图片已复制",
      message: size
        ? `己方详情已复制（${size.width}×${size.height}），可直接粘贴`
        : "己方详情已复制，可直接粘贴",
    })
    return true
  } catch (error) {
    notify({
      kind: "error",
      title: "导出图片失败",
      message: errorMessage(error),
      duration: 7000,
    })
    return false
  } finally {
    exporting.value = false
  }
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

  pollingActive.value = true
  notify({
    kind: "info",
    title: "轮询已开启",
    message: `每 ${POLL_INTERVAL_TEXT}检查一次最新战绩，发现新对局会自动复制图片`,
  })
  pollTimer = window.setInterval(() => {
    void pollTick()
  }, POLL_INTERVAL_MS)
}

async function pollTick() {
  if (pollingBusy.value) return
  pollingBusy.value = true
  try {
    if (exporting.value) return
    const stats = await latestGameOf(true)
    const first = stats.recentGames?.[0]
    if (!first) return
    const isNew = first.gameId !== lastSeenGameId && first.gameCreation >= lastSeenCreation
    if (!isNew) return

    lastSeenGameId = first.gameId
    lastSeenCreation = first.gameCreation
    await exportLatestMatch(true)
  } catch {
    // 单次轮询失败不打断定时任务，下一次继续
  } finally {
    pollingBusy.value = false
  }
}

function stopPolling(options: { silent?: boolean } = {}) {
  if (!pollingActive.value && !pollTimer) return
  pollingActive.value = false
  if (pollTimer) {
    window.clearInterval(pollTimer)
    pollTimer = 0
  }
  if (options.silent) return
  notify({ kind: "info", title: "轮询已停止", message: "已停止检查最新战绩" })
}

// 切换账号或区服时重置轮询，避免用旧账号的基线继续判断新对局。
watch(
  () => [props.accountQuery, props.sgpServerId, props.isSelf],
  () => {
    lastSeenGameId = null
    lastSeenCreation = 0
    stopPolling({ silent: true })
  },
)

onBeforeUnmount(() => {
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
      :title="pollingActive ? `停止每 ${POLL_INTERVAL_TEXT}轮询` : pollHint"
      @click="togglePolling"
    >
      <RadioTower v-if="pollingActive" class="pulse" :size="14" />
      <BellRing v-else class="action-icon" :size="14" />
      {{ pollingActive ? "停止轮询" : "轮询新战绩" }}
    </button>

    <Teleport to="body">
      <div
        v-if="exportGame && exportDetail"
        ref="exportStage"
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

.export-stage {
  position: fixed;
  left: 0;
  top: 0;
  z-index: 2147483000;
  width: 420px;
  background: #f6faf9;
}
</style>