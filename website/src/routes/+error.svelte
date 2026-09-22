<script lang="ts">
  import { page } from '$app/stores';
  import { ErrorPage } from 'svedocs/theme';
  import config from 'virtual:svedocs/config';
  import pages from 'virtual:svedocs/page-index';
  import tree from 'virtual:svedocs/tree';
  import loadSearch from 'virtual:svedocs/search-loader';
  import themeComponents from 'virtual:svedocs/theme-components';

  $: ErrorComponent = themeComponents.Error ?? ErrorPage;
</script>

<svelte:boundary>
  <svelte:component
    this={ErrorComponent}
    status={$page.status}
    message={$page.error?.message}
    error={$page.error}
    path={$page.url.pathname}
    {config}
    {pages}
    {tree}
    search={[]}
    {loadSearch}
    {themeComponents}
  />
  {#snippet failed(fallbackError)}
    <ErrorPage
      status={$page.status}
      message={$page.error?.message}
      error={fallbackError instanceof Error ? fallbackError : { message: String(fallbackError) }}
      path={$page.url.pathname}
      {config}
      {pages}
      {tree}
      search={[]}
      {loadSearch}
      {themeComponents}
    />
  {/snippet}
</svelte:boundary>
