<script lang="ts">
  import { onMount, tick, type Snippet } from 'svelte';

  let { children, currentPath = '', activeSelector = '[aria-current="page"]' }: {
    children: Snippet; currentPath?: string; activeSelector?: string;
  } = $props();
  let viewport: HTMLDivElement;
  let body: HTMLDivElement;
  let above = $state(false);
  let below = $state(false);

  function measure() {
    above = viewport.scrollTop > 1;
    below = viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop > 1;
  }

  function reveal(target: Element) {
    if (!target.getClientRects().length || !viewport.clientHeight) return;
    const bounds = viewport.getBoundingClientRect();
    const rect = target.getBoundingClientRect();
    const inset = 24;
    if (rect.top < bounds.top + inset) viewport.scrollTop += rect.top - bounds.top - inset;
    else if (rect.bottom > bounds.bottom - inset) viewport.scrollTop += rect.bottom - bounds.bottom + inset;
    if (viewport.scrollTop < inset) viewport.scrollTop = 0;
    else if (viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < inset) viewport.scrollTop = viewport.scrollHeight;
    measure();
  }

  $effect(() => {
    currentPath;
    let cancelled = false;
    void tick().then(() => {
      if (cancelled || !viewport) return;
      const active = viewport.querySelector(activeSelector);
      if (active) reveal(active);
      measure();
    });
    return () => { cancelled = true; };
  });

  onMount(() => {
    let lastActive: Element | null = null;
    let lastHeight = 0;
    function revealActive() {
      const active = viewport.querySelector(activeSelector);
      if (active && active !== lastActive) reveal(active);
      lastActive = active;
    }
    const selection = new MutationObserver(revealActive);
    selection.observe(body, { subtree: true, attributes: true, attributeFilter: ['class', 'aria-current'] });
    const observer = new ResizeObserver(() => {
      measure();
      if (lastHeight !== viewport.clientHeight) {
        lastHeight = viewport.clientHeight;
        lastActive = null;
        revealActive();
      }
      const focused = document.activeElement;
      if (focused && viewport.contains(focused)) reveal(focused);
    });
    observer.observe(viewport);
    observer.observe(body);
    measure();
    return () => { observer.disconnect(); selection.disconnect(); };
  });
</script>

<div class="rv-scroll-area" data-scroll-above={above} data-scroll-below={below}>
  <div class="rv-scroll-viewport" bind:this={viewport} onscroll={measure}
    onfocusin={event => { if (event.target instanceof Element) reveal(event.target); }}>
    <div class="rv-scroll-body" bind:this={body}>{@render children()}</div>
  </div>
</div>
