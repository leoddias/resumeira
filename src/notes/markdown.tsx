import type { ReactNode } from 'react';

/**
 * Minimal Markdown renderer for model-generated summaries.
 *
 * Supports exactly what a summary needs: headings (`#`, `##`, `###`), bullet
 * lists (`-` or `*`), bold (`**text**`) and paragraphs. Deliberately
 * dependency-free.
 *
 * The source is untrusted — it is model output derived from a recorded
 * conversation — so this never touches `innerHTML`. Every piece of source
 * text ends up as a React text child, which React escapes on render; a
 * `<script>` tag or an `onerror="..."` attribute in the summary shows up as
 * visible text, never as markup.
 */
export default function Markdown({ source }: { source: string }) {
  return <>{parseBlocks(source)}</>;
}

interface HeadingBlock {
  type: 'heading';
  level: 1 | 2 | 3;
  text: string;
}
interface ListBlock {
  type: 'list';
  items: string[];
}
interface ParagraphBlock {
  type: 'paragraph';
  text: string;
}
type Block = HeadingBlock | ListBlock | ParagraphBlock;

const HEADING_RE = /^(#{1,3})\s+(.*)$/;
const LIST_ITEM_RE = /^[-*]\s+(.*)$/;
const BOLD_RE = /(\*\*[^*]+\*\*)/g;

function parseBlocks(source: string): ReactNode[] {
  const lines = source.replace(/\r\n/g, '\n').split('\n');
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let list: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length > 0) {
      blocks.push({ type: 'paragraph', text: paragraph.join(' ') });
      paragraph = [];
    }
  };
  const flushList = () => {
    if (list.length > 0) {
      blocks.push({ type: 'list', items: list });
      list = [];
    }
  };

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    const heading = HEADING_RE.exec(line);
    const listItem = LIST_ITEM_RE.exec(line);

    if (heading) {
      flushParagraph();
      flushList();
      const marker = heading[1] ?? '';
      blocks.push({
        type: 'heading',
        level: marker.length as 1 | 2 | 3,
        text: heading[2] ?? '',
      });
    } else if (listItem) {
      flushParagraph();
      list.push(listItem[1] ?? '');
    } else if (line.trim() === '') {
      flushParagraph();
      flushList();
    } else {
      flushList();
      paragraph.push(line.trim());
    }
  }
  flushParagraph();
  flushList();

  return blocks.map((block, index) => renderBlock(block, index));
}

function renderBlock(block: Block, key: number): ReactNode {
  switch (block.type) {
    case 'heading':
      return renderHeading(block, key);
    case 'list':
      return (
        <ul key={key}>
          {block.items.map((item, i) => (
            <li key={i}>{renderInline(item)}</li>
          ))}
        </ul>
      );
    case 'paragraph':
      return <p key={key}>{renderInline(block.text)}</p>;
  }
}

function renderHeading(block: HeadingBlock, key: number): ReactNode {
  const content = renderInline(block.text);
  switch (block.level) {
    case 1:
      return <h1 key={key}>{content}</h1>;
    case 2:
      return <h2 key={key}>{content}</h2>;
    case 3:
      return <h3 key={key}>{content}</h3>;
  }
}

/** Bold via `**text**`; everything else is plain text, escaped by React. */
function renderInline(text: string): ReactNode[] {
  return text.split(BOLD_RE).map((part, i) => {
    if (part.startsWith('**') && part.endsWith('**') && part.length >= 4) {
      return <strong key={i}>{part.slice(2, -2)}</strong>;
    }
    return part;
  });
}
