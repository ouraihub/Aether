// Markdown → sanitized HTML. Render it with
// `{@attach (node) => { node.innerHTML = html }}`, never `{@html}`
// (docs/04 §5).
import DOMPurify from 'dompurify'
import { marked } from 'marked'

/** Untrusted model and skill text: DOMPurify drops scripts, handlers and
 * URLs. */
export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(marked.parse(source, { async: false, gfm: true }))
}
