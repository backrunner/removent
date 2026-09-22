<script lang="ts">
  import type { SvedocsThemeContext } from 'svedocs/theme/types';
  import { resolveLocalizedHref } from 'svedocs/theme/headless';
  import Icon from './Icon.svelte';
  export let context: SvedocsThemeContext;
  type Protocol = 'native' | 'vnc' | 'rdp';
  let selected: Protocol = 'native';
  const protocols: { id: Protocol; name: string; icon: string; path: string }[] = [
    { id: 'native', name: 'Removent', icon: 'wifi', path: '/docs/connecting' },
    { id: 'vnc', name: 'VNC', icon: 'monitor', path: '/docs/vnc' },
    { id: 'rdp', name: 'RDP', icon: 'layers', path: '/docs/rdp' }
  ];
  $: t = context.t;
  $: active = protocols.find(p => p.id === selected)!;
  function move(event: KeyboardEvent, index: number) {
    let next: number;
    if (event.key === 'ArrowDown' || event.key === 'ArrowRight') next = (index + 1) % protocols.length;
    else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') next = (index + 2) % protocols.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = protocols.length - 1;
    else return;
    event.preventDefault();
    selected = protocols[next].id;
    document.getElementById('protocol-' + selected)?.focus();
  }
</script>
<figure class="rv-preview" aria-label={t('preview.label')}>
  <div class="rv-window">
    <div class="rv-window-bar">
      <div class="rv-traffic-lights" aria-hidden="true"><i></i><i></i><i></i></div>
      <span>Removent</span>
      <span class="rv-preview-tag">{t('preview.note')}</span>
    </div>
    <div class="rv-window-body">
      <div class="rv-window-sidebar">
        <p class="rv-window-label">{t('preview.devices')}</p>
        <div class="rv-protocols" role="tablist" aria-label={t('preview.devices')}>
          {#each protocols as protocol, index}
            <button type="button" role="tab" id={'protocol-' + protocol.id} aria-selected={selected === protocol.id} aria-controls="protocol-panel" tabindex={selected === protocol.id ? 0 : -1} on:click={() => selected = protocol.id} on:keydown={(e) => move(e, index)}>
              <span class="rv-device-icon"><Icon name={protocol.icon} size={21} /></span>
              <span><strong>{protocol.name}</strong><small>{t('preview.' + protocol.id)}</small></span>
              <span class="rv-protocol-arrow" aria-hidden="true">›</span>
            </button>
          {/each}
        </div>
        <div class="rv-local-device"><Icon name="laptop" /><div><strong>{t('preview.thisMac')}</strong><small>{t('preview.local')}</small></div></div>
      </div>
      <div class="rv-connection-detail" id="protocol-panel" role="tabpanel" aria-labelledby={'protocol-' + selected} tabindex="0">
        <div class="rv-connection-heading"><span class="rv-large-icon"><Icon name={active.icon} size={29} /></span><span class="rv-protocol-badge">{active.name}</span></div>
        <h2>{t('preview.title.' + selected)}</h2>
        <p>{t('preview.body.' + selected)}</p>
        <dl>{#each ['host', 'transport', 'security'] as field}<div><dt>{t('preview.' + field)}</dt><dd class:rv-mono={field === 'host'}>{t('preview.' + selected + '.' + field)}</dd></div>{/each}</dl>
        <a class="rv-preview-link" href={resolveLocalizedHref(active.path, context)}>{t('preview.link')}<Icon name="arrow" size={16} /></a>
      </div>
    </div>
    <div class="rv-window-status"><span><Icon name="shield" size={13} />{t(selected === 'native' ? 'preview.encrypted' : 'preview.' + selected + '.security')}</span><span>RVP · VNC · RDP</span></div>
  </div>
  <figcaption>{t('preview.caption')}</figcaption>
</figure>
