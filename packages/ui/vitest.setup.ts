/// <reference types="vitest/jsdom" />

// jsdom lacks matchMedia; svelte-sonner's Toaster reads it for the system theme.
if (typeof window !== 'undefined') {
  // Node 26 exposes Web Storage too; browser tests must use jsdom's origin-scoped storage.
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: jsdom.window.localStorage })
  window.matchMedia = (query: string): MediaQueryList =>
    Object.assign(new EventTarget(), {
      matches: false,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
    })
  // jsdom lacks scrollIntoView; bits-ui Command scrolls the highlighted item into view.
  Element.prototype.scrollIntoView = () => undefined
  // jsdom has no layout observer; report measured boxes when observed or explicitly resized.
  class TestResizeObserver implements ResizeObserver {
    readonly targets = new Set<Element>()
    constructor(readonly callback: ResizeObserverCallback) {}
    readonly report = () => {
      const entries = [...this.targets].map((target): ResizeObserverEntry => {
        const contentRect = target.getBoundingClientRect()
        const size = { inlineSize: contentRect.width, blockSize: contentRect.height }
        return { target, contentRect, borderBoxSize: [size], contentBoxSize: [size], devicePixelContentBoxSize: [size] }
      })
      if (entries.length) this.callback(entries, this)
    }
    observe(target: Element) {
      this.targets.add(target)
      window.addEventListener('resize', this.report)
      queueMicrotask(this.report)
    }
    unobserve(target: Element) {
      this.targets.delete(target)
      if (!this.targets.size) this.disconnect()
    }
    disconnect() {
      this.targets.clear()
      window.removeEventListener('resize', this.report)
    }
  }
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, value: TestResizeObserver })
  Object.defineProperty(window, 'ResizeObserver', { configurable: true, value: TestResizeObserver })
}
