<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from "vue"
import { LoaderCircle, RefreshCw, Sword, UserRound } from "lucide-vue-next"
import { queryPlayerActiveGame } from "../api"
import type {
  ActiveGamePlayer,
  ChampionSummaryItem,
  GameAssetBundle,
  GameAssetEntry,
  PlayerActiveGame,
} from "../types"
import { championName } from "../utils"
import AssetIcon from "./AssetIcon.vue"
import ChampionAvatar from "./ChampionAvatar.vue"

const props = defineProps<{
  puuid: string
  sgpServerId?: string
  champions: Record<number, ChampionSummaryItem>
  gameAssets: GameAssetBundle
}>()

const AUTO_REFRESH_MS = 15000

const activeGame = ref<PlayerActiveGame | null>(null)
const loading = ref(false)
const error = ref("")
let autoRefreshTimer = 0

const spellMap = computed(() =>
  props.gameAssets.summonerSpells.reduce<Record<number, GameAssetEntry>>((acc, entry) => {
    acc[entry.id] = entry
    return acc
  }, {}),
)

function positionName(position: string) {
  const raw = (position || "").toUpperCase()
  const map: Record<string, string> = {
    TOP: "上路",
    JUNGLE: "打野",
    MIDDLE: "中路",
    MID: "中路",
    BOTTOM: "下路",
    ADC: "下路",
    UTILITY: "辅助",
    SUPPORT: "辅助",
    NONE: "-",
    UNSELECTED: "-",
  }
  return map[raw] || raw || "-"
}

function gameStateLabel(state: string) {
  const map: Record<string, string> = {
    running: "进行中",
    in_progress: "进行中",
    champ_select: "选人阶段",
    pre_ready_check: "准备检查",
    stalled: "已暂停",
  }
  return state ? map[state.toLowerCase()] || state : "-"
}

function gameModeLabel(mode: string) {
  const map: Record<string, string> = {
    CLASSIC: "召唤师峡谷",
    ARAM: "极地大乱斗",
    STRAWBERRY: "海克斯大乱斗",
  }
  return mode ? map[mode.toUpperCase()] || mode : "-"
}

function spellAsset(player: ActiveGamePlayer, slot: 1 | 2) {
  const id = slot === 1 ? player.spell1Id : player.spell2Id
  return id ? spellMap.value[id] : undefined
}

async function loadActiveGame() {
  const puuid = props.puuid
  if (!puuid || loading.value) return
  loading.value = true
  error.value = ""
  try {
    activeGame.value = await queryPlayerActiveGame(puuid, props.sgpServerId)
  } catch (loadError) {
    error.value = loadError instanceof Error ? loadError.message : String(loadError)
    activeGame.value = null
  } finally {
    loading.value = false
  }
}

function startAutoRefresh() {
  if (autoRefreshTimer || !props.puuid) return
  autoRefreshTimer = window.setInterval(() => {
    void loadActiveGame()
  }, AUTO_REFRESH_MS)
}

function stopAutoRefresh() {
  if (!autoRefreshTimer) return
  window.clearInterval(autoRefreshTimer)
  autoRefreshTimer = 0
}

watch(
  () => [props.puuid, props.sgpServerId] as const,
  () => {
    activeGame.value = null
    stopAutoRefresh()
    void loadActiveGame()
    startAutoRefresh()
  },
  { immediate: true },
)

onMounted(startAutoRefresh)
onActivated(startAutoRefresh)
onDeactivated(stopAutoRefresh)
onBeforeUnmount(stopAutoRefresh)

const inGame = computed(() => Boolean(activeGame.value?.inGame))
const teams = computed(() => activeGame.value?.teams || [])
</script>

