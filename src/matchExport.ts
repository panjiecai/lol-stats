import { nextTick, ref } from "vue"
import { loadMatchDetail, loadSelfStats, searchPlayer } from "./api"
import { copyElementAsPng } from "./imageShare"
import type { Notify } from "./notifications"
import { rpaNotifyReady, runRpaNotify } from "./rpaNotify"
import type { MatchDetailResponse, RecentGame, SummonerInfo } from "./types"
import { riotId } from "./utils"

/**
 * 导出「某账号最新一局所在队伍的详情截图」的公共实现。
 *
 * 手动导出（RecordWatchToolbar）和多账号监控面板共用这套逻辑，
 * 所以这里不绑定任何组件状态：舞台 ref、截图内容由调用方渲染，导出流程由本模块跑。
 */

/** 导出前额外等待的渲染时间（等字体、图片解码完成再截图） */
export const EXPORT_RENDER_WAIT_MS = 250

/** 拉取最近战绩的深度：导出只要最新一局，但接口粒度就是这个 */
export const EXPORT_RECENT_DEPTH = 20

/**
 * 导出舞台宽度必须装得下 MatchOverviewPanel 的紧凑列，否则 team-block 的
 * overflow-x 会把右侧的伤害列裁掉。紧凑列 = 160 + 22 + 59 + 96 + 3 个 4px 间距 = 349，
 * 面板 padding 12/边框 1 + team-block padding 8/边框 1 共 44 → 393，取 420 留余量。
 * 该数值与 RecordWatchToolbar.vue 的 .export-stage 宽度由 check:logic 护栏锁定。
 */
export const EXPORT_STAGE_WIDTH = 420

/**
 * 全局导出锁：截图要独占剪贴板，同一时刻只允许一个导出任务在跑。
 * 手动导出、轮询导出、监控面板同时开的时候，靠它排掉并发。
 */
const exporting = ref(false)

export function matchExportBusy() {
  return exporting.value
}

export type MatchExportTarget = {
  /** 账号查询串（GameName#Tag 或 PUUID）；isSelf 为真时可省 */
  query?: string
  sgpServerId?: string
  isSelf?: boolean
  depth?: number
}

export type MatchExportOutcome =
  | { ok: true; message: string; gameId: number; ownerLabel: string }
  | { ok: false; message: string }

async function loadLatestGame(target: MatchExportTarget, forceRefresh: boolean) {
  const depth = target.depth ?? EXPORT_RECENT_DEPTH
  if (target.isSelf) return loadSelfStats(depth, forceRefresh, false)
  if (!target.query) throw new Error("缺少账号查询信息")
  return searchPlayer(target.query, depth, target.sgpServerId, forceRefresh, false)
}

/**
 * 截图进剪贴板后调用影刀把图片发到微信。返回一段追加到成功提示后面的文案；
 * 未开启或失败时返回空串（失败会自己弹一条错误提示，不影响导出结果）。
 *
 * 注意：**不要**在这里回头去「补回剪贴板」。影刀的微信流程会拿剪贴板把
 * 群名/联系人名粘进搜索框，我们若在这段时间把图片写回剪贴板，会把群名顶掉，
 * 直接搞坏它的搜索。剪贴板只能有一个侵占者。
 */
export async function dispatchRpaNotify(notify: Notify): Promise<string> {
  if (!rpaNotifyReady()) return ""
  try {
    await runRpaNotify()
    return "，已唤起影刀发送到微信"
  } catch (error) {
    notify({
      kind: "error",
      title: "影刀发送失败",
      message: error instanceof Error ? error.message : String(error),
      duration: 7000,
    })
    return ""
  }
}

export function useMatchExport() {
  const exportStage = ref<HTMLElement | null>(null)
  const exportGame = ref<RecentGame | null>(null)
  const exportDetail = ref<MatchDetailResponse | null>(null)
  const exportTeamId = ref<number | undefined>(undefined)
  const exportOwnerLabel = ref("")
  const exportOwnerPuuid = ref("")

  function clearStage() {
    exportGame.value = null
    exportDetail.value = null
    exportTeamId.value = undefined
  }

  /**
   * 模板里用 :ref="bindStage" 绑定导出舞台。
   * 用函数 ref 而不是 ref="名字" 字符串，避免依赖编译器对解构 ref 的推断。
   */
  function bindStage(element: unknown) {
    exportStage.value = (element as HTMLElement | null) || null
  }

  /**
   * 把该账号最新一局（或用 known 指定的一局）的己方详情截图写进剪贴板。
   *
   * known 是轮询已经拿到的结果，传进来可以省掉一次 SGP 请求 —— 轮询发现新对局的
   * 场景下，那份数据本来就是刚查出来的。
   */
  async function exportMatch(
    target: MatchExportTarget,
    options: { known?: { game?: RecentGame; summoner?: SummonerInfo } } = {},
  ): Promise<MatchExportOutcome> {
    if (exporting.value) return { ok: false, message: "已有导出任务正在进行，稍后再试" }
    exporting.value = true
    try {
      let game = options.known?.game
      let summoner = options.known?.summoner

      if (!game || !summoner) {
        const stats = await loadLatestGame(target, true)
        summoner = summoner || stats.summoner
        game = game
          ? stats.recentGames?.find((item) => item.gameId === game!.gameId) || game
          : stats.recentGames?.[0]
      }

      if (!summoner) return { ok: false, message: "没读到该账号的召唤师信息" }
      if (!game) return { ok: false, message: "请确认该账号有对局记录" }

      const detail = await loadMatchDetail(game.gameId, target.sgpServerId)
      const puuid = summoner.puuid?.trim().toLowerCase() || ""
      const label = riotId(summoner).trim().toLowerCase()
      const ownerTeam =
        detail.teams.find((team) =>
          team.players.some((player) => puuid && (player.puuid?.trim().toLowerCase() || "") === puuid),
        ) ||
        detail.teams.find((team) =>
          team.players.some(
            (player) =>
              label && `${player.gameName}#${player.tagLine}`.trim().toLowerCase() === label,
          ),
        )
      if (!ownerTeam) return { ok: false, message: "无法在该对局中找到此账号" }

      exportGame.value = game
      exportDetail.value = detail
      exportTeamId.value = ownerTeam.teamId
      exportOwnerLabel.value = riotId(summoner)
      exportOwnerPuuid.value = summoner.puuid || ""

      await nextTick()
      await document.fonts?.ready
      await new Promise((resolve) => window.setTimeout(resolve, EXPORT_RENDER_WAIT_MS))

      const stage = exportStage.value
      if (!stage) throw new Error("导出区域未渲染")

      const size = await copyElementAsPng(stage, {
        backgroundColor: "#f6faf9",
        pixelRatio: 2,
        filter: (node) =>
          !(node instanceof HTMLElement && node.classList.contains("overview-actions")),
      })

      // 截图完成立即隐藏覆盖层，避免遮挡界面
      clearStage()
      await nextTick()

      const message = size
        ? `己方详情已复制（${size.width}×${size.height}）`
        : "己方详情已复制"
      return { ok: true, message, gameId: game.gameId, ownerLabel: riotId(summoner) }
    } catch (error) {
      clearStage()
      return { ok: false, message: error instanceof Error ? error.message : String(error) }
    } finally {
      exporting.value = false
    }
  }

  return {
    exporting,
    exportStage,
    exportGame,
    exportDetail,
    exportTeamId,
    exportOwnerLabel,
    exportOwnerPuuid,
    exportMatch,
    clearStage,
    bindStage,
  }
}
