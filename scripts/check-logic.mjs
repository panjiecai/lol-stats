/**
 * 纯前端逻辑自检（牛牛牌型/牛气排名、数据统计深度口径）。
 * 前端目前没有测试框架，这里用 esbuild 把 TS 逻辑打成临时 ESM 再跑断言，
 * 产物写到 node_modules/.cache，不污染仓库。`npm run check:logic` 直接调用。
 */
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const rootDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const esbuildBin = path.join(rootDir, "node_modules", "esbuild", "bin", "esbuild")
const entryFile = path.join(rootDir, "scripts", "check-logic.ts")
// 放在 node_modules/.cache 下：既被 git 忽略，又能正常解析 vue 依赖
const outDir = path.join(rootDir, "node_modules", ".cache")
const outFile = path.join(outDir, `logic-check-${process.pid}.mjs`)

if (!existsSync(esbuildBin)) {
  console.error("[check:logic] 找不到 esbuild，请先执行 npm install")
  process.exit(1)
}

mkdirSync(outDir, { recursive: true })

// localStorage 兜底：bullDisplay 在模块加载时就会读取开关默认值
const localStorageStub = "globalThis.localStorage={getItem:()=>null,setItem:()=>{},removeItem:()=>{}};"

function run(args) {
  const result = spawnSync(process.execPath, args, { cwd: rootDir, stdio: "inherit" })
  if (result.status !== 0) process.exit(result.status ?? 1)
}

run([
  esbuildBin,
  entryFile,
  "--bundle",
  "--platform=node",
  "--format=esm",
  "--external:vue",
  `--banner:js=${localStorageStub}`,
  `--outfile=${outFile}`,
  "--log-level=warning",
])
run([outFile])
