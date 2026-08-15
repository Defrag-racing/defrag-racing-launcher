/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

// `$t` is registered once on the app instance (see main.ts), so every template
// can call it without importing anything.
declare module "vue" {
  interface ComponentCustomProperties {
    $t: (key: string, params?: Record<string, string | number>) => string;
  }
}

export {};
