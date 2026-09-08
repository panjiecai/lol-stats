<script setup lang="ts">
import { computed, inject, ref } from "vue"
import { Ban, LoaderCircle, Search, ShieldAlert, Sparkles } from "lucide-vue-next"
import { loadMatchDetail } from "../api"
import { notifyKey } from "../notifications"
import type {
  ChampionSummaryItem,
  GameAssetBundle,
  MatchDetailResponse,
  MatchDetailPlayer,
  OpenMatchPayload,
  PlayerStatsResponse,
  RecentGame,
} from "../types"
import { championName, formatDate } from "../utils"
import ChampionAvatar from "./ChampionAvatar.vue"

type FindMode = "anyFirstSame" | "specificAugment"

const props = defineProps<{
  stats: PlayerStatsResponse | null
  champions: Record<number, ChampionSummaryItem>
  gameAssets: GameAssetBundle
  sgpServerId?: string
  ownerLabel?: string
}>()

const emit = defineEmits<{
  openMatch: [payload: OpenMatchPayload]
}>()

const notify = inject(notifyKey, () => 0)

const CONCURRENCY = 10
const DEFAULT_LIMIT = 30

const augmentMap = computed(() => {
  const merged: Record<number, GameAssetBundle["augments"][number]> = {}
  for (const entry of props.gameAssets.augments) merged[entry.id] = entry
  // perks 作为回退（同 id 时以 augments 优先，缺失补 perks）
  for (const entry of props.gameAssets.perks) {
    if (!merged[entry.id]) merged[entry.id] = entry
  }
  return merged
})

const allAugmentOptions = computed(() =>
  Object.values(augmentMap.value)
    .filter((entry) => entry?.name?.trim())
    .sort((a, b) => a.name.localeCompare(b.name, "zh-CN")),
)

// 带海克斯的候选对局：以查询者自身在该局 augments 非空为准，按时间倒序。
const candidateGames = computed<RecentGame[]>(() =>
  (props.stats?.recentGames || [])
    .filter((game) => (game.augmentIds || []).length > 0)
    .slice()
    .sort((a, b) => b.gameCreation - a.gameCreation),
)

const ownerPuuid = computed(() => props.stats?.summoner.puuid || "")

const mode = ref<FindMode>("anyFirstSame")
const specificAugmentId = ref<number | null>(null)
const augmentSearch = ref("")

// —— 匹配范围 ——
// 'first' = 只比对每个人的「首个海克斯」（旧「首个连号」行为）；
// 'all'   = 把同方所有队员全部位置的海克斯并到一起，同一个海克斯被 ≥2 名不同队员共同拥有即命中（不限次序）。
type PosScope = "first" | "all"
const posScope = ref<PosScope>("all")

function setPosScope(next: PosScope) {
  if (scanning.value) return
  posScope.value = next
  results.value = []
  activeGroupId.value = null
  errorText.value = ""
}

const limit = ref(DEFAULT_LIMIT)
const limitOpen = ref(false)

const scanning = ref(false)
const cancelled = ref(false)
const progress = ref({ done: 0, total: 0, matched: 0 })
const statusMessage = ref("")
const errorText = ref("")

type MatchHit = {
  game: RecentGame
  firstAugmentId: number
  /** 命中所在队伍名（蓝方/红方…） */
  teamName: string
  /** 是否命中在查询者自己那一方 */
  isOwnerTeam: boolean
  /** 命中那一方最终是否获胜（取 team.win，而非 owner 账号的 game.win） */
  win: boolean
  teammates: Array<{ puuid: string; summonerName: string; championId: number }>
}

/** 按海克斯类型汇总的一条记录 */
type AugmentGroup = {
  firstAugmentId: number
  name: string
  rarity?: string
  games: number
  wins: number
  losses: number
  winRate: number
  hits: MatchHit[]
}

const results = ref<MatchHit[]>([])
/** 当前展开查看的海克斯汇总 id；null 表示停留在汇总视图 */
const activeGroupId = ref<number | null>(null)

let requestToken = 0

