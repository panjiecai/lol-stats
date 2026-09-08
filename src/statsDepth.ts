// 战绩拉取深度（局）的统一配置。
//
// 说明：后端逐页(每页 50)从 SGP / LCU 增量拉取并落 SQLite 缓存；把 MAX 调大意味着
// 首次可拉得更深，但也成比例增加请求页数与耗时（1000→3000 约 40→60 次分页请求）。
// 前端各组件统一从这里取上限/档位，避免在多处写死 1000 造成口径漂移。

/** 深度上限：单账号最多可加载的历史对局数。 */
export const MAX_STATS_DEPTH = 3000

/** 深度下限。 */
export const MIN_STATS_DEPTH = 50

/** 最近战绩列表可拉取的最大深度（与统计深度一致放开到 3000）。 */
export const MAX_RECENT_DEPTH = 3000

/** 深度默认值。 */
export const DEFAULT_STATS_DEPTH = 500

/** 深度下拉可选档位：1000 内按 100 步进，之后放稀，直到 3000。 */
export const STATS_DEPTH_OPTIONS = [
  50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1200, 1400, 1600, 1800, 2000, 2500, 3000,
]

/** 将任意输入夹到 [MIN_STATS_DEPTH, MAX_STATS_DEPTH]，非法输入回落 defaultDepth。 */
export function clampStatsDepth(value: unknown, defaultDepth: number = DEFAULT_STATS_DEPTH) {
  const numberValue = Number(value)
  if (!Number.isFinite(numberValue)) return defaultDepth
  return Math.min(MAX_STATS_DEPTH, Math.max(MIN_STATS_DEPTH, Math.round(numberValue)))
}
