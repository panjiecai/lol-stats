<script setup lang="ts">
import { computed, ref, watch } from "vue"
import { LoaderCircle, RefreshCw } from "lucide-vue-next"
import { debugOfficialApi } from "../api"
import type { OfficialApiDebugResponse } from "../types"

const props = defineProps<{
  active: boolean
}>()

const loading = ref(false)
const loaded = ref(false)
const error = ref("")
const payload = ref<OfficialApiDebugResponse | null>(null)

const sampleGames = computed(() => payload.value?.sampleGames || [])

watch(
  () => props.active,
  (active) => {
    if (!active || loaded.value || loading.value) return
    void loadProbe()
  },
  { immediate: true },
)

async function loadProbe() {
  if (loading.value) return
  loading.value = true
  error.value = ""
  try {
    payload.value = await debugOfficialApi()
    loaded.value = true
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
  } finally {
    loading.value = false
  }
}

function dateTimeText(timestamp: number) {
  if (!timestamp) return "--"
  return new Date(timestamp).toLocaleString("zh-CN", { hour12: false })
}
</script>

<template>
  <section class="official-probe-panel">
    <header class="official-probe-header">
      <div>
        <strong>官方接口探查</strong>
        <p>直接读取当前客户端能访问到的官方接口原始返回，方便确认到底拿到了哪些字段。</p>
      </div>

      <button class="official-probe-refresh" :disabled="loading" @click="loadProbe">
        <LoaderCircle v-if="loading" class="spin" :size="16" />
        <RefreshCw v-else :size="16" />
        <span>{{ loading ? "读取中" : "刷新" }}</span>
      </button>
    </header>

    <div v-if="error" class="official-probe-error">{{ error }}</div>

    <template v-else-if="payload">
      <section class="official-probe-summary">
        <article class="official-probe-card">
          <span>当前召唤师</span>
          <strong>{{ payload.summoner.gameName }}<template v-if="payload.summoner.tagLine">#{{ payload.summoner.tagLine }}</template></strong>
          <small>{{ payload.summoner.displayName || "当前客户端账号" }}</small>
        </article>

        <article class="official-probe-card">
          <span>当前区服</span>
          <strong>{{ payload.sgpServerId }}</strong>
          <small>{{ payload.baseUrl }}</small>
        </article>

        <article class="official-probe-card">
          <span>最近样本局</span>
          <strong>{{ sampleGames.length }} 局</strong>
          <small>默认展示最近 {{ sampleGames.length }} 局官方详情原始返回</small>
        </article>
      </section>

      <section class="official-probe-note">
        <strong>说明</strong>
        <span>{{ payload.note }}</span>
      </section>

      <section class="official-probe-block">
        <header>
          <strong>官方段位接口</strong>
          <small>{{ payload.rankedRequestUrl }}</small>
        </header>

        <div class="official-probe-tags">
          <span v-for="path in payload.rankedLikePaths" :key="path">{{ path }}</span>
          <em v-if="!payload.rankedLikePaths.length">未命中疑似分数字段</em>
        </div>

        <pre>{{ payload.rankedPayload || "--" }}</pre>
      </section>

      <section class="official-probe-block">
        <header>
          <strong>官方深战绩接口</strong>
          <small>{{ payload.matchHistoryRequestUrl }}</small>
        </header>

        <div class="official-probe-tags">
          <span v-for="path in payload.matchHistoryLikePaths" :key="path">{{ path }}</span>
          <em v-if="!payload.matchHistoryLikePaths.length">未命中疑似分数字段</em>
        </div>

        <pre>{{ payload.matchHistoryPayload || "--" }}</pre>
      </section>

      <section class="official-probe-games">
        <article v-for="game in sampleGames" :key="game.gameId" class="official-probe-block">
          <header class="official-probe-game-head">
            <div>
              <strong>对局 {{ game.gameId }}</strong>
              <small>{{ game.gameMode || "未知模式" }} · 队列 {{ game.queueId || 0 }} · {{ dateTimeText(game.gameCreation) }}</small>
            </div>
            <span class="official-probe-game-champion">玩家英雄 {{ game.championId || "--" }}</span>
          </header>

          <div class="official-probe-subblock">
            <h4>疑似分数字段路径</h4>
            <div class="official-probe-tags">
              <span v-for="path in game.rankLikePaths" :key="`${game.gameId}-${path}`">{{ path }}</span>
              <em v-if="!game.rankLikePaths.length">未命中疑似分数字段</em>
            </div>
          </div>

          <div class="official-probe-subblock">
            <h4>首个玩家 challenges 命中</h4>
            <div class="official-probe-tags">
              <span
                v-for="entry in game.firstParticipantChallengeHits"
                :key="`${game.gameId}-${entry}`"
              >
                {{ entry }}
              </span>
              <em v-if="!game.firstParticipantChallengeHits.length">没有命中 rank/elo/mmr 相关 key</em>
            </div>
          </div>

          <small class="official-probe-url">{{ game.summaryRequestUrl }}</small>
          <pre>{{ game.summaryPayload || "--" }}</pre>
        </article>
      </section>
    </template>

    <section v-else class="official-probe-empty">
      <strong>还没有读取官方接口数据</strong>
      <span>点上面的刷新后，会把当前账号能访问到的官方接口原始返回直接展示出来。</span>
    </section>
  </section>
