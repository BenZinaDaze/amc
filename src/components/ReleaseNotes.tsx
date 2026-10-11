import { Fragment, type ReactNode } from "react";

/// 更新说明的极简 Markdown 渲染：只认发版文档实际用到的语法子集——
/// `#`/`##`/`###` 标题、`-` 无序列表、``` 围栏代码块与段落；HTML 注释
/// （feat 提交确认标记）整行丢弃。输出 React 节点而非 HTML 字符串，
/// notes 内容不可信（latest.json 的 notes 字段不在签名范围内），不存在
/// 注入面。未识别的行按普通文本处理，任何输入都能渲染。
///
/// 语法子集来自 docs/releases/*.md 的既定结构；新发版文档若引入新语法
/// （粗体、链接等），需要先在这里扩展。

type Block =
  | { kind: "heading"; level: 1 | 2 | 3; text: string }
  | { kind: "list"; items: string[] }
  | { kind: "code"; text: string }
  | { kind: "paragraph"; text: string };

function parseBlocks(body: string): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let bullets: string[] | null = null;
  let code: string[] | null = null;
  const flushParagraph = () => {
    if (paragraph.length) {
      blocks.push({ kind: "paragraph", text: paragraph.join(" ") });
      paragraph = [];
    }
  };
  const flushBullets = () => {
    if (bullets) {
      blocks.push({ kind: "list", items: bullets });
      bullets = null;
    }
  };
  for (const line of body.split("\n")) {
    const text = line.trim();
    if (code !== null) {
      if (text.startsWith("```")) {
        blocks.push({ kind: "code", text: code.join("\n") });
        code = null;
      } else {
        code.push(line);
      }
      continue;
    }
    if (text.startsWith("```")) {
      flushParagraph();
      flushBullets();
      code = [];
      continue;
    }
    // 空行与 HTML 注释是纯标记，只负责切断当前段落/列表。
    if (!text || text.startsWith("<!--")) {
      flushParagraph();
      flushBullets();
      continue;
    }
    const heading = /^(#{1,3})\s+(.*)$/.exec(text);
    if (heading) {
      flushParagraph();
      flushBullets();
      blocks.push({
        kind: "heading",
        level: heading[1].length as 1 | 2 | 3,
        text: heading[2],
      });
      continue;
    }
    if (text.startsWith("- ")) {
      flushParagraph();
      bullets ??= [];
      bullets.push(text.slice(2));
      continue;
    }
    flushBullets();
    paragraph.push(text);
  }
  flushParagraph();
  flushBullets();
  // 未闭合的围栏按代码块收尾，内容不丢。
  if (code) blocks.push({ kind: "code", text: code.join("\n") });
  return blocks;
}

function renderBlock(block: Block, index: number): ReactNode {
  switch (block.kind) {
    case "heading": {
      // 弹窗标题是 h2,说明文档整体降一级:# → h3、## → h4、### → h5。
      const Tag = (["h3", "h4", "h5"] as const)[block.level - 1];
      return <Tag key={index}>{block.text}</Tag>;
    }
    case "list":
      return (
        <ul key={index}>
          {block.items.map((item, itemIndex) => (
            <li key={itemIndex}>{item}</li>
          ))}
        </ul>
      );
    case "code":
      return (
        <pre key={index}>
          <code>{block.text}</code>
        </pre>
      );
    case "paragraph":
      return <p key={index}>{block.text}</p>;
  }
}

/// 滚动容器（.update-notes）由调用方提供,这里只产出内容片段。
export function ReleaseNotes({ body }: { body: string }) {
  return <Fragment>{parseBlocks(body).map(renderBlock)}</Fragment>;
}
