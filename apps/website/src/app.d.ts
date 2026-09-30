declare module 'virtual:svedocs/config' {
  import type { SvedocsResolvedConfig } from 'svedocs/core';
  const config: SvedocsResolvedConfig;
  export default config;
}

declare module 'virtual:svedocs/server-config' {
  import type { SvedocsResolvedConfig } from 'svedocs/core';
  const config: SvedocsResolvedConfig;
  export default config;
}

declare module 'virtual:svedocs/pages' {
  import type { SvedocsPage } from 'svedocs/core';
  const pages: SvedocsPage[];
  export default pages;
}

declare module 'virtual:svedocs/page-index' {
  import type { SvedocsPage } from 'svedocs/core';
  const pages: SvedocsPage[];
  export default pages;
}

declare module 'virtual:svedocs/page-loaders' {
  import type { SvedocsPage } from 'svedocs/core';
  const loaders: Record<string, () => Promise<{ default: SvedocsPage | undefined }>>;
  export default loaders;
}

declare module 'virtual:svedocs/tree' {
  import type { SvedocsTreeItem } from 'svedocs/core';
  const tree: SvedocsTreeItem[];
  export default tree;
}

declare module 'virtual:svedocs/search' {
  import type { SvedocsSearchRecord } from 'svedocs/core';
  const records: SvedocsSearchRecord[];
  export default records;
}

declare module 'virtual:svedocs/search-loader' {
  import type { SvedocsSearchRecord } from 'svedocs/core';
  const loadSearch: () => Promise<SvedocsSearchRecord[]>;
  export default loadSearch;
}

declare module 'virtual:svedocs/markdown' {
  const markdown: Record<string, string>;
  export default markdown;
}

declare module 'virtual:svedocs/components' {
  import type { Component } from 'svelte';
  const components: Record<string, Component>;
  export default components;
}

declare module 'virtual:svedocs/layouts' {
  import type { Component } from 'svelte';
  const layouts: Record<string, Component>;
  export default layouts;
}

declare module 'virtual:svedocs/theme-components' {
  import type { SvedocsThemeComponentMap } from 'svedocs/theme/types';
  const components: Partial<SvedocsThemeComponentMap>;
  export default components;
}

declare module 'virtual:svedocs/component-loaders' {
  import type { Component } from 'svelte';
  import type { SvedocsModuleLoader } from 'svedocs/routes';
  const loaders: Record<string, SvedocsModuleLoader<Component>>;
  export default loaders;
}

declare module 'virtual:svedocs/layout-loaders' {
  import type { Component } from 'svelte';
  import type { SvedocsModuleLoader } from 'svedocs/routes';
  import type { SvedocsCustomLayoutProps } from 'svedocs/theme/types';
  const loaders: Record<string, SvedocsModuleLoader<Component<SvedocsCustomLayoutProps>>>;
  export default loaders;
}
