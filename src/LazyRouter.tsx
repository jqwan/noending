import React, { Suspense } from "react";
import Router from "./Router";
import { Route } from "./App";

export default function LazyRouter(props: { route: Route; navigate: (r: Route) => void; refreshSidebar: () => void }) {
  return (
    <Suspense fallback={<div className="main">Loading…</div>}>
      <Router {...props} />
    </Suspense>
  );
}
