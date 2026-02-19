/**
 * Rich markdown renderer for chat messages.
 *
 * Supports: headings, bold/italic, lists, tables, fenced code blocks with
 * syntax highlighting, inline code, links, blockquotes, horizontal rules,
 * and LaTeX math (inline $...$ and display $$...$$).
 */

import { Marked } from "marked";
import hljs from "highlight.js/lib/core";
import katex from "katex";

// Register commonly-used languages to keep the bundle lean
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import yaml from "highlight.js/lib/languages/yaml";
import xml from "highlight.js/lib/languages/xml";
import css from "highlight.js/lib/languages/css";
import sql from "highlight.js/lib/languages/sql";
import markdown from "highlight.js/lib/languages/markdown";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import cpp from "highlight.js/lib/languages/cpp";
import toml from "highlight.js/lib/languages/ini";

hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("js", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("ts", typescript);
hljs.registerLanguage("python", python);
hljs.registerLanguage("py", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("rs", rust);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("shell", bash);
hljs.registerLanguage("json", json);
hljs.registerLanguage("yaml", yaml);
hljs.registerLanguage("yml", yaml);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("css", css);
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("markdown", markdown);
hljs.registerLanguage("md", markdown);
hljs.registerLanguage("go", go);
hljs.registerLanguage("java", java);
hljs.registerLanguage("cpp", cpp);
hljs.registerLanguage("c", cpp);
hljs.registerLanguage("toml", toml);

/**
 * Pre-process LaTeX math before markdown parsing.
 *
 * Replaces $$ ... $$ (display) and $ ... $ (inline) with placeholders,
 * then restores them as rendered KaTeX HTML after markdown parsing.
 */
function extractMath(text: string): { processed: string; restore: (html: string) => string } {
  const placeholders: Map<string, string> = new Map();
  let counter = 0;

  function makePlaceholder(latex: string, displayMode: boolean): string {
    const key = `%%MATH_${counter++}%%`;
    try {
      placeholders.set(key, katex.renderToString(latex, { displayMode, throwOnError: false }));
    } catch {
      // Fallback: show raw LaTeX in a <code> block
      placeholders.set(key, `<code class="oa-math-error">${escapeHtml(latex)}</code>`);
    }
    return key;
  }

  // Display math: $$ ... $$ (can span multiple lines)
  let result = text.replace(/\$\$([\s\S]+?)\$\$/g, (_, latex) => makePlaceholder(latex.trim(), true));

  // Inline math: $ ... $ (single line, non-greedy, not preceded/followed by $)
  result = result.replace(/(?<!\$)\$(?!\$)(.+?)(?<!\$)\$(?!\$)/g, (_, latex) =>
    makePlaceholder(latex.trim(), false),
  );

  return {
    processed: result,
    restore(html: string): string {
      let out = html;
      for (const [key, rendered] of placeholders) {
        // The placeholder may have been HTML-escaped by marked
        out = out.replace(key, rendered);
        out = out.replace(escapeHtml(key), rendered);
      }
      return out;
    },
  };
}

function escapeHtml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// Create a shared Marked instance with highlight.js integration
const marked = new Marked({
  renderer: {
    code({ text, lang }: { text: string; lang?: string }) {
      const language = lang && hljs.getLanguage(lang) ? lang : undefined;
      const highlighted = language
        ? hljs.highlight(text, { language }).value
        : hljs.highlightAuto(text).value;
      const langLabel = lang ? `<span class="oa-code-lang">${escapeHtml(lang)}</span>` : "";
      return `<div class="oa-code-block">${langLabel}<pre><code class="hljs">${highlighted}</code></pre></div>`;
    },
  },
});

/**
 * Render a markdown string to safe HTML with syntax highlighting and math.
 */
export function renderMarkdown(text: string): string {
  if (!text) return "";

  // 1. Extract and replace math expressions with placeholders
  const { processed, restore } = extractMath(text);

  // 2. Parse markdown to HTML
  let html = marked.parse(processed) as string;

  // 3. Wrap tables with scrollable container for styling
  html = html.replace(/<table>/g, '<div class="oa-table-wrap"><table>');
  html = html.replace(/<\/table>/g, '</table></div>');

  // 4. Restore math placeholders with rendered KaTeX HTML
  return restore(html);
}
