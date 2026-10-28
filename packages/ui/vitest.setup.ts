// jsdom lacks matchMedia; svelte-sonner's Toaster reads it for the system theme.
if (typeof window !== 'undefined') {
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
}
