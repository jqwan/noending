// 同类图标共用基础轮廓，避免添加符号后边框和留白逐渐分化。
const chatOutline = "M3.5 2.5h11A1.5 1.5 0 0 1 16 4v7.5a1.5 1.5 0 0 1-1.5 1.5H7l-5 3V4a1.5 1.5 0 0 1 1.5-1.5Z";
const archiveOutline = "M3 3h12a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM3.5 6v7.5A1.5 1.5 0 0 0 5 15h8a1.5 1.5 0 0 0 1.5-1.5V6";
const folderTop = "M2 7V4.5A1.5 1.5 0 0 1 3.5 3H7l2 2h5.5A1.5 1.5 0 0 1 16 6.5";

const paths = {
  close: "M3 3l12 12M15 3L3 15",
  refresh: "M14.5 5.5A6.5 6.5 0 1 0 15.5 10M14.5 2v3.5H11",
  palette: "M9 2a7 7 0 1 0 0 14h1a2 2 0 0 0 1-4c-1-1 0-2 1-2h2c3 0 2-8-5-8ZM5 7h.1M8 5h.1M12 6h.1",
  database: "M3 5c0-4 12-4 12 0s-12 4-12 0Zm0 0v8c0 4 12 4 12 0V5M3 9c0 4 12 4 12 0",
  pin: "M6 2h6l-1 5 3 3v1H4v-1l3-3ZM9 11v5",
  trash: "M2 4h14M6 4V2h6v2M4 4l1 12h8l1-12M7 7v6M11 7v6",
  edit: "M11 3l4 4M3 15l1-5 8-8 4 4-8 8Z",
  plus: "M9 3v12M3 9h12",
  minus: "M4 9h10",
  chevronRight: "M6.75 4.5 11.25 9 6.75 13.5",
  chevronDown: "M4.5 6.75 9 11.25 13.5 6.75",
  // 收入托盘的箭头，用于把外部 Skill 收进 NoEnding。
  adopt: "M9 2v8M5.5 6.5L9 10l3.5-3.5M3 12v3h12v-3",
  arrowLeft: "M15 9H3m6-6L3 9l6 6",
  arrowRight: "M3 9h12M9 3l6 6-6 6",
  arrowUp: "M9 15V3m-5 5 5-5 5 5",
  arrowDown: "M9 3v12m-5-5 5 5 5-5",
  home: "M2 8l7-6 7 6v8h-5v-5H7v5H2Z",
  search: "M12 12l4 4M13 8A5 5 0 1 1 3 8a5 5 0 0 1 10 0",
  tasks: "M5 2h8M3.5 4.5h11M3.5 7h11A1.5 1.5 0 0 1 16 8.5v6a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 2 14.5v-6A1.5 1.5 0 0 1 3.5 7ZM5 10h5M5 13h8",
  tasksExpanded: "M3 5l1 1 2-2M8 5h7M3 10l1 1 2-2M8 10h7M8 15h7",
  folder: `${folderTop}v7a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 2 13.5V7Z`,
  folderOpen: `${folderTop}V7M2 7v8l2-7h12l-2 7H2`,
  project: "M9 2l7 4-7 4-7-4ZM2 6v6l7 4 7-4V6M9 10v6",
  projectOpen: "M9 6l-6-3.5-2 3.5 6 3.5 2-3.5 2 3.5 6-3.5-2-3.5-6 3.5ZM3 8v4l6 3.5 6-3.5V8M9 6v9.5",
  git: "M5 3a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3ZM5 6v6M5 12a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3ZM13 4.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3ZM5 11a4.5 4.5 0 0 1 4.5-4.5h2",
  chat: chatOutline,
  chatPlus: `${chatOutline}M9 5v5M6.5 7.5h5`,
  chatLines: `${chatOutline}M5.5 6h7M5.5 9.5h4`,
  spark: "M9 2l2 5 5 2-5 2-2 5-2-5-5-2 5-2Z",
  // 展开的书本，表示技能与技能库。
  skill: "M9 4.5C7 3 4.5 2.5 2 2.5v11c2.5 0 5 .5 7 2 2-1.5 4.5-2 7-2v-11c-2.5 0-5 .5-7 2ZM9 4.5v11M4.5 7l2.5.75M11 7.75l2.5-.75",
  chart: "M3 16V9M9 16V2M15 16V7",
  settings: "M2 4.5h14M2 13.5h14M6 2v5M12 11v5",
  play: "M5.5 3l10 6-10 6Z",
  more: "M4.5 9h.01M9 9h.01M13.5 9h.01",
  // 会话子页切换：概览（圆圈 i）与终端（提示符）。
  info: "M9 16A7 7 0 1 0 9 2a7 7 0 0 0 0 14ZM9 8v4M9 5.3h.01",
  terminal: "M3.5 3h11A1.5 1.5 0 0 1 16 4.5v9a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 2 13.5v-9A1.5 1.5 0 0 1 3.5 3ZM5 6l3 3-3 3M10 12h3",
  // 归档与永久删除使用不同图标。
  unarchive: `${archiveOutline}M9 13V8M6.5 10.5L9 8l2.5 2.5`,
  desktop: "M3.5 2.5h11A1.5 1.5 0 0 1 16 4v7a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 2 11V4a1.5 1.5 0 0 1 1.5-1.5ZM5.5 15.5h7M9 12.5v3",
  archive: `${archiveOutline}M7 9.5h4`,
  // 芯片轮廓，用于 Agent 导航与归类。
  agent: "M6 4.5h6A1.5 1.5 0 0 1 13.5 6v6A1.5 1.5 0 0 1 12 13.5H6A1.5 1.5 0 0 1 4.5 12V6A1.5 1.5 0 0 1 6 4.5ZM8 7.5h2a.5.5 0 0 1 .5.5v2a.5.5 0 0 1-.5.5H8a.5.5 0 0 1-.5-.5V8a.5.5 0 0 1 .5-.5ZM7 2v2.5M11 2v2.5M7 13.5V16M11 13.5V16M2 7h2.5M2 11h2.5M13.5 7H16M13.5 11H16",
  copy: "M6 4.5V3a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-1.5M4 6h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1Z",
  circle: "M16 9A7 7 0 1 1 2 9a7 7 0 0 1 14 0Z",
  circleCheck: "M16 9A7 7 0 1 1 2 9a7 7 0 0 1 14 0ZM5.5 9l2.3 2.3 4.7-4.7",
  check: "M3.5 9.5l3.5 3.5 7.5-7.5",
  grid: "M3.5 2.5h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1ZM11.5 2.5h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1ZM3.5 10.5h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1ZM11.5 10.5h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1Z",
  list: "M2 4h14M2 9h14M2 14h14",
  file: "M5 2h4.5l5 5v7.5A1.5 1.5 0 0 1 13 16H5a1.5 1.5 0 0 1-1.5-1.5v-11A1.5 1.5 0 0 1 5 2ZM9.5 2v5h5M6 10h6M6 13h4",
  filter: "M2 2.5h14L10.5 9v6.5l-3-2V9Z",
  sliders: "M2.5 6h13M6.5 3.5v5M2.5 12h13M11.5 9.5v5",
  bell: "M5 6a4 4 0 0 1 8 0v4l2 3H3l2-3V6ZM7 14.5a2 2 0 0 0 4 0",
  send: "M16 2 11 16 8 10 2 7 16 2ZM8 10l8-8",
  inbox: "M3.5 3.5h11l1.5 6v4a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1v-4L3.5 3.5ZM2 9.5h4l1.5 2h3l1.5-2h4",
  collapseAll: "M4 4.5l5 3.5 5-3.5M4 13.5l5-3.5 5 3.5",
  expandAll: "M4 8l5-3.5 5 3.5M4 10l5 3.5 5-3.5",
  puzzle: "M2 5h3V4a2 2 0 0 1 4 0v1h4v4h1a2 2 0 0 1 0 4h-1v3H2Z",
  // 电源符号：插件包启停按钮，停用时整钮置灰。
  power: "M13.77 4.98a6.75 6.75 0 1 1-9.54 0M9 1.5v7.5",
};

export type IconName = keyof typeof paths;

export default function Icon({ name, className }: { name: IconName; className?: string }) {
  return <svg className={`ui-icon${className ? ` ${className}` : ""}`} viewBox="0 0 18 18" aria-hidden="true"><path d={paths[name]} /></svg>;
}
