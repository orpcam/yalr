// Verkleinert bilder vor dem upload (data-URL fuer image_url-content-parts).

/** Skaliert (w, h) so, dass die laengere seite <= maxSide ist; nie hochskalieren */
export function scaledSize(w: number, h: number, maxSide: number): { width: number; height: number } {
  if (w <= 0 || h <= 0) return { width: 0, height: 0 };
  const longest = Math.max(w, h);
  if (longest <= maxSide) return { width: w, height: h };
  const f = maxSide / longest;
  return { width: Math.max(1, Math.round(w * f)), height: Math.max(1, Math.round(h * f)) };
}

/** Liest eine bilddatei, skaliert sie und liefert eine JPEG-data-URL. GIFs werden abgelehnt. */
export async function resizeImageFile(file: File, maxSide = 1568, quality = 0.85): Promise<string> {
  if (!file.type.startsWith("image/")) throw new Error(`not an image: ${file.name}`);
  if (file.type === "image/gif") throw new Error("GIF images are not supported");
  const bmp = await createImageBitmap(file);
  try {
    const { width, height } = scaledSize(bmp.width, bmp.height, maxSide);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    // weisser hintergrund, damit transparenz im JPEG nicht schwarz wird
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, width, height);
    ctx.drawImage(bmp, 0, 0, width, height);
    return canvas.toDataURL("image/jpeg", quality);
  } finally {
    bmp.close();
  }
}
