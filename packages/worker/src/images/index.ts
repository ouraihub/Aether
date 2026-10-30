import type { ImagePart, ImageRef } from '@ouraihub/aether-contracts'
import {
  generateImage,
  ImageFailure,
  type ImageConnection,
  type ImageRequest,
  type ImageGeneration,
  type ImageLimits,
} from '@ouraihub/aether-core/images'
import type { OutboundHttp } from '@ouraihub/aether-core/ports'
import type { WorkerContext } from '../context.js'

export interface GeneratedImage extends ImageGeneration {
  readonly reference: ImageRef
  readonly persisted: boolean
}
export interface ImageServiceOptions {
  readonly outbound: OutboundHttp
  readonly limits?: ImageLimits
  /** Host stores kind=generated under context.actor and returns a controlled display URL. */
  readonly persist?: (image: ImagePart, context: WorkerContext) => Promise<ImageRef>
}

/** Explicit-input operation; runtime owns tool authorization, references and per-run limits. */
export class ImageService {
  private readonly options: ImageServiceOptions
  constructor(options: ImageServiceOptions) {
    this.options = options
  }

  async generate(connection: ImageConnection, input: ImageRequest, context: WorkerContext): Promise<GeneratedImage> {
    const result = await generateImage(this.options.outbound, connection, input, context.signal, this.options.limits)
    const { image } = result
    if (!this.options.persist) {
      return {
        ...result,
        reference: { mime: image.mime, url: `data:${image.mime};base64,${image.data}` },
        persisted: false,
      }
    }
    try {
      if (context.signal.aborted)
        throw new ImageFailure('cancelled', 'Image generated; persistence was cancelled', 'generated')
      const reference = await this.options.persist(image, context)
      return { ...result, reference, persisted: true }
    } catch {
      // Generation succeeded. Do not claim the provider did nothing or that saving succeeded.
      throw new ImageFailure('internal', 'Image generated but persistence could not be confirmed', 'generated', {
        field: 'images',
        reason: 'generated_save_failed',
      })
    }
  }
}
