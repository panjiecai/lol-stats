import { readFileSync } from "node:fs"
import path from "node:path"
import {
  BULL_TIER_ORDER,
  buildBullBoard,
  bullDetailEnabled,
  bullLeadingFiveOnly,
  bullRankLine,
  bullTierClass,
  findOwnerTeam,
  setBullLeadingFiveOnly,
  toggleBullDetail,
} from "../src/bullDisplay"
import {
  clampStatsDepth,
  DEFAULT_STATS_DEPTH,
  MAX_STATS_DEPTH,
  MIN_STATS_DEPTH,
  STATS_DEPTH_OPTIONS,
} from "../src/statsDepth"
import type { MatchDetailTeam } from "../src/types"
import { computeBull, compareBull, comparePointsDesc } from "../src/utils"

let failed = 0
function check(label: string, actual: unknown, expected: unknown) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected)
  if (!ok) failed += 1
  console.log(`${ok ? "PASS" : "FAIL"} ${label} -> ${JSON.stringify(actual)}${ok ? "" : ` (期望 ${JSON.stringify(expected)})`}`)
}

// --- 1. 牌型分档 ---
const cases: [number, string, string][] = [
  [55000, "bull-bull", "牛牛"],
  [23580, "bull-high", "牛8"],
  [12790, "bull-high", "牛9"],
  [22000, "bull", "牛4"],
  [12345, "bull", "牛5"],
  [11111, "none", "1大"],
  [4444, "none", "4大"],
  [1111, "none", "1大"],
  [900, "none", "10大"],
  [0, "none", "10大"],
]
for (const [damage, tier, label] of cases) {
  const result = computeBull(damage)
  check(`computeBull(${damage}).tier`, result.tier, tier)
  check(`computeBull(${damage}).label`, result.label, label)
}

// 数字格式统一：牌型标签只用阿拉伯数字（牛牛 是固定术语，允许例外）
const CHINESE_NUMERAL = /[一二三四五六七八九十]/
const illegalLabels = cases
  .map(([damage]) => computeBull(damage).label)
  .filter((label) => label !== "牛牛" && CHINESE_NUMERAL.test(label))
check("牌型标签不含中文数字", illegalLabels, [])

// --- 1b. 同牌型比牌：点数相同再比最大牌，0 当 10（0 > 9 > 8） ---
// 11000 与 57000 都是牛2：前者牌面 10/10/10/1/1，后者 10/10/10/7/5，
// 最大牌都是 10，第二位也是 10，比到第四位 7 > 1 → 57000 排在前面。
const tieA = computeBull(11000)
const tieB = computeBull(57000)
check("11000 是牛2", tieA.label, "牛2")
check("57000 也是牛2", tieB.label, "牛2")
check("两者档位相同", tieA.rank, tieB.rank)
check("57000 牌面更大", compareBull(tieB, tieA) > 0, true)
check("11000 反向比较为负", compareBull(tieA, tieB) < 0, true)
check("同一副牌自己比是打平", compareBull(tieA, tieA), 0)
check("比牌时 0 当 10（10 大 大于 9 大）", comparePointsDesc([10, 1], [9, 9]) > 0, true)

const tieTeams = [
  {
    teamId: 100,
    name: "红队",
    win: true,
    towerKills: 0,
    players: [player(100, 1, 55000), player(100, 2, 11000), player(100, 3, 57000)],
  },
] as unknown as MatchDetailTeam[]
const tieBoard = buildBullBoard(tieTeams)
check("名次序列（牛牛第一、两个牛2 分列二三）", tieBoard.entries.map((entry) => entry.rankAll), [1, 2, 3])
check("两个牛2 按牌面排序", tieBoard.entries.map((entry) => entry.player.damageToChampions), [55000, 57000, 11000])

// --- 1c. 超过 5 位的取牌口径开关 ---
// 123456：默认会在 6 位里任意挑 5 位，能凑出牛牛；
// 打开「只取前 5 位」后只看 12345（1+4+5=10，剩 2+3=5 → 牛5）。
check("默认口径 123456 是牛牛", computeBull(123456).label, "牛牛")
check("只取前 5 位时 123456 变牛5", computeBull(123456, { leadingFiveOnly: true }).label, "牛5")
check("只取前 5 位时牌面就是前 5 位", computeBull(123456, { leadingFiveOnly: true }).digits, "12345")
check("5 位数不受开关影响", computeBull(12345, { leadingFiveOnly: true }).label, "牛5")
check("4 位数不受开关影响", computeBull(4444, { leadingFiveOnly: true }).label, "4大")

const sixTeams = [
  {
    teamId: 100,
    name: "红队",
    win: true,
    towerKills: 0,
    players: [player(100, 1, 123456)],
  },
] as unknown as MatchDetailTeam[]
check("排行榜默认按最大组合算", buildBullBoard(sixTeams).entries[0].result.label, "牛牛")
check("开关默认关闭", bullLeadingFiveOnly.value, false)
setBullLeadingFiveOnly(true)
check("开关打开后只取前 5 位", buildBullBoard(sixTeams).entries[0].result.label, "牛5")
setBullLeadingFiveOnly(false)
check("开关关闭后恢复最大组合", buildBullBoard(sixTeams).entries[0].result.label, "牛牛")

