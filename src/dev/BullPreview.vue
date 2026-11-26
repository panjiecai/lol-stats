<script setup lang="ts">
import { computed } from "vue"
import {
  BULL_TIER_META,
  BULL_TIER_ORDER,
  buildBullBoard,
  bullTierClass,
  findOwnerTeam,
} from "../bullDisplay"
import type { MatchDetailTeam } from "../types"
import { computeBull } from "../utils"
import BullRankBoard from "../components/BullRankBoard.vue"

// 仅开发期预览用：mock 一局 10 人数据，检查四档徽章配色与牛气排行排版
function player(teamId: number, participantId: number, damage: number, name: string) {
  return {
    teamId,
    participantId,
    damageToChampions: damage,
    puuid: `puuid-${teamId}-${participantId}`,
    gameName: name,
    tagLine: "CN1",
    summonerName: name,
    championId: 1,
  }
}

const mockTeams = [
  {
    teamId: 100,
    name: "红队",
    win: true,
    towerKills: 0,
    players: [
      player(100, 1, 55000, "牛气冲天"),
      player(100, 2, 23580, "就差一点"),
      player(100, 3, 12790, "稳如老牛"),
      player(100, 4, 12345, "中规中矩"),
      player(100, 5, 11111, "很遗憾"),
    ],
  },
  {
    teamId: 200,
    name: "蓝队",
    win: false,
    towerKills: 0,
    players: [
      player(200, 6, 55000, "双牛牛"),
      player(200, 7, 22000, "小牛一头"),
      player(200, 8, 900, "伤害好低"),
      player(200, 9, 4444, "四大"),
      player(200, 10, 1111, "没牛选手"),
    ],
  },
] as unknown as MatchDetailTeam[]

const OWNER_PUUID = "puuid-100-1"
// 排行榜只统计同一边：本队（红队）与全场各来一份对照
const lobbyBoard = computed(() => buildBullBoard(mockTeams))
const teamBoard = computed(() => {
  const ownerTeam = findOwnerTeam(mockTeams, OWNER_PUUID, "")
  return buildBullBoard(ownerTeam ? [ownerTeam] : mockTeams)
})

const sampleDamage = [55000, 23580, 12790, 12345, 11111, 900]

const samples = computed(() =>
  sampleDamage.map((damage) => ({
    damage,
    text: `${(damage / 1000).toFixed(1)}k`,
    share: `(${Math.round((damage / 120000) * 100)}%)`,
    result: computeBull(damage),
    rank: teamBoard.value.entries.find((entry) => entry.player.damageToChampions === damage)?.rankAll ?? 0,
  })),
)
</script>

<template>
  <main class="preview">
    <h1>战绩详情 · 牛牛效果预览（开发期）</h1>

    <section class="card">
      <h2>四档徽章配色（下面一行放大 2 倍看细节）</h2>
      <div class="chip-row">
        <span v-for="tier in BULL_TIER_ORDER" :key="tier" class="chip-item">
          <b class="bull-chip" :class="bullTierClass(tier)">{{ BULL_TIER_META[tier].label }}</b>
          <em>{{ BULL_TIER_META[tier].hint }}</em>
        </span>
      </div>
      <div class="chip-row zoom">
        <b class="bull-chip" :class="bullTierClass(tier)" v-for="tier in BULL_TIER_ORDER" :key="`zoom-${tier}`">
          {{ BULL_TIER_META[tier].label }}
        </b>
      </div>
    </section>

    <section class="card">
      <h2>伤害列：徽章 + 同边排名</h2>
      <ul class="damage-row">
        <li v-for="sample in samples" :key="sample.damage">
          <strong>
            {{ sample.text }}<em class="damage-sub">
              <span>{{ sample.share }}</span>
              <b class="bull-chip" :class="bullTierClass(sample.result.tier)">{{ sample.result.label }}</b>
              <em v-if="sample.rank" class="bull-rank-badge" :class="`rank-${Math.min(sample.rank, 3)}`">
                #{{ sample.rank }}
              </em>
            </em>
          </strong>
        </li>
      </ul>
    </section>

    <section class="card">
      <h2>牛气排行（本队口径，实际渲染）</h2>
      <BullRankBoard :board="teamBoard" :champions="{}" :owner-puuid="OWNER_PUUID" title="牛气排行 · 红队" />
      <p class="hint">只统计同一边：{{ teamBoard.totalPlayers }} 人；头像需要 Tauri 资源加载，网页预览里显示英雄 ID 占位</p>
    </section>

    <section class="card">
      <h2>对照：不限定同一边时的全场口径</h2>
      <BullRankBoard :board="lobbyBoard" :champions="{}" :owner-puuid="OWNER_PUUID" title="牛气排行 · 全场" />
    </section>

    <section class="card">
      <h2>开关按钮两种状态</h2>
      <div class="toggle-row">
        <button class="preview-toggle"><span class="bull-toggle-dot"></span>牛牛 开</button>
        <button class="preview-toggle off"><span class="bull-toggle-dot"></span>牛牛 关</button>
      </div>
    </section>
  </main>
</template>

<style scoped>
.preview {
  display: flex;
  flex-direction: column;
  gap: 14px;
  min-height: 100vh;
  background: #f6faf9;
  color: #35434a;
  font-family: "Microsoft YaHei", system-ui, sans-serif;
  padding: 18px 22px 40px;
}

h1 {
  color: #1f5f56;
  font-size: 18px;
}

h2 {
  font-size: 13px;
}

.card {
  border: 1px solid #dce7e4;
  border-radius: 10px;
  background: #ffffff;
  padding: 12px 14px;
}

.chip-row {
  display: flex;
  flex-wrap: wrap;
  gap: 18px;
}

.chip-row.zoom {
  align-items: center;
  margin-top: 12px;
  border-top: 1px dashed #dce7e4;
  padding-top: 12px;
  zoom: 2;
}

.chip-item {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 5px;
}

.chip-item em {
  color: #7d8b8f;
  font-size: 11px;
  font-style: normal;
}

.damage-row {
  display: flex;
  flex-wrap: wrap;
  gap: 18px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.damage-sub {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: #8a989c;
  font-size: 13.5px;
  font-style: normal;
  font-weight: 700;
}

.toggle-row {
  display: flex;
  gap: 10px;
}

.preview-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: none;
  border-radius: 8px;
  color: #8a4a12;
  background: #fff3d6;
  font-size: 12px;
  font-weight: 800;
  padding: 7px 10px;
}

.preview-toggle .bull-toggle-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: linear-gradient(135deg, #ffd447, #ff8a1f);
  box-shadow: 0 0 6px rgba(255, 138, 31, 0.8);
}

.preview-toggle.off {
  color: #657179;
  background: #f0f3f4;
}

.preview-toggle.off .bull-toggle-dot {
  background: #b6c1c4;
  box-shadow: none;
}

.hint {
  color: #7d8b8f;
  font-size: 11px;
}
</style>
