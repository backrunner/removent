<script lang="ts">
  export let name = 'arrow';
  export let size = 20;
  const paths: Record<string, string> = {
    arrow: 'M5 12h14m-6-6 6 6-6 6', down: 'M12 4v16m-6-6 6 6 6-6',
    monitor: 'M4 3h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Zm4 18h8m-4-4v4',
    laptop: 'M5 4h14v12H5ZM2 20h20l-3-4H5Z',
    shield: 'M12 3 3 7v5c0 5 9 9 9 9s9-4 9-9V7Zm-4 9 3 3 5-6',
    check: 'm5 12 4 4L19 6',
    network: 'M8 7h8M7 12v5m10-5v5M5 17h4v4H5Zm10 0h4v4h-4ZM4 3h6v8H4Zm10 0h6v8h-6Z',
    layers: 'm12 3 10 5-10 5L2 8Zm-10 9 10 5 10-5M2 16l10 5 10-5',
    wifi: 'M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0m-11 4a6 6 0 0 1 8 0m-4 4h.01',
    server: 'M4 3h16v7H4Zm0 11h16v7H4ZM7 6.5h.01M7 17.5h.01',
    plus: 'M12 5v14M5 12h14', menu: 'M4 6h16M4 12h16M4 18h16',
    close: 'm6 6 12 12M6 18 18 6', search: 'M21 21l-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0Z',
    book: 'M12 5C8 2 3 3 3 3v16s5-1 9 2c4-3 9-2 9-2V3s-5-1-9 2Zm0 0v16',
    command: 'M8 8H5a3 3 0 1 1 3-3v14a3 3 0 1 1-3-3h14a3 3 0 1 1-3 3V5a3 3 0 1 1 3 3H8'
  };
</script>
<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.arrow} /></svg>
