const paths = {
  close: "M4 4l10 10M14 4L4 14",
  refresh: "M14 6A6 6 0 1 0 15 11M14 2v4h-4",
  palette: "M9 2a7 7 0 1 0 0 14h1a2 2 0 0 0 1-4c-1-1 0-2 1-2h2c3 0 2-8-5-8ZM5 7h.1M8 5h.1M12 6h.1",
  database: "M3 5c0-4 12-4 12 0s-12 4-12 0Zm0 0v8c0 4 12 4 12 0V5M3 9c0 4 12 4 12 0",
  pin: "M6 2h6l-1 5 3 3v1H4v-1l3-3ZM9 11v5",
  trash: "M3 5h12M7 5V3h4v2M5 5l1 10h6l1-10M8 8v4M10 8v4",
  edit: "M11 3l4 4M3 15l1-5 8-8 4 4-8 8Z",
  plus: "M9 3v12M3 9h12",
  chevronRight: "M7 4l5 5-5 5",
  arrowDown: "M9 3.5v11M4.5 10l4.5 4.5 4.5-4.5",
  home: "M3 8l6-5 6 5v7H11v-4H7v4H3Z",
  search: "M12 12l4 4M13 8A5 5 0 1 1 3 8a5 5 0 0 1 10 0",
  tasks: "M3 5l1 1 2-2M8 5h7M3 10l1 1 2-2M8 10h7M8 15h7",
  folder: "M2 5V3h5l2 2h7v10H2Z",
  chat: "M3 3h12v9H8l-5 3Z",
  spark: "M9 2l2 5 5 2-5 2-2 5-2-5-5-2 5-2Z",
  chart: "M3.5 15V9M9 15V4M14.5 15V8",
  settings: "M3 5h12M3 13h12M6 3v4M12 11v4",
  play: "M6.5 4.5l7.5 4.5-7.5 4.5Z",
  more: "M4.5 9h.01M9 9h.01M13.5 9h.01",
  // 会话子页切换：概览（圆圈 i）与终端（提示符）。
  info: "M9 16A7 7 0 1 0 9 2a7 7 0 0 0 0 14ZM9 8v4M9 5.3h.01",
  terminal: "M2.5 4h13v10h-13ZM5 7l2.5 2L5 11M9.5 11.5h3.5",
  // 归档与永久删除使用不同图标。
  unarchive: "M3 4h12v3H3ZM4.5 7v7h9V7M9 12V8M7 10l2-2 2 2",
  archive: "M3 4h12v3H3ZM4.5 7v7h9V7M7.5 10.5h3",
  bot: "M9 2v2M4 6a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6ZM6.5 8.5h.01M11.5 8.5h.01M6.5 12h5",
  copy: "M6 4.5V3a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-1.5M4 6h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1Z",
  check: "M3.5 9.5l3.5 3.5 7.5-7.5",
  grid: "M3.5 3.5h4.5v4.5h-4.5ZM10 3.5h4.5v4.5h-4.5ZM3.5 10h4.5v4.5h-4.5ZM10 10h4.5v4.5h-4.5Z",
  list: "M3.5 4.5h11M3.5 9h11M3.5 13.5h11",
};

export default function Icon({ name }: { name: keyof typeof paths }) {
  return <svg className="ui-icon" viewBox="0 0 18 18" aria-hidden="true"><path d={paths[name]} /></svg>;
}
