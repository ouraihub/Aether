import type { KitI18n } from '../i18n'
import type { KitNotify } from '../context'
import type { ImagePart } from '../ports'

export type AttachedImage = ImagePart & { preview: string }

export function hasImages(files: FileList | null | undefined): boolean {
  return [...(files ?? [])].some((file) => file.type.startsWith('image/'))
}

function readBase64(file: File): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>()
  const reader = new FileReader()
  reader.onload = () => resolve(String(reader.result).slice(String(reader.result).indexOf(',') + 1))
  reader.onerror = () => reject(reader.error ?? new Error('read failed'))
  reader.readAsDataURL(file)
  return promise
}

/** Owns attachment reads and previews; the panel transfers images to run/draft recovery. */
export class ChatAttachments {
  images = $state<AttachedImage[]>([])
  reading = $state(0)

  constructor(
    private readonly canVision: () => boolean,
    private readonly notify: KitNotify,
    private readonly i18n: KitI18n,
  ) {}

  remove = (index: number): void => {
    const [image] = this.images.splice(index, 1)
    if (image) URL.revokeObjectURL(image.preview)
  }

  attach = async (files: FileList | File[] | null | undefined): Promise<void> => {
    const picked = [...(files ?? [])].filter((file) => file.type.startsWith('image/'))
    if (picked.length === 0) return
    const { t } = this.i18n
    if (!this.canVision()) this.notify.toast('error', t('chat.visionUnsupported'))
    else if (
      picked.some(
        (file) => !['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(file.type),
      )
    )
      this.notify.toast('error', t('chat.imageType'))
    else if (picked.some((file) => file.size > 4 * 1024 * 1024))
      this.notify.toast('error', t('chat.imageSize'))
    else if (this.images.length + picked.length > 4)
      this.notify.toast('error', t('chat.imageLimit', { max: 4 }))
    else {
      this.reading++
      try {
        const read = await Promise.all(
          picked.map(async (file) => ({ mime: file.type, data: await readBase64(file), file })),
        )
        if (this.images.length + read.length > 4)
          this.notify.toast('error', t('chat.imageLimit', { max: 4 }))
        else
          this.images.push(
            ...read.map(({ mime, data, file }) => ({
              mime,
              data,
              preview: URL.createObjectURL(file),
            })),
          )
      } catch {
        this.notify.toast('error', t('errors.internal'))
      } finally {
        this.reading--
      }
    }
  }
}