const filteredAugmentOptions = computed(() => {
  const keyword = augmentSearch.value.trim().toLowerCase().replace(/[\s·.'’_-]/g, "")
  const list = keyword
    ? allAugmentOptions.value.filter((entry) =>
        entry.name.toLowerCase().replace(/[\s·.'’_-]/g, "").includes(keyword),
      )
    : allAugmentOptions.value
  return list.slice(0, 60)
})

const limitOptions = computed(() => {
  const options = [10, 20, 30, 50, 100, 200]
  const max = candidateGames.value.length
  return Array.from(new Set(options.filter((value) => value <= max).concat([max])))
    .filter((value, index, self) => self.indexOf(value) === index)
    .sort((a, b) => a - b)
})

const canRun = computed(() => {
  if (!ownerPuuid.value) return false
  if (candidateGames.value.length === 0) return false
  if (mode.value === "specificAugment" && !specificAugmentId.value) return false
  return !scanning.value
})

const effectiveLimit = computed(() => Math.min(limit.value, candidateGames.value.length))

/** 汇总视图排序维度：按胜率 or 按对局数（均降序）。 */
type SortMode = "winRate" | "games"
const sortBy = ref<SortMode>("winRate")

/** 把命中按「海克斯类型」汇总，按当前 sortBy 降序（次关键字对局数/名称稳定）。 */
const augmentGroups = computed<AugmentGroup[]>(() => {
  const byId = new Map<number, MatchHit[]>()
  for (const hit of results.value) {
    const list = byId.get(hit.firstAugmentId) || []
    list.push(hit)
    byId.set(hit.firstAugmentId, list)
  }
  const groups: AugmentGroup[] = []
  for (const [firstAugmentId, hits] of byId.entries()) {
    const wins = hits.filter((hit) => hit.win).length
    const games = hits.length
    groups.push({
      firstAugmentId,
      name: augmentName(firstAugmentId),
      rarity: augmentMap.value[firstAugmentId]?.rarity,
      games,
      wins,
      losses: games - wins,
      winRate: games ? (wins / games) * 100 : 0,
      hits,
    })
  }
  groups.sort((a, b) => {
    const primary =
      sortBy.value === "winRate"
        ? b.winRate - a.winRate
        : b.games - a.games
    if (primary !== 0) return primary
    return b.games - a.games || a.name.localeCompare(b.name, "zh-CN")
  })
  return groups
})

/** 当前展开的海克斯下的全部对局（按时间倒序）。 */
const activeGroupHits = computed<MatchHit[]>(() => {
  const group = augmentGroups.value.find((item) => item.firstAugmentId === activeGroupId.value)
  return group
    ? group.hits
        .slice()
        .sort((a, b) => b.game.gameCreation - a.game.gameCreation)
    : []
})

/** 当前展开的海克斯汇总对象（activeGroupId 非空时才有）。 */
const activeGroup = computed<AugmentGroup | null>(() => {
  const id = activeGroupId.value
  return id === null ? null : augmentGroups.value.find((item) => item.firstAugmentId === id) || null
})

function openGroup(augmentId: number) {
  activeGroupId.value = activeGroupId.value === augmentId ? null : augmentId
}

function backToSummary() {
  activeGroupId.value = null
}

function setMode(next: FindMode) {
  if (scanning.value) return
  mode.value = next
  results.value = []
  activeGroupId.value = null
  errorText.value = ""
}

function selectAugment(entryId: number) {
  specificAugmentId.value = entryId
  augmentSearch.value = ""
}

function rarityText(rarity?: string) {
  switch (rarity) {
    case "kPrismatic":
      return "棱彩"
    case "kGold":
      return "黄金"
    case "kSilver":
      return "银色"
    case "kBronze":
      return "青铜"
    default:
      return ""
  }
}

function rarityClass(rarity?: string) {
  switch (rarity) {
    case "kPrismatic":
      return "augment-prismatic"
    case "kGold":
      return "augment-gold"
    case "kSilver":
      return "augment-silver"
    case "kBronze":
      return "augment-bronze"
    default:
      return "augment-plain"
  }
}

function augmentName(augmentId: number) {
  return augmentMap.value[augmentId]?.name || `强化 ${augmentId}`
}

// 在每一方（蓝方/红方各自独立）里，找出“≥2 名不同队员共同拥有某个海克斯”的海克斯及其队员。
// posScope=first：只看每人的首个海克斯（旧「首个连号」行为，等价“拥有者≥2且都在第1位”）。
// posScope=all：把每人全部位置的海克斯并到一起统计——同一海克斯被 ≥2 名不同队员共同拥有即命中，不限它出现在第几位。
// 注意：只在本方/对方各自内部比对，绝不跨队混算（“不是跨边”）。
function evaluateDetail(detail: MatchDetailResponse, game: RecentGame) {
  const teams = detail?.teams || []
  const hits: MatchHit[] = []

  for (const team of teams) {
    const players = team.players || []
    if (players.length < 2) continue

    const isOwnerTeam = players.some((player) => player.puuid === ownerPuuid.value)

    const pushHit = (augmentId: number, mates: MatchDetailPlayer[]) => {
      hits.push({
        game,
        firstAugmentId: augmentId,
        teamName: team.name || (isOwnerTeam ? "我方" : "对方"),
        isOwnerTeam,
        win: team.win,
        teammates: mates.map((player) => ({
          puuid: player.puuid,
          summonerName: player.summonerName || player.gameName || player.tagLine || "队友",
          championId: player.championId,
        })),
      })
    }

    // 统计该队内每个海克斯被哪些不同队员拥有（key=海克斯 id，value=队员列表，同一人只计一次）。
    const group = new Map<number, MatchDetailPlayer[]>()
    const seen = new Set<string>()
    for (const player of players) {
      const augIds = player.augmentIds || []
      if (posScope.value === "first" && augIds.length) {
        // 只看首个海克斯
        seen.add(`${augIds[0]}:${player.puuid}`)
        const first = augIds[0]
        const list = group.get(first) || []
        list.push(player)
        group.set(first, list)
        continue
      }
      for (const augId of augIds) {
        if (!augId) continue
        const seenKey = `${augId}:${player.puuid}`
        if (seen.has(seenKey)) continue
        seen.add(seenKey)
        const list = group.get(augId) || []
        list.push(player)
        group.set(augId, list)
      }
    }

    if (mode.value === "specificAugment") {
      const target = specificAugmentId.value
      if (target) {
        const list = group.get(target)
        if (list && list.length >= 2) pushHit(target, list)
      }
    } else {
      for (const [augmentId, list] of group.entries()) {
        if (list.length >= 2) pushHit(augmentId, list)
      }
    }
  }

  return hits.length ? hits : null
}

async function runScan() {
  if (!canRun.value) return

  const games = candidateGames.value.slice(0, effectiveLimit.value)
  if (!games.length) {
    notify({ kind: "warning", title: "没有可检索的海克斯对局", message: "先拉取战绩，或当前账号没有海克斯对局" })
    return
  }

  const token = ++requestToken
  scanning.value = true
  cancelled.value = false
  results.value = []
  activeGroupId.value = null
  errorText.value = ""
  progress.value = { done: 0, total: games.length, matched: 0 }
  statusMessage.value =
    posScope.value === "all"
      ? "正在逐局读取对局详情，找同方 ≥2 名队员共同拥有的海克斯（不限位次）…"
      : "正在逐局读取对局详情，找同方 ≥2 名队员首个海克斯相同的对局…"

  let index = 0
  const matched: MatchHit[] = []

  const worker = async () => {
    while (index < games.length) {
      const game = games[index]
      index += 1
      if (token !== requestToken || cancelled.value) return
      try {
        const detail = await loadMatchDetail(game.gameId, props.sgpServerId)
        const evaluated = detail && detail.teams ? evaluateDetail(detail, game) : null
        if (evaluated && evaluated.length) {
          matched.push(...evaluated)
        }
      } catch (error) {
        if (token === requestToken && !cancelled.value) {
          errorText.value = error instanceof Error ? error.message : String(error)
        }
      } finally {
        if (token === requestToken) {
          progress.value.done += 1
          progress.value.matched = matched.length
        }
      }
    }
  }

  const workers = Array.from({ length: Math.min(CONCURRENCY, games.length) }, () => worker())
  await Promise.all(workers)

  if (token !== requestToken) return
  scanning.value = false

  const uniqueByGame = new Map<string, MatchHit>()
  for (const hit of matched)
    uniqueByGame.set(
      `${hit.game.gameId}:${hit.teamName}:${hit.firstAugmentId}`,
      hit,
    )
  results.value = Array.from(uniqueByGame.values())

  if (results.value.length) {
    statusMessage.value = ""
    notify({
      kind: "success",
      title: "海克斯连号检索完成",
      message: `共命中 ${results.value.length} 处，判定：${summaryPrefix()}`,
    })
  } else {
    statusMessage.value = errorText.value
      ? `已完成，但过程中有 ${progress.value.done}/${progress.value.total} 局读取失败，可重试`
      : "检索完成，没有命中"
    if (!errorText.value) {
      notify({
        kind: "info",
        title: "没有命中",
        message: `在最近 ${effectiveLimit.value} 局海克斯对局中，未出现满足「${summaryPrefix()}」的命中`,
      })
    }
  }
}

function cancelScan() {
  cancelled.value = true
  requestToken += 1
  scanning.value = false
  statusMessage.value = "已取消"
}

function resetAll() {
  results.value = []
  activeGroupId.value = null
  errorText.value = ""
  statusMessage.value = ""
  progress.value = { done: 0, total: 0, matched: 0 }
}

function queueLabel(game: RecentGame) {
  const map: Record<number, string> = {
    2400: "海克斯大乱斗",
    2410: "海斗大赛",
    2450: "海克斯大乱斗",
  }
  return map[game.queueId] || game.gameMode || `队列 ${game.queueId}`
}

function openHit(hit: MatchHit) {
  emit("openMatch", {
    game: hit.game,
    ownerLabel: props.ownerLabel || props.stats?.summoner.gameName || "玩家",
    ownerPuuid: ownerPuuid.value,
    sgpServerId: props.sgpServerId,
  })
}

/** 匹配判据的一句自然描述，用于检索说明。 */
function summaryPrefix() {
  if (mode.value === "specificAugment") {
    const name = augmentName(specificAugmentId.value || 0)
    return posScope.value === "all"
      ? `${name} 被同方 ≥2 名队员共同拥有（不限位次）`
      : `${name} 被同方 ≥2 名队员作为首个海克斯共同拥有`
  }
  return posScope.value === "all"
    ? "同一海克斯被同方 ≥2 名队员共同拥有（不限位次）"
    : "同一海克斯被同方 ≥2 名队员作为首个海克斯共同拥有"
}

function dateText(timestamp: number) {
  return formatDate(timestamp)
}

function winClass(win: boolean) {
  return win ? "win" : "loss"
}
</script>

<template>
  <div class="augment-combo">
    <div class="combo-head">
      <div class="combo-title">
        <Sparkles :size="16" />
        <strong>海克斯连号检索</strong>
        <span>逐局读取详情，分别检查我方、对方（同方内）≥2 名队员共同拥有同一海克斯的对局</span>
      </div>
    </div>

    <div class="combo-controls">
      <div class="mode-switch" role="tablist" aria-label="检索模式">
        <button
          type="button"
          :class="{ active: mode === 'anyFirstSame' }"
          :disabled="scanning"
          @click="setMode('anyFirstSame')"
        >
          任意首海克斯相同
        </button>
        <button
          type="button"
          :class="{ active: mode === 'specificAugment' }"
          :disabled="scanning"
          @click="setMode('specificAugment')"
        >
          指定海克斯
        </button>
      </div>

      <div class="pos-scope-row">
        <label>匹配范围</label>
        <div class="pos-scope-switch" role="group" aria-label="匹配范围">
          <button
            type="button"
            :class="{ active: posScope === 'all' }"
            :disabled="scanning"
            @click="setPosScope('all')"
          >
            全部海克斯·不限次序
          </button>
          <button
            type="button"
            :class="{ active: posScope === 'first' }"
            :disabled="scanning"
            @click="setPosScope('first')"
          >
            仅首个海克斯
          </button>
        </div>
        <span class="pos-scope-hint">
          {{
            posScope === "all"
              ? "同一边内，任一海克斯被 ≥2 名队员共同拥有即命中（不管在第几位）"
              : "同一边内，≥2 名队员首个海克斯相同即命中"
          }}
        </span>
      </div>

      <div class="augment-picker" v-if="mode === 'specificAugment'">
        <div class="augment-search-wrap">
          <Search :size="14" />
          <input
            v-model="augmentSearch"
            placeholder="搜索海克斯名称…"
            :disabled="scanning"
          />
        </div>
        <div class="augment-option-list" v-if="augmentSearch.trim()">
          <button
            v-for="entry in filteredAugmentOptions"
            :key="entry.id"
            type="button"
            @click="selectAugment(entry.id)"
          >
            <span class="rarity-dot" :class="rarityClass(entry.rarity)"></span>
            <span class="opt-name">{{ entry.name }}</span>
            <span class="opt-rarity" v-if="rarityText(entry.rarity)">{{ rarityText(entry.rarity) }}</span>
          </button>
          <div class="augment-no-match" v-if="!filteredAugmentOptions.length">没有匹配的海克斯</div>
        </div>
      </div>

      <div class="selected-target" v-if="mode === 'specificAugment' && specificAugmentId">
        <span>已选：</span>
        <strong :class="rarityClass(augmentMap[specificAugmentId]?.rarity)">
          {{ augmentName(specificAugmentId) }}
        </strong>
      </div>

      <div class="scope-row">
        <label>扫描最近</label>
        <div class="limit-select">
          <button type="button" :disabled="scanning" @click="limitOpen = !limitOpen">
            {{ effectiveLimit }} 局海克斯对局
          </button>
          <div class="limit-menu" v-if="limitOpen">
            <button
              v-for="option in limitOptions"
              :key="option"
              type="button"
              :class="{ active: effectiveLimit === option }"
              @click="limit = option; limitOpen = false"
            >
              最近 {{ option }} 局
            </button>
          </div>
        </div>
        <span class="candidate-total">（候选共 {{ candidateGames.length }} 局，拉取深度 {{ stats?.depthLoaded || 0 }}）</span>
      </div>

      <div class="action-row">
        <button
          class="run-button"
          type="button"
          :disabled="!canRun"
          @click="runScan"
        >
          <LoaderCircle v-if="scanning" class="spin" :size="15" />
          <Search v-else :size="15" />
          {{ scanning ? "检索中…" : "开始检索" }}
        </button>
        <button
          v-if="scanning"
          class="cancel-button"
          type="button"
          @click="cancelScan"
        >
          <Ban :size="15" />
          取消
        </button>
        <button
          v-if="results.length"
          class="clear-button"
          type="button"
          @click="resetAll"
        >
          清空结果
        </button>
      </div>

      <div class="progress-line" v-if="scanning || statusMessage">
        <template v-if="scanning">
          <LoaderCircle class="spin" :size="14" />
          <span>
            已读取 {{ progress.done }}/{{ progress.total }} 局，命中 {{ progress.matched }} 处
            （并发 {{ CONCURRENCY }}，逐局详情较慢，请耐心等待）
          </span>
        </template>
        <template v-else-if="statusMessage">
          <span class="status-done">{{ statusMessage }}</span>
        </template>
      </div>

      <div class="error-text" v-if="errorText && !scanning">
        <ShieldAlert :size="14" />
        <span>部分对局读取失败：{{ errorText }}</span>
      </div>
    </div>

    <div class="result-body" v-if="results.length">
      <!-- 展开视图：某海克斯下的全部对局 -->
      <template v-if="activeGroup">
        <div class="result-note">
          <button type="button" class="back-button" @click="backToSummary">‹ 返回海克斯汇总</button>
          <span class="rarity-dot" :class="rarityClass(activeGroup.rarity)"></span>
          <strong>{{ activeGroup.name }}</strong>
          命中 {{ activeGroup.games }} 处，胜率 {{ activeGroup.winRate.toFixed(0) }}%（{{ activeGroup.wins }} 胜 {{ activeGroup.losses }} 负）——胜率指「该海克斯所在队伍」的胜率。点击对局可打开完整详情。
        </div>
        <div class="result-list">
          <button
            v-for="hit in activeGroupHits"
            :key="`${hit.game.gameId}:${hit.teamName}:${hit.firstAugmentId}`"
            class="result-row"
            type="button"
            :title="`打开对局 ${hit.game.gameId}`"
            @click="openHit(hit)"
          >
            <ChampionAvatar
              class="result-avatar"
              :champion-id="hit.game.championId"
              :champions="champions"
            />
            <div class="result-main">
              <div class="result-title">
                <span class="win-tag" :class="winClass(hit.win)">
                  {{ hit.win ? "胜" : "负" }}
                </span>
                <span class="side-tag" :class="hit.isOwnerTeam ? 'side-owner' : 'side-enemy'">
                  {{ hit.isOwnerTeam ? "我方" : "对方" }} · {{ hit.teamName }}
                </span>
                <strong>{{ queueLabel(hit.game) }}</strong>
                <span>{{ dateText(hit.game.gameCreation) }}</span>
              </div>
              <div class="result-augment">
                <span class="rarity-dot" :class="rarityClass(augmentMap[hit.firstAugmentId]?.rarity)"></span>
                <strong>{{ augmentName(hit.firstAugmentId) }}</strong>
                <span class="pos-chip" v-if="posScope === 'first'">首个海克斯</span>
                <span class="pos-chip" v-else>不限位次</span>
                <span>{{ hit.isOwnerTeam ? "我方" : "对方" }} {{ hit.teammates.length }} 名队员持有：</span>
              </div>
              <div class="result-players">
                <span
                  v-for="mate in hit.teammates"
                  :key="mate.puuid"
                  class="mate-chip"
                  :title="mate.summonerName"
                >
                  {{ championName(champions, mate.championId) }} · {{ mate.summonerName }}
                </span>
              </div>
            </div>
            <span class="open-hint">查看对局 ›</span>
          </button>
        </div>
      </template>

      <!-- 汇总视图：按海克斯类型统计命中方的胜负胜率 -->
      <template v-else>
        <div class="result-note">
          命中 <strong>{{ results.length }}</strong> 处（<strong>{{ augmentGroups.length }}</strong> 种海克斯）——我方与对方分开检查，仅同方内比对（不跨边）。判定：{{ summaryPrefix() }}。胜率统计的是「拥有该海克斯的那支队伍」的胜负（非账号那一方），点击海克斯查看其下全部对局。
        </div>
        <div class="summary-sort-row">
          <span>排序</span>
          <div class="summary-sort-switch" role="group" aria-label="汇总排序方式">
            <button
              type="button"
              :class="{ active: sortBy === 'winRate' }"
              @click="sortBy = 'winRate'"
            >
              胜率
            </button>
            <button
              type="button"
              :class="{ active: sortBy === 'games' }"
              @click="sortBy = 'games'"
            >
              对局数
            </button>
          </div>
          <span class="sort-hint">{{ sortBy === "winRate" ? "按胜率从高到低" : "按对局数从多到少" }}</span>
        </div>
        <div class="augment-summary-list">
          <button
            v-for="group in augmentGroups"
            :key="group.firstAugmentId"
            class="augment-summary-row"
            type="button"
            @click="openGroup(group.firstAugmentId)"
          >
            <span class="rarity-dot" :class="rarityClass(group.rarity)"></span>
            <span class="aug-name">{{ group.name }}</span>
            <span class="aug-count">{{ group.games }} 处命中 · {{ group.wins }} 胜 {{ group.losses }} 负</span>
            <span class="aug-rate" :class="group.winRate >= 50 ? 'rate-win' : 'rate-loss'">
              {{ group.winRate.toFixed(0) }}%
            </span>
            <span class="rate-bar">
              <span class="rate-fill" :class="group.winRate >= 50 ? 'rate-fill-win' : 'rate-fill-loss'" :style="{ width: group.winRate + '%' }"></span>
            </span>
            <span class="aug-view">展开对局 ›</span>
          </button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.augment-combo {
  display: flex;
  flex-direction: column;
  gap: 14px;
  border: 1px solid #dce7e4;
  border-radius: 12px;
  background: #ffffff;
  box-shadow: 0 12px 28px rgba(32, 67, 73, 0.07);
  padding: 14px 16px;
}

.combo-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.combo-title {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: #20333a;
}

.combo-title strong {
  font-size: 15px;
  font-weight: 900;
}

.combo-title span {
  color: #718087;
  font-size: 12px;
}

.combo-controls {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.mode-switch {
  display: inline-flex;
  gap: 6px;
}

.mode-switch button {
  border: 1px solid #dce7e4;
  border-radius: 999px;
  color: #53666c;
  background: #f5f9f8;
  cursor: pointer;
  font-size: 13px;
  font-weight: 800;
  padding: 6px 12px;
}

.mode-switch button.active {
  color: #ffffff;
  border-color: #1f5f56;
  background: #1f5f56;
}

.mode-switch button:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.pos-scope-row {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: #53666c;
}

.pos-scope-row label {
  font-weight: 700;
}

.pos-scope-switch {
  display: inline-flex;
  gap: 4px;
  border: 1px solid #dce7e4;
  border-radius: 999px;
  background: #f5f9f8;
  padding: 2px;
}

.pos-scope-switch button {
  border: 0;
  border-radius: 999px;
  color: #53666c;
  background: transparent;
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
  padding: 4px 12px;
}

.pos-scope-switch button.active {
  color: #ffffff;
  background: #1f5f56;
}

.pos-scope-switch button:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.pos-scope-hint {
  color: #8a9a9f;
  font-size: 12px;
}

.augment-picker {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: 360px;
}

.augment-search-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  border: 1px solid #dce7e4;
  border-radius: 8px;
  background: #f5f9f8;
  padding: 6px 9px;
  color: #718087;
}

.augment-search-wrap input {
  flex: 1;
  border: 0;
  outline: 0;
  background: transparent;
  color: #20333a;
}

.augment-option-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid #dce7e4;
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 0 8px 20px rgba(32, 67, 73, 0.1);
  padding: 4px;
}

.augment-option-list button {
  display: flex;
  align-items: center;
  gap: 7px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  cursor: pointer;
  color: #263238;
  text-align: left;
  padding: 6px 8px;
}

.augment-option-list button:hover {
  background: #eef6f3;
}

.opt-name {
  flex: 1;
  font-size: 13px;
  font-weight: 700;
}

.opt-rarity {
  color: #718087;
  font-size: 11px;
}

.augment-no-match {
  color: #718087;
  font-size: 12px;
  padding: 8px;
}

.selected-target {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
}

.selected-target strong {
  font-size: 13px;
  font-weight: 900;
}

.rarity-dot {
  flex: 0 0 auto;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.rarity-dot.augment-prismatic {
  background: linear-gradient(135deg, #e056a0, #7c5cff, #34b8d6);
}

.rarity-dot.augment-gold {
  background: #e6b23a;
}

.rarity-dot.augment-silver {
  background: #c2d2da;
}

.rarity-dot.augment-bronze {
  background: #b08a5a;
}

.rarity-dot.augment-plain {
  background: #8b9aa2;
}

.selected-target strong.augment-prismatic {
  color: #c0399b;
}

.selected-target strong.augment-gold {
  color: #b8860b;
}

.selected-target strong.augment-silver {
  color: #5f7a86;
}

.selected-target strong.augment-bronze {
  color: #92652f;
}

.scope-row {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: #53666c;
}

.limit-select {
  position: relative;
}

.limit-select > button {
  border: 1px solid #dce7e4;
  border-radius: 8px;
  color: #20333a;
  background: #f5f9f8;
  cursor: pointer;
  font-weight: 800;
  padding: 5px 10px;
}

.limit-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  z-index: 20;
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 140px;
  border: 1px solid #dce7e4;
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 0 8px 20px rgba(32, 67, 73, 0.1);
  padding: 4px;
}

.limit-menu button {
  border: 0;
  border-radius: 6px;
  background: transparent;
  cursor: pointer;
  color: #263238;
  text-align: left;
  font-size: 13px;
  padding: 5px 8px;
}

.limit-menu button:hover {
  background: #eef6f3;
}

.limit-menu button.active {
  color: #1f5f56;
  background: #e2f0ec;
  font-weight: 800;
}

.candidate-total {
  color: #8a9a9f;
  font-size: 12px;
}

.action-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.run-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 0;
  border-radius: 8px;
  color: #ffffff;
  background: #1f5f56;
  cursor: pointer;
  font-size: 13px;
  font-weight: 800;
  padding: 8px 14px;
}

.run-button:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.cancel-button,
.clear-button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid #dce7e4;
  border-radius: 8px;
  color: #53666c;
  background: #ffffff;
  cursor: pointer;
  font-size: 13px;
  font-weight: 700;
  padding: 7px 12px;
}

.progress-line {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: #1f5f56;
  font-size: 13px;
  font-weight: 700;
}

.status-done {
  color: #53666c;
}

.error-text {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: #b3372f;
  font-size: 12px;
}

.result-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.result-note {
  color: #53666c;
  font-size: 13px;
}

.result-note strong {
  color: #1f5f56;
  font-size: 15px;
}

.back-button {
  display: inline-flex;
  align-items: center;
  border: 1px solid #dce7e4;
  border-radius: 999px;
  color: #1f5f56;
  background: #ffffff;
  cursor: pointer;
  font-size: 12px;
  font-weight: 800;
  padding: 2px 10px;
  margin-right: 4px;
}

.back-button:hover {
  background: #eef6f3;
}

.summary-sort-row {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: #718087;
}

.summary-sort-switch {
  display: inline-flex;
  gap: 4px;
  border: 1px solid #dce7e4;
  border-radius: 8px;
  background: #f5f9f8;
  padding: 2px;
}

.summary-sort-switch button {
  border: 0;
  border-radius: 6px;
  color: #53666c;
  background: transparent;
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
  padding: 3px 12px;
}

.summary-sort-switch button.active {
  color: #1f5f56;
  background: #ffffff;
  box-shadow: 0 1px 3px rgba(32, 67, 73, 0.12);
}

.sort-hint {
  color: #8a9a9f;
}

.augment-summary-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 460px;
  overflow-y: auto;
}

.augment-summary-row {
  display: flex;
  align-items: center;
  gap: 10px;
  border: 1px solid #e2ebe8;
  border-radius: 10px;
  background: #ffffff;
  cursor: pointer;
  text-align: left;
  padding: 9px 12px;
}

.augment-summary-row:hover {
  border-color: #bcd6cf;
  background: #f5faf8;
}

.aug-name {
  flex: 1;
  min-width: 0;
  color: #20333a;
  font-size: 13px;
  font-weight: 900;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.aug-count {
  flex: 0 0 auto;
  color: #8a9a9f;
  font-size: 12px;
  white-space: nowrap;
}

.aug-rate {
  flex: 0 0 auto;
  width: 42px;
  text-align: right;
  font-size: 14px;
  font-weight: 900;
  white-space: nowrap;
}

.rate-win {
  color: #d34f4f;
}

.rate-loss {
  color: #4d7ac9;
}

.rate-bar {
  flex: 0 0 110px;
  height: 8px;
  border-radius: 999px;
  background: #eef2f5;
  overflow: hidden;
}

.rate-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
}

.rate-fill-win {
  background: #d34f4f;
}

.rate-fill-loss {
  background: #4d7ac9;
}

.aug-view {
  flex: 0 0 auto;
  color: #1f5f56;
  font-size: 12px;
  font-weight: 800;
  white-space: nowrap;
}

.result-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 460px;
  overflow-y: auto;
}

