import { createCn } from 'cn/config'
import { createTV } from 'tailwind-variants'
export type { VariantProps } from 'tailwind-variants'

const twMergeConfig = {
  extend: {
    theme: {
      text: ['kit-xs', 'kit-sm', 'kit-base', 'kit-lg'],
      spacing: ['kit-control', 'kit-control-sm'],
      radius: ['kit', 'kit-sm'],
    },
  },
}
export const cn = createCn(twMergeConfig)
export const tv = createTV({ twMergeConfig })

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChild<T> = T extends { child?: any } ? Omit<T, 'child'> : T
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChildren<T> = T extends { children?: any }
  ? Omit<T, 'children'>
  : T
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & {
  ref?: U | null
}
