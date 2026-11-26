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

export type BullTier = "bull-bull" | "bull-high" | "bull" | "none"

export type BullResult = {
  /**
   * bull-bull: 牛牛（牛10）/ bull-high: 牛8~牛9 高档 / bull: 有牛（牛1~牛7）/ none: 没牛（按最大牌报 X大）
   */
  tier: BullTier
  label: string
  /** 用于比较：有牛 21~30（牛牛=30），没牛 1~10（0 当 10 计） */
  rank: number
  hasBull: boolean
  /** 参与计算的牌面数字 */
  digits: string
  /**
   * 参与计算的牌面点数，从大到小排列（0 当 10 计）。
   * 牌型档位相同时用它逐位比大小决定先后。
   */
  cards: number[]
}

// 牌型一律用阿拉伯数字展示（牛牛 / 牛9 / 牛5 / 10大），不要中英数混排。
// 数字 0 在牛牛里按 10 点计
function bullPoint(digit: number) {
  return digit === 0 ? 10 : digit
}

function sortPointsDesc(points: number[]) {
  return [...points].sort((left, right) => right - left)
}

/**
 * 比牌：按点数从大到小逐个比，0 当 10，所以 0 比 9 大、9 比 8 大。
 * 只比双方都有的位数；前缀完全相同算并列，不靠张数硬拆先后。
 */
export function comparePointsDesc(left: number[], right: number[]) {
  const size = Math.min(left.length, right.length)
  for (let i = 0; i < size; i += 1) {
    if (left[i] !== right[i]) return left[i] - right[i]
  }
  return 0
}

/**
 * 两副牌的先后：先比牌型档位，档位相同再逐位比牌面（从最大牌开始）。
 * 返回 > 0 表示 left 排在 right 前面；返回 0 表示完全打平。
 */
export function compareBull(left: BullResult, right: BullResult) {
  if (left.rank !== right.rank) return left.rank - right.rank
  return comparePointsDesc(left.cards, right.cards)
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

export type BullComputeOptions = {
  /**
   * 伤害位数超过 5 位时的取牌方式：
   * - false / 缺省：在全部位数里任意挑 5 位，取牌型最大的一组（默认，牌面更好看）。
   * - true：只取数字的前 5 位（高位优先），不做任意组合（取法唯一、可预期）。
   */
  leadingFiveOnly?: boolean
}

export function computeBull(damage: number, options: BullComputeOptions = {}): BullResult {
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
  } else if (options.leadingFiveOnly) {
    // 开关打开：只看数字前 5 位（高位优先），第 6 位起直接忽略，取法唯一可预期。
    chosen = digits.slice(0, 5)
    const bull = bullRankOfFive(chosen.map(bullPoint))
    rank = bull > 0 ? 20 + bull : Math.max(...chosen.map(bullPoint))
  } else {
    // 多于 5 位：任意取 5 位拼出最大牛；档位相同时留牌面更大的那种取法，
    // 保证同一个人只会有一个确定的牌面，比牌结果才稳定。
    let bestRank = -1
    let bestChoice: number[] = []
    let bestCards: number[] = []
    const picked: number[] = []
    const n = digits.length
    const walk = (start: number) => {
      if (picked.length === 5) {
        const points = picked.map((id) => bullPoint(digits[id]))
        const bull = bullRankOfFive(points)
        const r = bull > 0 ? 20 + bull : Math.max(...points)
        const cards = sortPointsDesc(points)
        if (r > bestRank || (r === bestRank && comparePointsDesc(cards, bestCards) > 0)) {
          bestRank = r
          bestChoice = picked.map((id) => digits[id])
          bestCards = cards
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
  const bull = rank - 20
  const tier: BullTier = hasBull ? (rank === 30 ? "bull-bull" : bull >= 8 ? "bull-high" : "bull") : "none"
  const label = hasBull
    ? rank === 30
      ? "牛牛"
      : `牛${bull}`
    : `${Math.max(...chosen.map(bullPoint))}大`
  return { tier, label, rank, hasBull, digits: chosen.join(""), cards: sortPointsDesc(chosen.map(bullPoint)) }
}
