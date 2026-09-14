import AppShell from "./app/AppShell";

/** 应用入口只剩 Shell（实施方案 §4）：业务与骨架分离。 */
export default function App() {
  return <AppShell />;
}