// --- 2. 对比排名 ---
function player(teamId: number, participantId: number, damage: number) {
  return {
    teamId,
    participantId,
    damageToChampions: damage,
    puuid: `puuid-${teamId}-${participantId}`,
    gameName: "召唤师",
    tagLine: String(participantId),
    summonerName: "",
    championId: 1,
  }
}

const teams = [
  {
    teamId: 100,
    name: "红队",
    win: true,
    towerKills: 0,
    players: [player(100, 1, 55000), player(100, 2, 55000), player(100, 3, 23580), player(100, 4, 12345), player(100, 5, 11111)],
  },
  {
    teamId: 200,
    name: "蓝队",
    win: false,
    towerKills: 0,
    players: [player(200, 6, 12790), player(200, 7, 900), player(200, 8, 1111), player(200, 9, 4444), player(200, 10, 22000)],
  },
] as unknown as MatchDetailTeam[]

const board = buildBullBoard(teams)
check("rankAll 序列", board.entries.map((entry) => entry.rankAll), [1, 1, 3, 4, 5, 6, 7, 8, 9, 9])
check("entry0 是牛牛", board.entries[0].result.label, "牛牛")
check("全场第三是牛9", board.entries[2].result.label, "牛9")
check("totalPlayers", board.totalPlayers, 10)
check("红队牛气总分", board.teams.find((team) => team.teamId === 100)?.score, 114)
check("蓝队牛气总分", board.teams.find((team) => team.teamId === 200)?.score, 68)
check("牛气领先队伍", board.topTeam?.teamId, 100)
check("红队4号 队内名次", board.byKey["100:4"].rankTeam, 4)
check("蓝队10号 队内名次", board.byKey["200:10"].rankTeam, 2)
check("蓝队7号 全场名次", board.byKey["200:7"].rankAll, 7)

// --- 3. 开关默认开启 + 可切换 ---
check("默认开启", bullDetailEnabled.value, true)
toggleBullDetail()
check("切换后关闭", bullDetailEnabled.value, false)
toggleBullDetail()
check("再切回开启", bullDetailEnabled.value, true)
check("空对局不报错", buildBullBoard(undefined).entries.length, 0)

// --- 3b. 排行榜只统计同一边 ---
check("两支队伍 = 全场口径", board.scope, "lobby")
check("全场口径报名次", bullRankLine(board.entries[0], board), "牛气全场第 1/10")

const ownerTeam = findOwnerTeam(teams, "puuid-100-1", "")
const teamBoard = buildBullBoard(ownerTeam ? [ownerTeam] : teams)
check("按 puuid 找到本队", ownerTeam?.teamId, 100)
check("只传本队 = 本队口径", teamBoard.scope, "team")
check("本队口径队名", teamBoard.teamName, "红队")
check("本队只统计 5 人", teamBoard.totalPlayers, 5)
check("本队名次序列", teamBoard.entries.map((entry) => entry.rankAll), [1, 1, 3, 4, 5])
check("本队里不含对面玩家", teamBoard.entries.every((entry) => entry.player.teamId === 100), true)
check("本队口径报名次", bullRankLine(teamBoard.entries[0], teamBoard), "牛气本队第 1/5（红队）")
check("按 gameName#tagLine 找到本队", findOwnerTeam(teams, "", "召唤师#6")?.teamId, 200)
check("owner 在蓝队时只统计蓝队", buildBullBoard([findOwnerTeam(teams, "puuid-200-6", "")!]).entries.map((entry) => entry.player.teamId), [200, 200, 200, 200, 200])
check("找不到 owner 返回 null", findOwnerTeam(teams, "不存在的 puuid", ""), null)

// --- 3c. 档位类名必须和 CSS 里写死的类名对得上（历史 bug：拼成 bull-bull-bull 导致牛牛没样式） ---
const tierClasses = BULL_TIER_ORDER.map((tier) => bullTierClass(tier))
check("档位类名", tierClasses, ["bull-bull", "bull-high", "bull", "bull-none"])
check("档位类名互不重复", new Set(tierClasses).size, tierClasses.length)
check("牛牛不再拼成 bull-bull-bull", tierClasses.includes("bull-bull-bull"), false)

function declaredClasses(file: string, pattern: RegExp) {
  const css = readFileSync(path.resolve(file), "utf8")
  return [...new Set([...css.matchAll(pattern)].map((match) => match[1]))].sort()
}

// 注意：档位类名里 "bull" 本身不带连字符，所以后缀必须写成 (?:-[a-z]+)* 而非 -[a-z-]+
const chipClasses = declaredClasses("src/bullChip.css", /\.bull-chip\.(bull(?:-[a-z]+)*)/g)
check("bullChip.css 的档位类名与 tier 一致", chipClasses, [...tierClasses].sort())

