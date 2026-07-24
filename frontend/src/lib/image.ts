/** Largest input file we accept before downscaling (5 MB). */
export const AVATAR_MAX_INPUT_BYTES = 5 * 1024 * 1024;
/** Avatars are downscaled so their longest edge is at most this many pixels. */
export const AVATAR_MAX_DIM = 256;

/** Reasons {@link fileToAvatarDataUrl} can reject a file, for i18n mapping. */
export type AvatarError = 'not-image' | 'too-large' | 'decode-failed';

export class AvatarProcessingError extends Error {
  constructor(readonly reason: AvatarError) {
    super(reason);
    this.name = 'AvatarProcessingError';
  }
}

/**
 * Validates an uploaded image, downscales it to at most {@link AVATAR_MAX_DIM}
 * on its longest edge, and returns a compact WebP data URL. Bounding the
 * dimensions keeps avatars small enough to persist in localStorage regardless
 * of the original file size.
 */
export async function fileToAvatarDataUrl(file: File): Promise<string> {
  if (!file.type.startsWith('image/')) throw new AvatarProcessingError('not-image');
  if (file.size > AVATAR_MAX_INPUT_BYTES) throw new AvatarProcessingError('too-large');

  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new AvatarProcessingError('decode-failed');
  }

  try {
    const scale = Math.min(1, AVATAR_MAX_DIM / Math.max(bitmap.width, bitmap.height));
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));

    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new AvatarProcessingError('decode-failed');
    ctx.drawImage(bitmap, 0, 0, width, height);
    return canvas.toDataURL('image/webp', 0.9);
  } finally {
    bitmap.close();
  }
}
