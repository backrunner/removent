<script lang="ts">
  import { SidebarTree } from 'svedocs/theme';
  import type { SvedocsTreeItem } from 'svedocs/core';
  import type { SvedocsThemeContext } from 'svedocs/theme/types';
  import Icon from '../Icon.svelte';

  let { items = [], currentPath, context }: {
    items: SvedocsTreeItem[]; currentPath: string; context: SvedocsThemeContext;
  } = $props();

  const groups = [
    { key: 'start', icon: 'book', slugs: ['', 'installation', 'permissions'] },
    { key: 'connect', icon: 'network', slugs: ['connecting', 'vnc', 'rdp'] },
    { key: 'operate', icon: 'server', slugs: ['hosting', 'relay'] },
    { key: 'reference', icon: 'layers', slugs: ['quality', 'security', 'building'] }
  ];
  // Group the framework's localized tree; it remains the authority for URLs,
  // titles and nested navigation. Unrecognized future pages remain visible.
  function slug(item: SvedocsTreeItem) {
    return (item.path ?? '').replace(/\/$/, '').replace(/^\/docs(?:\/zh)?(?:\/|$)/, '');
  }
  let other = $derived(items.filter(item => !groups.some(group => group.slugs.includes(slug(item)))));
</script>

<div class="rv-doc-navigation">
  {#each groups as group}
    {@const links = items.filter(item => group.slugs.includes(slug(item)))}
    {#if links.length}
      <div class="rv-nav-group">
        <p class="rv-nav-group-title"><Icon name={group.icon} size={14} />{context.t(`docs.group.${group.key}`)}</p>
        <SidebarTree items={links} {currentPath} />
      </div>
    {/if}
  {/each}
  {#if other.length}<SidebarTree items={other} {currentPath} />{/if}
</div>