const rowClasses = declaredClasses("src/components/BullRankBoard.vue", /\.bull-rank-row\.(bull(?:-[a-z]+)*)/g)
check("排行行的档位类名与 tier 一致", rowClasses, [...tierClasses].sort())

// --- 3d. 头像尺寸、列宽、导出舞台宽度必须同步 ---
// 历史坑一：改了头像尺寸忘同步列宽 → 头像被裁 / 名字被挤没。
// 历史坑二：导出舞台没跟着列宽加宽 → team-block 的 overflow-x 把右侧伤害列裁掉。
const readSource = (file: string) => readFileSync(path.resolve(file), "utf8")
const panelSource = readSource("src/components/MatchOverviewPanel.vue")
const toolbarSource = readSource("src/components/RecordWatchToolbar.vue")
const boardSource = readSource("src/components/BullRankBoard.vue")

// 牛气排行榜头像：22 → 44（用户定的 2 倍，88 太大）。
// 行高不写死、靠头像自己撑开，所以只锁「头像 = 第 2 列 = 2 倍」
const rankAvatarSize = Number(boardSource.match(/ChampionAvatar[^>]*:size="(\d+)"/)?.[1] ?? 0)
check("排行榜头像放大到原来的 2 倍", rankAvatarSize, 22 * 2)

const rankCols = boardSource.match(/\.bull-rank-row \{[^}]*?grid-template-columns:\s*([^;]+);/s)?.[1] ?? ""
const rankColNumbers = [...rankCols.matchAll(/(\d+)px/g)].map((match) => Number(match[1]))
check("排行榜第 2 列就是头像列", rankColNumbers[1], rankAvatarSize)
check("排行榜行高不写死（不会裁切）", /\.bull-rank-row \{[^}]*?height:/.test(boardSource), false)

// 详情行是另一处头像，用户明确说这块不改 —— 锁原值，避免以后又被顺手放大
check("详情行头像仍是 42", Number(panelSource.match(/ChampionAvatar[^>]*:size="(\d+)"/)?.[1] ?? 0), 42)
check("详情行行高仍是 48", Number(panelSource.match(/\.detail-row \{\s*height: (\d+)px/)?.[1] ?? 0), 48)

// 导出舞台：紧凑列 160 + 22 + 59 + 96 + 3 个 4px 间距 = 349，
// 再加面板 padding 12*2/边框 1*2 与 team-block padding 8*2/边框 1*2 共 44
const compactCols = panelSource.match(/\.panel-compact[^{]*\{\s*grid-template-columns:\s*([^;]+);/)?.[1] ?? ""
const compactNumbers = [...compactCols.matchAll(/(\d+)px/g)].map((match) => Number(match[1]))
check("紧凑列至少解析出 4 列", compactNumbers.length >= 4, true)
const compactContent = compactNumbers.reduce((sum, value) => sum + value, 0) + (compactNumbers.length - 1) * 4
const EXPORT_STAGE_CHROME = 44
const stageWidth = Number(toolbarSource.match(/\.export-stage \{[^}]*?width: (\d+)px/s)?.[1] ?? 0)
check("导出舞台装得下紧凑列", stageWidth >= compactContent + EXPORT_STAGE_CHROME, true)

// --- 4. 数据统计的自定义对局数量 ---
check("MIN_STATS_DEPTH 允许小样本", MIN_STATS_DEPTH <= 1, true)
check("自定义 36 场不被抬到 50", clampStatsDepth("36"), 36)
check("数字 36 原样通过", clampStatsDepth(36), 36)
check("自定义 7 场", clampStatsDepth("7"), 7)
check("0 兜到下限", clampStatsDepth("0"), MIN_STATS_DEPTH)
check("空输入回落默认值", clampStatsDepth(""), DEFAULT_STATS_DEPTH)
check("空输入回落调用方默认值", clampStatsDepth("", 120), 120)
check("非法输入回落默认值", clampStatsDepth("abc"), DEFAULT_STATS_DEPTH)
check("超上限夹到 3000", clampStatsDepth("99999"), MAX_STATS_DEPTH)
check("小数四舍五入", clampStatsDepth("24.6"), 25)
check("档位升序且都在范围内", STATS_DEPTH_OPTIONS.every((depth, index) => depth > MIN_STATS_DEPTH && depth <= MAX_STATS_DEPTH && (index === 0 || depth > STATS_DEPTH_OPTIONS[index - 1])), true)

// 统计口径就是「按 depth 截断」，验证 36 场只会算 36 条
const sixtyGames = Array.from({ length: 60 }, (_, index) => index)
check("口径截断到 36 场", sixtyGames.slice(0, clampStatsDepth("36")).length, 36)

console.log(failed === 0 ? "\n全部通过" : `\n失败 ${failed} 项`)
process.exitCode = failed === 0 ? 0 : 1
