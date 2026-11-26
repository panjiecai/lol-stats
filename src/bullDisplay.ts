import { ref } from "vue"
import { readBooleanSetting, writeBooleanSetting } from "./toolSettings"
import type { MatchDetailPlayer, MatchDetailTeam } from "./types"
import { computeBull, compareBull, type BullResult, type BullTier } from "./utils"

/** 战绩详情里的「牛牛」展示开关，默认开启 */
export const BULL_DETAIL_ENABLED_KEY = "lol-stats.match-detail.bull.enabled"

export const BULL_TIER_ORDER: BullTier[] = ["bull-bull", "bull-high", "bull", "none"]

export const BULL_TIER_META: Record<BullTier, { label: string; hint: string }> = {
  "bull-bull": { label: "牛牛", hint: "伤害数字里任意 3 张成 10 点倍数，剩余也是 10 点，最大牌型" },
  "bull-high": { label: "牛8·牛9", hint: "有牛且点数很高（牛8 / 牛9），仅次于牛牛" },
  bull: { label: "有牛", hint: "有牛但点数一般（牛1~牛7）" },
  none: { label: "没牛", hint: "凑不出 10 点倍数，按最大牌报 X大（0 当 10 点）" },
}

/**
 * 档位 → CSS 类名。tier 本身带 `bull-` 前缀，不能直接拼 `bull-${tier}`，
 * 否则牛牛会拼成 bull-bull-bull 而匹配不到任何规则。CSS 类名以这里为准。
 */
const BULL_TIER_CLASS: Record<BullTier, string> = {
  "bull-bull": "bull-bull",
  "bull-high": "bull-high",
  bull: "bull",
  none: "bull-none",
}

export function bullTierClass(tier: BullTier) {
  return BULL_TIER_CLASS[tier]
}

const bullDetailEnabled = ref(readBooleanSetting(BULL_DETAIL_ENABLED_KEY, true))

export { bullDetailEnabled }

export function toggleBullDetail() {
  bullDetailEnabled.value = !bullDetailEnabled.value
  writeBooleanSetting(BULL_DETAIL_ENABLED_KEY, bullDetailEnabled.value)
}

/**
 * 伤害超过 5 位时的取牌口径开关，默认关闭：
 * - 关闭：在全部位数里任意挑 5 位，取牌型最大的一组。
 * - 打开：只取数字前 5 位（高位优先），第 6 位起忽略。
 */
export const BULL_LEADING_FIVE_KEY = "lol-stats.match-detail.bull.leading-five"

export const bullLeadingFiveOnly = ref(readBooleanSetting(BULL_LEADING_FIVE_KEY, false))

export function setBullLeadingFiveOnly(value: boolean) {
  bullLeadingFiveOnly.value = value
  writeBooleanSetting(BULL_LEADING_FIVE_KEY, value)
}

export function toggleBullLeadingFiveOnly() {
  setBullLeadingFiveOnly(!bullLeadingFiveOnly.value)
}

/** 当前取牌口径的中文说明，用于按钮 tooltip 与设置页。 */
export function bullDrawRuleText() {
  return bullLeadingFiveOnly.value
    ? "超过 5 位的伤害只取前 5 位（不做任意组合）"
    : "超过 5 位的伤害在全部位数里挑最大的 5 张牌"
}

export function bullPlayerKey(player: MatchDetailPlayer) {
  return `${player.teamId}:${player.participantId}`
}

export function bullTitle(player: MatchDetailPlayer, result: BullResult) {
  const tierText = result.hasBull ? result.label : `没牛 · ${result.label}`
  return `伤害 ${player.damageToChampions} → 牌面 ${result.digits} · ${tierText}（0 按 10 点计）`
}

export type BullBoardEntry = {
  key: string
  player: MatchDetailPlayer
  result: BullResult
  /** 全场名次（同牌型再比牌，完全打平才并列） */
  rankAll: number
  /** 队内名次（同牌型再比牌，完全打平才并列） */
  rankTeam: number
  teamSize: number
}

export type BullBoardTeam = {
  teamId: number
  name: string
  win: boolean
  /** 队伍牛气总分：有牛记 21~30、没牛记 1~10 */
  score: number
}

export type BullBoard = {
  entries: BullBoardEntry[]
  byKey: Record<string, BullBoardEntry>
  totalPlayers: number
  teams: BullBoardTeam[]
  top: BullBoardEntry | null
  topTeam: BullBoardTeam | null
  /** team = 只统计同一边（本队）；lobby = 全场（拿不到本队时的兜底） */
  scope: BullBoardScope
  /** scope=team 时的队名 */
  teamName: string
  /** 名次口径文案：本队 / 全场 */
  scopeLabel: string
}

export type BullBoardScope = "team" | "lobby"

/**
 * 按比较器降序排名：比较器判定完全打平才并列（1,1,3），否则依次占位。
 * 牛牛里同牌型会继续比牌面，所以只有牌面一模一样才会并列。
 */
