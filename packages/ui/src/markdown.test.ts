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
    const html = renderMarkdown(
      '[x](javascript:alert(1)) <a href="JaVaScRiPt:alert(2)">y</a>',
    )
    expect(html.toLowerCase()).not.toContain('javascript:')
    expect(html).toContain('>x</a>')
  })

  it('drops inline event handlers', () => {
    expect(
      renderMarkdown('<img src="a.png" onerror="alert(1)">'),
    ).not.toContain('onerror')
  })

  it(
    'retains structured replies while removing active HTML and unsafe links',
    () => {
      const html = renderMarkdown(
        `# Heading

        - item

        > quote

        \`\`\`js
        const text = '<script>not executable</script>'
        \`\`\`

        | Column | Value |
        | --- | --- |
        | A | B |

        <svg onload="alert(1)"><a href="javascript:alert(2)">bad</a></svg>
        <iframe src="https://example.com"></iframe>
        <a href="data:text/html;base64,PHNjcmlwdD4=">unsafe</a>
        [safe](https://example.com)`,
      )
      const body = document.createElement('div')
      body.innerHTML = html
      for (const tag of ['h1', 'ul', 'blockquote', 'pre', 'table'])
        expect(body.querySelector(tag)).not.toBeNull()
      expect(
        body.querySelector('script, iframe, [onload], [onerror]'),
      ).toBeNull()
      expect(body.querySelector('pre code')?.textContent).toContain(
        '<script>not executable</script>',
      )
      for (const link of body.querySelectorAll('a[href]'))
        expect(link.getAttribute('href')).toMatch(/^https:\/\//)
    },
  )
})
