import type { ChampionSummaryItem, RecentGame, SummonerInfo } from "./types"

export function riotId(summoner?: SummonerInfo) {
  if (!summoner) return "未知玩家"
  if (summoner.gameName && summoner.tagLine) return `${summoner.gameName}#${summoner.tagLine}`
  return summoner.displayName || summoner.internalName || "未知玩家"
}

export function percent(value: number) {
  return `${Math.round(value * 100)}%`
}

export function fixed(value: number, digits = 2) {
  return Number.isFinite(value) ? value.toFixed(digits) : "0.00"
}

export function compactNumber(value: number) {
  return new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 0 }).format(value)
}

export function mitigationValue(game: RecentGame) {
  return Number(game.damageSelfMitigated || 0) + Number(game.totalDamageTaken || 0)
}

export function teamMitigationValue(game: RecentGame) {
  return Number(game.teamDamageSelfMitigated || 0) + Number(game.teamTotalDamageTaken || 0)
}

export function formatDate(timestamp: number) {
  if (!timestamp) return "-"
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp))
}

export function championName(
  champions: Record<number, ChampionSummaryItem>,
  championId?: number,
) {
  if (!championId) return "未选择"
  const champion = champions[championId]
  return champion?.name || champion?.alias || `英雄 ${championId}`
}

export function phaseName(phase?: string) {
  const map: Record<string, string> = {
    None: "空闲",
    Lobby: "房间",
    Matchmaking: "匹配中",
    ReadyCheck: "等待接受",
    ChampSelect: "选人中",
    GameStart: "游戏开始",
    InProgress: "游戏中",
    Reconnect: "可重连",
    WaitingForStats: "等待结算",
    PreEndOfGame: "结算中",
    EndOfGame: "游戏结束",
  }
  return phase ? map[phase] || phase : "-"
}

export type BullResult = {
  /** bull-bull: 牛牛 / bull: 牛1~牛9 / none: 没牛（按最大牌报 X大） */
  tier: "bull-bull" | "bull" | "none"
  label: string
  /** 用于比较：有牛 21~30（牛牛=30），没牛 1~10（0 当 10 计） */
  rank: number
  hasBull: boolean
  /** 参与计算的牌面数字 */
  digits: string
}

const BULL_DIGIT_NAME: Record<number, string> = {
  10: "十",
  9: "九",
  8: "八",
  7: "七",
  6: "六",
  5: "五",
  4: "四",
  3: "三",
  2: "二",
  1: "一",
}

// 数字 0 在牛牛里按 10 点计
function bullPoint(digit: number) {
  return digit === 0 ? 10 : digit
}

// 5 张牌的牌型：任意 3 张之和为 10 的倍数即有牛，
// 最大牛 10 即牛牛，无牛返回 0
function bullRankOfFive(points: number[]) {
  let best = 0
  const total = points[0] + points[1] + points[2] + points[3] + points[4]
  for (let i = 0; i < 5; i++) {
    for (let j = i + 1; j < 5; j++) {
      for (let k = j + 1; k < 5; k++) {
        const three = points[i] + points[j] + points[k]
        if (three % 10 !== 0) continue
        const bull = (total - three) % 10
        if (bull === 0) return 10
        if (bull > best) best = bull
      }
    }
  }
  return best
}

export function computeBull(damage: number): BullResult {
  const text = String(Math.trunc(damage) || 0)
  const digits = Array.from(text, (ch) => Number(ch))
  let chosen: number[]
  let rank: number

  if (digits.length < 5) {
    // 不够 5 位：没牛，按最大数字报 X大，0 最大（当 10）
    chosen = digits
    rank = Math.max(...chosen.map(bullPoint))
  } else if (digits.length === 5) {
    chosen = digits
    const bull = bullRankOfFive(chosen.map(bullPoint))
    rank = bull > 0 ? 20 + bull : Math.max(...chosen.map(bullPoint))
  } else {
    // 多于 5 位：任意取 5 位拼出最大牛
    let bestRank = -1
    let bestChoice: number[] = []
    const picked: number[] = []
    const n = digits.length
    const walk = (start: number) => {
      if (picked.length === 5) {
        const points = picked.map((id) => bullPoint(digits[id]))
        const bull = bullRankOfFive(points)
        const r = bull > 0 ? 20 + bull : Math.max(...points)
        if (r > bestRank) {
          bestRank = r
          bestChoice = picked.map((id) => digits[id])
        }
        return
      }
      for (let i = start; i < n; i++) {
        picked.push(i)
        walk(i + 1)
        picked.pop()
      }
    }
    walk(0)
    chosen = bestChoice
    rank = bestRank
  }

  const hasBull = rank >= 21
  const tier = hasBull ? (rank === 30 ? "bull-bull" : "bull") : "none"
  const label = hasBull
    ? rank === 30
      ? "牛牛"
      : `牛${rank - 20}`
    : `${BULL_DIGIT_NAME[Math.max(...chosen.map(bullPoint))]}大`
  return { tier, label, rank, hasBull, digits: chosen.join("") }
}
