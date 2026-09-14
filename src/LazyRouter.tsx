import React, { Suspense } from "react";
import Router from "./app/Router";
import type { Route } from "./app/routes";

export default function LazyRouter(props: { route: Route; navigate: (r: Route) => void }) {
  return (
    <Suspense fallback={<div className="main">Loading…</div>}>
      <Router {...props} />
    </Suspense>
  );
}
