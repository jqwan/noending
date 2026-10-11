import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import SummaryMarkdown from "./SummaryMarkdown";

afterEach(cleanup);

it("collapses each top-level topic with its nested sections independently", () => {
  const { container } = render(<SummaryMarkdown>{`Intro

### First topic

**Lead description.**

- Keep **important** terms emphasized.

#### Nested heading

Nested content.

### Second topic

Second content.

\`\`\`md
# This is code, not a section
\`\`\``}</SummaryMarkdown>);
  const topics = container.querySelectorAll<HTMLDetailsElement>("details");
  expect(topics).toHaveLength(2);
  expect(Array.from(topics, topic => topic.open)).toEqual([true, true]);
  expect(topics[0].contains(screen.getByRole("heading", { name: "Nested heading" }))).toBe(true);
  expect(topics[0].contains(screen.getByText("Nested content."))).toBe(true);
  expect(topics[1].contains(screen.getByText("Second content."))).toBe(true);
  expect(screen.getByText("Lead description.").tagName).toBe("P");
  expect(screen.getByText("important").tagName).toBe("STRONG");
  const title = screen.getByRole("heading", { name: "First topic" }).closest("summary")!;
  fireEvent.click(title);
  expect(topics[0].open).toBe(false);
  expect(topics[1].open).toBe(true);
  fireEvent.click(title);
  expect(topics[0].open).toBe(true);
});

it("keeps summaries without headings readable and supports actual h1 sections", () => {
  const { container, rerender } = render(<SummaryMarkdown>{"Plain summary.\n\n- One point."}</SummaryMarkdown>);
  expect(container.querySelector("details")).toBeNull();
  expect(screen.getByText("One point.")).toBeTruthy();
  rerender(<SummaryMarkdown>{"# Topic\n\nA description.\n\n## Subtopic\n\nMore detail.\n\n# Another topic\n\nAnother detail."}</SummaryMarkdown>);
  const topics = container.querySelectorAll("details");
  expect(topics).toHaveLength(2);
  expect(topics[0].contains(screen.getByRole("heading", { name: "Subtopic" }))).toBe(true);
});
