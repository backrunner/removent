<script lang="ts">
  import { DocsApp } from 'svedocs/theme';
  import themeComponents from 'virtual:svedocs/theme-components';
  import loadSearch from 'virtual:svedocs/search-loader';
  import Landing from './Landing.svelte';
  import type { PageData } from '../routes/$types';
  export let data: PageData;
</script>
<DocsApp page={data.page} pages={data.pages} tree={data.tree} search={data.search} config={data.config} content={data.content} layout={data.layout} {themeComponents} {loadSearch}>
  <svelte:fragment slot="landing" let:context><Landing {context} /></svelte:fragment>
</DocsApp>