function rankByComparator<T>(items: T[], compare: (a: T, b: T) => number) {
  const sorted = [...items].sort((a, b) => compare(b, a))
  const ranks = new Map<T, number>()
  let rank = 0
  sorted.forEach((item, index) => {
    if (index === 0 || compare(sorted[index - 1], item) !== 0) rank = index + 1
    ranks.set(item, rank)
  })
  return { sorted, ranks }
}

/**
 * 按牛牛牌面给传入的队伍排名（牛气排行榜只统计同一边时，调用方只传本队）。
 * 先比牌型档位（有牛 21~30 恒高于没牛 1~10），同档位再逐位比牌面大小，
 * 牌面完全一样才算并列。只传一支队伍就是本队口径，名次与分母都只算这支队伍的人。
 */
export function buildBullBoard(
  teams?: MatchDetailTeam[],
  options: { leadingFiveOnly?: boolean } = {},
): BullBoard {
  const list = teams || []
  // 默认跟随全局开关；在 computed 里调用时自动建立响应式依赖，切换开关即时重算。
  const leadingFiveOnly = options.leadingFiveOnly ?? bullLeadingFiveOnly.value
  const resultOf = new Map<MatchDetailPlayer, BullResult>()
  for (const team of list) {
    for (const player of team.players) {
      resultOf.set(player, computeBull(player.damageToChampions, { leadingFiveOnly }))
    }
  }

  const comparePlayers = (a: MatchDetailPlayer, b: MatchDetailPlayer) =>
    compareBull(resultOf.get(a)!, resultOf.get(b)!)

  const allPlayers = list.flatMap((team) => team.players)
  const { sorted, ranks } = rankByComparator(allPlayers, comparePlayers)

  const teamRankMaps = new Map<number, Map<MatchDetailPlayer, number>>()
  for (const team of list) {
    const teamRanks = rankByComparator(team.players, comparePlayers).ranks
    teamRankMaps.set(team.teamId, teamRanks)
  }

  const entries: BullBoardEntry[] = sorted.map((player) => ({
    key: bullPlayerKey(player),
    player,
    result: resultOf.get(player)!,
    rankAll: ranks.get(player) ?? 0,
    rankTeam: teamRankMaps.get(player.teamId)?.get(player) ?? 0,
    teamSize: list.find((team) => team.teamId === player.teamId)?.players.length ?? 0,
  }))

  const byKey: Record<string, BullBoardEntry> = {}
  for (const entry of entries) byKey[entry.key] = entry

  const boardTeams: BullBoardTeam[] = list.map((team) => ({
    teamId: team.teamId,
    name: team.name,
    win: team.win,
    score: team.players.reduce((sum, player) => sum + (resultOf.get(player)?.rank ?? 0), 0),
  }))

  const topTeam =
    boardTeams.length > 1
      ? boardTeams.reduce((best, team) => (team.score > best.score ? team : best))
      : boardTeams[0] || null

  const scope: BullBoardScope = list.length === 1 ? "team" : "lobby"

  return {
    entries,
    byKey,
    totalPlayers: allPlayers.length,
    teams: boardTeams,
    top: entries[0] || null,
    topTeam,
    scope,
    teamName: scope === "team" ? list[0]?.name || "" : "",
    scopeLabel: scope === "team" ? "本队" : "全场",
  }
}

/**
 * 找出某个玩家（按 puuid 或 gameName#tagLine）所在的队伍。
 * 牛气排行榜用它把统计范围收窄到「同一边」。
 */
export function findOwnerTeam(
  teams: MatchDetailTeam[] | undefined,
  ownerPuuid?: string,
  ownerLabel?: string,
): MatchDetailTeam | null {
  const list = teams || []
  const puuid = ownerPuuid?.trim().toLowerCase() || ""
  const label = ownerLabel?.trim().toLowerCase() || ""
  if (!puuid && !label) return null

  for (const team of list) {
    for (const player of team.players) {
      if (puuid && (player.puuid?.trim().toLowerCase() || "") === puuid) return team
      if (label && bullPlayerLabel(player).trim().toLowerCase() === label) return team
    }
  }

  return null
}

/** 名次说明文案，跟随排行榜口径（本队 / 全场）。 */
export function bullRankLine(entry: BullBoardEntry, board: BullBoard) {
  const total = board.scope === "team" ? entry.teamSize : board.totalPlayers
  if (board.scope === "team") {
    const suffix = board.teamName ? `（${board.teamName}）` : ""
    return `牛气本队第 ${entry.rankAll}/${total}${suffix}`
  }
  return `牛气全场第 ${entry.rankAll}/${total}`
}

export function bullPlayerLabel(player: MatchDetailPlayer) {
  if (player.gameName && player.tagLine) return `${player.gameName}#${player.tagLine}`
  return player.summonerName || player.puuid || "未知玩家"
}

export function bullDamageText(value: number) {
  return `${(value / 1000).toFixed(1)}k`
}
