import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { MD_COMPONENTS } from "./SessionMessage";

// Only the AST fields used by this transform; all other Markdown nodes are
// preserved unchanged, including lists, nested headings and fenced code.
interface MarkdownNode {
  type: string;
  depth?: number;
  children?: MarkdownNode[];
  data?: { hName: string; hProperties?: { open?: boolean; className?: string[] } };
}

function summarySections() {
  return (tree: MarkdownNode) => {
    const nodes = tree.children ?? [];
    for (let i = 0; i < nodes.length - 1; i++) {
      const lead = nodes[i + 1];
      if (nodes[i].type === "heading" && lead.type === "paragraph"
          && lead.children?.length === 1 && lead.children[0].type === "strong") {
        lead.children = lead.children[0].children;
      }
    }
    const depths = nodes.filter(node => node.type === "heading").map(node => node.depth!);
    if (!depths.length) return;
    const topLevel = Math.min(...depths);
    const grouped: MarkdownNode[] = [];
    let section: MarkdownNode | undefined;
    for (const node of nodes) {
      if (node.type === "heading" && node.depth === topLevel) {
        section = {
          type: "summarySection",
          data: { hName: "details", hProperties: { open: true, className: ["summary-topic"] } },
          children: [{ type: "summaryHeading", data: { hName: "summary" }, children: [node] }],
        };
        grouped.push(section);
      } else if (section) {
        section.children!.push(node);
      } else {
        grouped.push(node);
      }
    }
    tree.children = grouped;
  };
}

export default function SummaryMarkdown({ children }: { children: string }) {
  return <ReactMarkdown remarkPlugins={[remarkGfm, summarySections]} components={MD_COMPONENTS}>
    {children}
  </ReactMarkdown>;
}