</template>

<style scoped>
.official-probe-panel {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 12px;
}

.official-probe-header,
.official-probe-summary,
.official-probe-tags {
  display: flex;
}

.official-probe-header {
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.official-probe-header strong,
.official-probe-empty strong,
.official-probe-note strong,
.official-probe-block strong {
  color: #20333a;
  font-weight: 950;
}

.official-probe-header strong {
  display: block;
  font-size: 20px;
  margin-bottom: 6px;
}

.official-probe-header p,
.official-probe-note span,
.official-probe-card small,
.official-probe-empty span,
.official-probe-block small,
.official-probe-url {
  margin: 0;
  color: #5f7177;
  font-size: 13px;
  line-height: 1.6;
}

.official-probe-refresh {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border: 1px solid #d9e5e2;
  border-radius: 8px;
  background: #f8fbfa;
  color: #30454a;
  cursor: pointer;
  font-size: 13px;
  font-weight: 900;
  line-height: 1;
  padding: 10px 14px;
}

.official-probe-refresh:disabled {
  cursor: wait;
  opacity: 0.7;
}

.official-probe-summary {
  gap: 12px;
}

.official-probe-card,
.official-probe-note,
.official-probe-block,
.official-probe-empty {
  border: 1px solid #dbe8e5;
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 0 12px 28px rgba(32, 67, 73, 0.08);
}

.official-probe-card {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 14px;
}

.official-probe-card span {
  color: #6c7a80;
  font-size: 12px;
  font-weight: 900;
}

.official-probe-card strong {
  font-size: 24px;
  line-height: 1.1;
}

.official-probe-note,
.official-probe-empty {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 14px 16px;
}

.official-probe-block {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 16px;
}

.official-probe-block header {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.official-probe-game-head {
  gap: 8px;
}

.official-probe-game-champion {
  color: #30535d;
  font-size: 13px;
  font-weight: 900;
}

.official-probe-subblock {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.official-probe-subblock h4 {
  margin: 0;
  color: #20333a;
  font-size: 13px;
  font-weight: 950;
}

.official-probe-tags {
  flex-wrap: wrap;
  gap: 8px;
}

.official-probe-tags span,
.official-probe-tags em {
  border-radius: 999px;
  font-size: 12px;
  line-height: 1;
  padding: 7px 10px;
}

.official-probe-tags span {
  background: #eef6f4;
  color: #275f54;
  font-weight: 900;
}

.official-probe-tags em {
  background: #f5f7f7;
  color: #6f8085;
  font-style: normal;
  font-weight: 800;
}

.official-probe-block pre {
  margin: 0;
  max-height: 360px;
  overflow: auto;
  border-radius: 8px;
  background: #f7fbfa;
  color: #41545b;
  font-family: Consolas, "SFMono-Regular", monospace;
  font-size: 12px;
  line-height: 1.5;
  padding: 12px;
  white-space: pre-wrap;
  word-break: break-word;
}

.official-probe-games {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.official-probe-error {
  border-radius: 8px;
  background: #fff1f1;
  color: #a73a3a;
  font-size: 13px;
  font-weight: 900;
  line-height: 1.6;
  padding: 12px 14px;
}

.official-probe-empty {
  min-height: 180px;
  justify-content: center;
}

.spin {
  animation: official-probe-spin 0.8s linear infinite;
}

@keyframes official-probe-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

@media (max-width: 920px) {
  .official-probe-summary {
    flex-direction: column;
  }

  .official-probe-header {
    flex-direction: column;
  }
}
</style>
