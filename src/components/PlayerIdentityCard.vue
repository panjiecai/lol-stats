<script setup lang="ts">
import { computed } from "vue"
import { rankIconLarge } from "../rankIcons"
import { searchServerLabel } from "../servers"
import type { RankedQueueEntry, RankedStatsResponse, SummonerInfo } from "../types"
import { riotId } from "../utils"
import AssetIcon from "./AssetIcon.vue"

const props = defineProps<{
  summoner?: SummonerInfo | null
  sgpServerId?: string
  rankedStats?: RankedStatsResponse | null
  rankedLoading?: boolean
  isSelf?: boolean
}>()

const tierLabels: Record<string, string> = {
  IRON: "坚韧黑铁",
  BRONZE: "英勇黄铜",
  SILVER: "不屈白银",
  GOLD: "荣耀黄金",
  PLATINUM: "华贵铂金",
  EMERALD: "流光翡翠",
  DIAMOND: "璀璨钻石",
  MASTER: "超凡大师",
  GRANDMASTER: "傲世宗师",
  CHALLENGER: "最强王者",
}
const apexTiers = new Set(["MASTER", "GRANDMASTER", "CHALLENGER"])

const displayName = computed(() => riotId(props.summoner || undefined))
const hasSummoner = computed(() => Boolean(props.summoner?.puuid || props.summoner?.displayName))
const serverLabel = computed(() => searchServerLabel(props.sgpServerId))
const levelText = computed(() =>
  props.summoner && props.summoner.summonerLevel > 0 ? `Lv.${props.summoner.summonerLevel}` : "",
)
const shortPuuid = computed(() => {
  const puuid = props.summoner?.puuid || ""
  return puuid ? `${puuid.slice(0, 8)}…` : ""
})

const avatarPath = computed(() => {
  const iconId = props.summoner?.profileIconId || 0
  return iconId > 0 ? `/lol-game-data/assets/v1/profile-icons/${iconId}.jpg` : undefined
})

const rankCards = computed(() => {
  const ranked = props.rankedStats
  if (!ranked) return []

  const flex = ranked.queueMap.RANKED_FLEX_SR
  const solo = ranked.queueMap.RANKED_SOLO_5x5
  return [
    { label: "当前灵活组排", value: formatCurrentRank(flex), tier: flex.tier },
    { label: "当前单双排", value: formatCurrentRank(solo), tier: solo.tier },
    { label: "历史灵活组排最高", value: formatHighestRank(flex), tier: bestTier(flex) },
    { label: "历史单双排最高", value: formatHighestRank(solo), tier: bestTier(solo) },
  ]
})

function formatCurrentRank(entry: RankedQueueEntry) {
  return formatRank(entry.tier, entry.division, entry.leaguePoints)
}

function formatHighestRank(entry: RankedQueueEntry) {
  return formatRank(bestTier(entry), bestDivision(entry))
}

function bestTier(entry: RankedQueueEntry) {
  return entry.highestTier || entry.previousSeasonHighestTier || entry.previousSeasonEndTier || ""
}

function bestDivision(entry: RankedQueueEntry) {
  return (
    entry.highestDivision ||
    entry.previousSeasonHighestDivision ||
    entry.previousSeasonEndDivision ||
    ""
  )
}

function formatRank(tier: string, division = "", leaguePoints?: number) {
  const normalizedTier = tier.trim().toUpperCase()
  if (!normalizedTier || normalizedTier === "NONE" || normalizedTier === "NA") return "未定级"

  const tierLabel = tierLabels[normalizedTier] || normalizedTier
  const divisionText = apexTiers.has(normalizedTier) ? "" : normalizeDivision(division)
  const lpText = typeof leaguePoints === "number" && leaguePoints > 0 ? ` ${leaguePoints}LP` : ""
  return `${tierLabel}${divisionText ? ` ${divisionText}` : ""}${lpText}`
}

function normalizeDivision(division: string) {
  const normalized = division.trim().toUpperCase()
  if (!normalized || normalized === "NA" || normalized === "NONE") return ""
  return normalized
}

function rankTierClass(tier: string) {
  const normalized = tier.trim().toLowerCase()
  return normalized ? `tier-${normalized}` : "tier-unranked"
}
</script>

