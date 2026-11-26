import { toBlob } from "html-to-image"
import { copyPngToClipboard } from "./api"

type CopyElementOptions = {
  backgroundColor?: string
  filter?: (node: Node) => boolean
  pixelRatio?: number
}

export async function copyElementAsPng(
  element: HTMLElement,
  options: CopyElementOptions = {},
) {
  await document.fonts?.ready
  const blob = await toBlob(element, {
    backgroundColor: options.backgroundColor ?? "#f6faf9",
    cacheBust: true,
    pixelRatio: options.pixelRatio ?? 2,
    filter: options.filter,
  })

  if (!blob) throw new Error("图片生成失败")

  await writeImageBlobToClipboard(blob)

  return readImageSize(blob)
}

/** 渲染成 PNG 但只返回 base64，不写剪贴板 —— 给后台推送用。 */
export async function renderElementAsPngBase64(
  element: HTMLElement,
  options: CopyElementOptions = {},
) {
  await document.fonts?.ready
  const blob = await toBlob(element, {
    backgroundColor: options.backgroundColor ?? "#f6faf9",
    cacheBust: true,
    pixelRatio: options.pixelRatio ?? 2,
    filter: options.filter,
  })

  if (!blob) throw new Error("图片生成失败")

  const bytes = new Uint8Array(await blob.arrayBuffer())
  return {
    base64: bytesToBase64(bytes),
    size: await readImageSize(blob),
  }
}

function bytesToBase64(bytes: Uint8Array) {
  let binary = ""
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  return btoa(binary)
}

async function readImageSize(blob: Blob) {
  try {
    const url = URL.createObjectURL(blob)
    const img = new Image()
    await new Promise((resolve, reject) => {
      img.onload = resolve
      img.onerror = reject
      img.src = url
    })
    URL.revokeObjectURL(url)
    return {
      width: img.naturalWidth,
      height: img.naturalHeight,
    }
  } catch {
    return null
  }
}

async function writeImageBlobToClipboard(blob: Blob) {
  const bytes = Array.from(new Uint8Array(await blob.arrayBuffer()))
  try {
    await copyPngToClipboard(bytes)
    return
  } catch (backendError) {
    if (!navigator.clipboard || typeof ClipboardItem === "undefined") {
      throw backendError
    }

    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          [blob.type || "image/png"]: blob,
        }),
      ])
    } catch {
      throw backendError
    }
  }
}
