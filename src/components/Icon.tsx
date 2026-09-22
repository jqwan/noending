const paths = {
  close: "M4 4l10 10M14 4L4 14",
  refresh: "M14 6A6 6 0 1 0 15 11M14 2v4h-4",
  palette: "M9 2a7 7 0 1 0 0 14h1a2 2 0 0 0 1-4c-1-1 0-2 1-2h2c3 0 2-8-5-8ZM5 7h.1M8 5h.1M12 6h.1",
  database: "M3 5c0-4 12-4 12 0s-12 4-12 0Zm0 0v8c0 4 12 4 12 0V5M3 9c0 4 12 4 12 0",
  pin: "M6 2h6l-1 5 3 3v1H4v-1l3-3ZM9 11v5",
  trash: "M3 5h12M7 5V3h4v2M5 5l1 10h6l1-10M8 8v4M10 8v4",
  edit: "M11 3l4 4M3 15l1-5 8-8 4 4-8 8Z",
  plus: "M9 3v12M3 9h12",
  filter: "M3 4h12l-5 6v4l-2 1v-5Z",
  home: "M3 8l6-5 6 5v7H11v-4H7v4H3Z",
  search: "M12 12l4 4M13 8A5 5 0 1 1 3 8a5 5 0 0 1 10 0",
  tasks: "M3 5l1 1 2-2M8 5h7M3 10l1 1 2-2M8 10h7M8 15h7",
  folder: "M2 5V3h5l2 2h7v10H2Z",
  chat: "M3 3h12v9H8l-5 3Z",
  spark: "M9 2l2 5 5 2-5 2-2 5-2-5-5-2 5-2Z",
  settings: "M3 5h12M3 13h12M6 3v4M12 11v4",
  play: "M6.5 4.5l7.5 4.5-7.5 4.5Z",
  more: "M4.5 9h.01M9 9h.01M13.5 9h.01",
  // 回收站（入口）用归档盒：垃圾桶留给「移入回收站」那个动作，两个含义不再共用图形。
  archive: "M3 4h12v3H3ZM4.5 7v7h9V7M7.5 10.5h3",
};

export default function Icon({ name }: { name: keyof typeof paths }) {
  return <svg className="ui-icon" viewBox="0 0 18 18" aria-hidden="true"><path d={paths[name]} /></svg>;
}
