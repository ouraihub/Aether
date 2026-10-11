import { describe, expect, it } from 'vitest'
import { renderMarkdown } from './markdown'

describe('renderMarkdown', () => {
  it('renders markdown', () => {
    expect(renderMarkdown('**bold** [link](https://example.com)')).toBe(
      '<p><strong>bold</strong> <a href="https://example.com">link</a></p>\n',
    )
  })

  it('strips script tags and their content', () => {
    const html = renderMarkdown('hi <script>alert(1)</script> there')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('alert(1)')
  })

  it('drops javascript: URLs in markdown links and raw HTML', () => {
    const html = renderMarkdown('[x](javascript:alert(1)) <a href="JaVaScRiPt:alert(2)">y</a>')
    expect(html.toLowerCase()).not.toContain('javascript:')
    expect(html).toContain('>x</a>')
  })

  it('drops inline event handlers', () => {
    expect(renderMarkdown('<img src="a.png" onerror="alert(1)">')).not.toContain('onerror')
  })
})
