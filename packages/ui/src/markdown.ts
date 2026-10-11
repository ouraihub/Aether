// Markdown → sanitized HTML. Render it with `{@attach (node) => { node.innerHTML = html }}`, never `{@html}` (docs/04 §5).
import DOMPurify from 'dompurify'
import { marked } from 'marked'

/** Model and skill text is untrusted: DOMPurify drops scripts, event handlers and `javascript:` URLs. */
export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(marked.parse(source, { async: false, gfm: true }))
}