<template>
  <section class="active-game-panel">
    <header class="agent-header">
      <div class="agent-title">
        <Sword :size="16" />
        <div>
          <span class="label">{{ inGame ? "实时对局" : "实时状态" }}</span>
          <h3>{{ inGame ? gameStateLabel(activeGame?.gameState || "") : "等待匹配" }}</h3>
        </div>
      </div>
      <button class="agent-refresh" :disabled="loading || !puuid" @click="loadActiveGame">
        <LoaderCircle v-if="loading" class="spin" :size="14" />
        <RefreshCw v-else :size="14" />
        {{ loading ? "查询中" : "刷新" }}
      </button>
    </header>

    <div class="agent-empty" v-if="!puuid">加载账号信息后自动查询实时对局</div>
    <div class="agent-empty loading" v-else-if="loading && !activeGame">正在查询该账号是否在对局中</div>
    <div class="agent-empty error" v-else-if="!activeGame && error">{{ error }}</div>
    <div class="agent-empty offline" v-else-if="activeGame && !inGame">
      <UserRound :size="18" />
      <span>当前不在对局中</span>
    </div>

    <template v-else-if="activeGame && inGame">
      <div class="agent-meta">
        <span v-if="activeGame.queueTypeName">{{ activeGame.queueTypeName }}</span>
        <span v-if="activeGame.gameMode">{{ gameModeLabel(activeGame.gameMode) }}</span>
        <span v-if="activeGame.gameState">{{ gameStateLabel(activeGame.gameState) }}</span>
        <span v-if="activeGame.gameId">对局 {{ activeGame.gameId }}</span>
      </div>

      <div class="agent-teams">
        <section class="agent-team" v-for="team in teams" :key="team.name">
          <div class="agent-team-title">
            <strong>{{ team.name }}</strong>
            <span>{{ team.players.length }} 人</span>
          </div>
          <div class="agent-player-grid">
            <article
              class="agent-player"
              v-for="player in team.players"
              :key="player.puuid"
            >
              <ChampionAvatar :champion-id="player.championId" :champions="champions" :size="34" />
              <div class="agent-player-main">
                <strong :title="player.summonerName">{{ player.summonerName }}</strong>
                <span>
                  {{ championName(champions, player.championId) }} ·
                  {{ positionName(player.position) }}
                </span>
              </div>
              <div class="agent-spells">
                <AssetIcon
                  :path="spellAsset(player, 1)?.iconPath"
                  :label="spellAsset(player, 1)?.name"
                  :fallback="spellAsset(player, 1)?.name"
                  :size="22"
                />
                <AssetIcon
                  :path="spellAsset(player, 2)?.iconPath"
                  :label="spellAsset(player, 2)?.name"
                  :fallback="spellAsset(player, 2)?.name"
                  :size="22"
                />
              </div>
            </article>
          </div>
        </section>
      </div>
    </template>
  </section>
</template>

<style scoped>
.active-game-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  border: 1px solid #d7e5e2;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.78);
  box-shadow: 0 10px 24px rgba(32, 67, 73, 0.06);
  padding: 12px 14px;
}

.agent-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.agent-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
  color: #1f5f56;
}

.agent-title h3 {
  margin: 1px 0 0;
  color: #1f2a2e;
  font-size: 18px;
  font-weight: 950;
  line-height: 1.1;
}

.label {
  color: #6d7d83;
  font-size: 12px;
  font-weight: 800;
}

.agent-refresh {
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

.agent-refresh:hover:not(:disabled) {
  color: #ffffff;
  background: #1f5f56;
}

.agent-refresh:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.spin {
  animation: agent-spin 0.9s linear infinite;
}

@keyframes agent-spin {
  to {
    transform: rotate(360deg);
  }
}

.agent-empty {
  display: flex;
  min-height: 64px;
  align-items: center;
  justify-content: center;
  gap: 7px;
  border: 1px dashed #cbdcd8;
  border-radius: 8px;
  color: #657179;
  background: rgba(255, 255, 255, 0.55);
  font-size: 13px;
  font-weight: 700;
}

.agent-empty.error {
  color: #a94745;
  border-color: #efc4c2;
}

.agent-empty.offline {
  color: #52636a;
  border-color: #cbdcd8;
}

.agent-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.agent-meta span {
  border: 1px solid #d6e5e1;
  border-radius: 999px;
  padding: 5px 10px;
  color: #53656b;
  background: rgba(255, 255, 255, 0.72);
  font-size: 12px;
  font-weight: 800;
}

.agent-teams {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
  gap: 12px;
}

.agent-team {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 8px;
}

.agent-team-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.agent-team-title strong {
  color: #263238;
  font-size: 15px;
  font-weight: 900;
}

.agent-team-title span {
  color: #6f7f84;
  font-size: 12px;
}

.agent-player-grid {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.agent-player {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
  border: 1px solid #e2ebe8;
  border-radius: 8px;
  background: #ffffff;
  padding: 6px 8px;
}

.agent-player-main {
  min-width: 0;
  flex: 1 1 auto;
}

.agent-player-main strong,
.agent-player-main span {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.agent-player-main strong {
  color: #233238;
  font-size: 14px;
  font-weight: 900;
  line-height: 1.2;
}

.agent-player-main span {
  margin-top: 2px;
  color: #738188;
  font-size: 12px;
}

.agent-spells {
  display: inline-flex;
  flex: 0 0 auto;
  gap: 4px;
}
</style>