<template>
  <section class="identity-card">
    <div class="identity-main">
      <AssetIcon
        class="identity-avatar"
        :path="avatarPath"
        :label="displayName"
        :fallback="displayName.slice(0, 1)"
        :size="52"
      />

      <div class="identity-text">
        <div class="identity-name-row">
          <strong :title="displayName">{{ hasSummoner ? displayName : "加载中" }}</strong>
          <span class="identity-badge" :class="{ 'is-self': isSelf }">
            {{ isSelf ? "当前角色" : "查询目标" }}
          </span>
        </div>

        <div class="identity-meta">
          <span>{{ serverLabel }}</span>
          <span v-if="levelText">{{ levelText }}</span>
          <span v-if="shortPuuid" class="identity-puuid" :title="summoner?.puuid || ''">
            {{ shortPuuid }}
          </span>
        </div>
      </div>
    </div>

    <div class="identity-rank" v-if="rankedLoading || rankCards.length">
      <div class="rank-card loading" v-if="rankedLoading && !rankCards.length">
        <span>段位</span>
        <strong>读取中</strong>
      </div>
      <article
        v-for="card in rankCards"
        :key="card.label"
        :class="['rank-card', rankTierClass(card.tier)]"
      >
        <img class="rank-card-icon" :src="rankIconLarge(card.tier)" :alt="card.value" />
        <div class="rank-card-text">
          <span>{{ card.label }}</span>
          <strong>{{ card.value }}</strong>
        </div>
      </article>
    </div>
  </section>
</template>

<style scoped>
.identity-card {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border: 1px solid #d7e5e2;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.78);
  box-shadow: 0 10px 24px rgba(32, 67, 73, 0.06);
  padding: 12px 14px;
}

.identity-main {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 11px;
}

.identity-avatar {
  border-radius: 50%;
}

.identity-text {
  min-width: 0;
}

.identity-name-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.identity-name-row strong {
  overflow: hidden;
  color: #1f2a2e;
  font-size: 19px;
  font-weight: 950;
  line-height: 1.15;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.identity-badge {
  flex: 0 0 auto;
  border: 1px solid #d6e5e1;
  border-radius: 999px;
  color: #6d7d83;
  background: rgba(255, 255, 255, 0.8);
  font-size: 11px;
  font-weight: 800;
  padding: 2px 8px;
}

.identity-badge.is-self {
  color: #ffffff;
  border-color: #1f5f56;
  background: #1f5f56;
}

.identity-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-top: 4px;
  color: #6d7d83;
  font-size: 12px;
  font-weight: 700;
}

.identity-meta span {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.identity-meta span + span::before {
  content: "";
  width: 1px;
  height: 10px;
  background: #cfdedb;
}

.identity-puuid {
  font-family: ui-monospace, "SFMono-Regular", Consolas, monospace;
  letter-spacing: 0.02em;
}

.identity-rank {
  display: grid;
  flex: 1 1 320px;
  grid-template-columns: repeat(auto-fit, minmax(148px, 1fr));
  gap: 8px;
}

.rank-card {
  --rank-accent: #7d8b91;
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  border: 1px solid #e2ebe8;
  border-left: 4px solid var(--rank-accent);
  border-radius: 8px;
  background: #ffffff;
  padding: 6px 9px;
}

.rank-card.tier-iron {
  --rank-accent: #68727a;
}

.rank-card.tier-bronze {
  --rank-accent: #a56638;
}

.rank-card.tier-silver {
  --rank-accent: #8c9faa;
}

.rank-card.tier-gold {
  --rank-accent: #d0a02f;
}

.rank-card.tier-platinum {
  --rank-accent: #39a18c;
}

.rank-card.tier-emerald {
  --rank-accent: #28a85b;
}

.rank-card.tier-diamond {
  --rank-accent: #4d8de8;
}

.rank-card.tier-master {
  --rank-accent: #9a66d6;
}

.rank-card.tier-grandmaster {
  --rank-accent: #d24d55;
}

.rank-card.tier-challenger {
  --rank-accent: #d89f34;
}

.rank-card.loading {
  justify-content: center;
  color: #6d7d83;
  font-size: 12px;
  font-weight: 800;
}

.rank-card-icon {
  flex: 0 0 auto;
  width: 26px;
  height: 26px;
  object-fit: contain;
}

.rank-card-text {
  min-width: 0;
}

.rank-card-text span {
  display: block;
  color: #6d7d83;
  font-size: 11px;
  font-weight: 800;
}

.rank-card-text strong {
  display: block;
  overflow: hidden;
  color: #263238;
  font-size: 13px;
  font-weight: 900;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (max-width: 1100px) {
  .identity-card {
    flex-direction: column;
    align-items: stretch;
  }

  .identity-rank {
    flex: 1 1 auto;
  }
}
</style>