.result-row {
  display: flex;
  align-items: center;
  gap: 11px;
  border: 1px solid #e2ebe8;
  border-radius: 10px;
  background: #ffffff;
  cursor: pointer;
  text-align: left;
  padding: 9px 12px;
}

.result-row:hover {
  border-color: #bcd6cf;
  background: #f5faf8;
}

.result-avatar {
  flex: 0 0 auto;
}

.result-main {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}

.result-title {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 7px;
  color: #20333a;
  font-size: 13px;
}

.result-title strong {
  font-size: 13px;
  font-weight: 900;
}

.result-title > span:not(.win-tag) {
  color: #8a9a9f;
  font-size: 12px;
}

.win-tag {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: 5px;
  color: #ffffff;
  font-size: 11px;
  font-weight: 900;
}

.win-tag.win {
  background: #d34f4f;
}

.win-tag.loss {
  background: #4d7ac9;
}

.side-tag {
  display: inline-flex;
  align-items: center;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 800;
  padding: 1px 8px;
  white-space: nowrap;
}

.side-tag.side-owner {
  border: 1px solid #bcd6cf;
  color: #1f5f56;
  background: #eaf4f0;
}

.side-tag.side-enemy {
  border: 1px solid #c9d4ea;
  color: #3a5a8c;
  background: #eef2fa;
}

.result-augment {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: #53666c;
}

.result-augment strong {
  color: #1f5f56;
  font-size: 13px;
  font-weight: 900;
}

.pos-chip {
  flex: 0 0 auto;
  border: 1px solid #dce7e4;
  border-radius: 999px;
  color: #53666c;
  background: #f2f7f5;
  font-size: 11px;
  font-weight: 700;
  padding: 1px 8px;
  white-space: nowrap;
}

.result-players {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.mate-chip {
  border: 1px solid #dce7e4;
  border-radius: 999px;
  color: #20333a;
  background: #f2f7f5;
  font-size: 12px;
  font-weight: 700;
  padding: 2px 9px;
}

.open-hint {
  flex: 0 0 auto;
  color: #1f5f56;
  font-size: 12px;
  font-weight: 800;
}
</style>
