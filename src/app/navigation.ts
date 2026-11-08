import { prepareRouteModule } from "./Router";
import { prepareNavigationData, prepareSupportingData } from "./navigationData";
import type { Route } from "./routes";

/** 首屏数据与代码并行预备，仍有缓存时同步切换；后台刷新保留现有内容。 */
export function prepareNavigation(route: Route): Route | Promise<Route> {
  const module = prepareRouteModule(route);
  const data = prepareNavigationData(route);
  const supporting = prepareSupportingData(route);
  if (module || supporting || data instanceof Promise) {
    return Promise.all([module, data, supporting]).then(([, next]) => next);
  }
  return data;
}
