<script setup lang="ts">
import { computed } from "vue"
import type { ChampionSummaryItem } from "../types"
import {
  BULL_TIER_META,
  BULL_TIER_ORDER,
  bullDamageText,
  bullPlayerLabel,
  bullRankLine,
  bullTierClass,
  bullTitle,
  type BullBoard,
  type BullBoardEntry,
} from "../bullDisplay"
import ChampionAvatar from "./ChampionAvatar.vue"

const props = withDefaults(
  defineProps<{
    board: BullBoard
    champions: Record<number, ChampionSummaryItem>
    ownerPuuid?: string
    title?: string
  }>(),
  {
    title: "牛气排行",
    ownerPuuid: "",
  },
)

const normalizedOwnerPuuid = computed(() => props.ownerPuuid.trim().toLowerCase())
const scoreMax = computed(() => Math.max(1, ...props.board.teams.map((team) => team.score)))

function isOwner(player: { puuid?: string }) {
  const puuid = player.puuid?.trim().toLowerCase() || ""
  return Boolean(normalizedOwnerPuuid.value) && puuid === normalizedOwnerPuuid.value
}

function barWidth(score: number) {
  return `${Math.max(6, Math.round((score / scoreMax.value) * 100))}%`
}

function entryTitle(entry: BullBoardEntry) {
  return [bullTitle(entry.player, entry.result), bullRankLine(entry, props.board)].join("\n")
}
</script>

<template>
  <section class="bull-board">
    <header class="bull-board-head">
      <strong class="bull-board-title">{{ title }}</strong>
      <span class="bull-board-note">{{ board.scopeLabel }} {{ board.totalPlayers }} 人 · 同牌型再比牌</span>
      <div class="bull-board-legend">
        <span
          v-for="tier in BULL_TIER_ORDER"
          :key="tier"
          class="bull-chip"
          :class="bullTierClass(tier)"
          :title="BULL_TIER_META[tier].hint"
        >
          {{ BULL_TIER_META[tier].label }}
        </span>
      </div>
    </header>

    <div class="bull-team-compare" v-if="board.teams.length > 1">
      <div
        v-for="team in board.teams"
        :key="team.teamId"
        class="bull-team-row"
        :class="{ win: team.win }"
      >
        <span class="bull-team-name">{{ team.name }}</span>
        <span class="bull-team-bar"><i :style="{ width: barWidth(team.score) }"></i></span>
        <strong>{{ team.score }}</strong>
        <span v-if="board.topTeam && team.teamId === board.topTeam.teamId" class="bull-team-flag">牛气领先</span>
      </div>
    </div>

    <ol class="bull-rank-list">
      <li
        v-for="entry in board.entries"
        :key="entry.key"
        class="bull-rank-row"
        :class="[bullTierClass(entry.result.tier), { owner: isOwner(entry.player) }]"
        :title="entryTitle(entry)"
      >
        <span class="bull-rank-no" :class="`no-${Math.min(entry.rankAll, 4)}`">{{ entry.rankAll }}</span>
        <ChampionAvatar :champion-id="entry.player.championId" :champions="champions" :size="44" />
        <span class="bull-rank-name">{{ bullPlayerLabel(entry.player) }}</span>
        <b class="bull-chip" :class="bullTierClass(entry.result.tier)">{{ entry.result.label }}</b>
        <span class="bull-rank-face">牌面 {{ entry.result.digits }}</span>
        <span class="bull-rank-dmg">{{ bullDamageText(entry.player.damageToChampions) }}</span>
      </li>
    </ol>
  </section>
</template>

<style scoped>
.bull-board {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border: 1px solid rgba(31, 95, 86, 0.16);
  border-radius: 10px;
  background: linear-gradient(180deg, #fffdf5, #f7fbfa);
  padding: 10px 12px;
}

.bull-board-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.bull-board-title {
  color: #1f5f56;
  font-size: 13px;
}

.bull-board-note {
  color: #7d8b8f;
  font-size: 11px;
  font-weight: 700;
}

.bull-board-legend {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: auto;
}

.bull-team-compare {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.bull-team-row {
  display: grid;
  grid-template-columns: 74px 1fr 34px auto;
  align-items: center;
  gap: 8px;
  color: #5c6a6e;
  font-size: 11px;
  font-weight: 700;
}

.bull-team-row strong {
  color: #1f5f56;
  font-size: 13px;
  text-align: right;
}

.bull-team-row.win strong {
  color: #b1443a;
}

.bull-team-bar {
  height: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: rgba(31, 95, 86, 0.1);
}

.bull-team-bar i {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, #8fd8cf, #1f5f56);
}

.bull-team-row.win .bull-team-bar i {
  background: linear-gradient(90deg, #ffc98a, #e0784a);
}

.bull-team-flag {
  border-radius: 999px;
  color: #8a4a12;
  background: #ffeccf;
  font-size: 10px;
  font-weight: 900;
  padding: 2px 6px;
}

.bull-rank-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

/* 第 2 列 = 头像列，必须跟头像尺寸同步（44 = 22 × 2，用户定的 2 倍）。
   行高不写死，让头像自己把行撑开，不存在裁切问题。 */
.bull-rank-row {
  display: grid;
  grid-template-columns: 18px 44px minmax(0, 1fr) auto auto 46px;
  align-items: center;
  gap: 7px;
  border-radius: 7px;
  border-left: 3px solid transparent;
  padding: 3px 6px 3px 4px;
}

.bull-rank-row.bull-bull {
  background-image: linear-gradient(90deg, rgba(255, 120, 40, 0.32), rgba(255, 196, 60, 0.12) 62%, rgba(255, 196, 60, 0) 88%);
  border-left-color: #e0501a;
  box-shadow: inset 0 0 0 1px rgba(255, 120, 40, 0.4), 0 0 12px rgba(255, 110, 30, 0.4);
}

.bull-rank-row.bull-high {
  background-image: linear-gradient(90deg, rgba(255, 214, 150, 0.24), rgba(255, 214, 150, 0) 74%);
  border-left-color: #ffb03a;
}

.bull-rank-row.bull {
  border-left-color: #7fd4cb;
}

.bull-rank-row.bull-none {
  border-left-color: #d3dcde;
}

/* 本人所在行：底色 + 描边，任何档位下都能一眼认出来 */
.bull-rank-row.owner {
  background-color: rgba(255, 214, 120, 0.28);
  outline: 2px solid rgba(31, 95, 86, 0.5);
  outline-offset: -2px;
}

.bull-rank-no {
  color: #7d8b8f;
  font-size: 12px;
  font-weight: 900;
  text-align: center;
}

.bull-rank-no.no-1 {
  color: #b9760d;
}

.bull-rank-no.no-2 {
  color: #6f8288;
}

.bull-rank-no.no-3 {
  color: #a1652f;
}

.bull-rank-name {
  overflow: hidden;
  color: #35434a;
  font-size: 12px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bull-rank-face,
.bull-rank-dmg {
  color: #8a989c;
  font-size: 11px;
  font-weight: 700;
  white-space: nowrap;
}

.bull-rank-dmg {
  text-align: right;
}
</style>
