<script lang="ts">
  import type { SvedocsThemeContext } from 'svedocs/theme/types';
  import { resolveLocalizedHref } from 'svedocs/theme/headless';
  import Icon from './Icon.svelte';
  import ConnectionPreview from './ConnectionPreview.svelte';
  export let context: SvedocsThemeContext;
  $: t = context.t;
  const features = [
    { id: 'direct', icon: 'network', path: '/docs/connecting' },
    { id: 'clear', icon: 'monitor', path: '/docs/quality' },
    { id: 'trust', icon: 'shield', path: '/docs/security' }
  ];
  const guides = [
    { id: 'install', path: '/docs/installation' },
    { id: 'pair', path: '/docs/connecting' },
    { id: 'host', path: '/docs/hosting' }
  ];
</script>
<div class="rv-landing">
  <section class="rv-hero" aria-labelledby="hero-title">
    <div class="rv-hero-copy">
      <p class="rv-eyebrow"><span class="rv-blue-line" aria-hidden="true"></span>{t('hero.eyebrow')}</p>
      <h1 id="hero-title">{t('hero.line1')}<br /><span>{t('hero.line2')}</span></h1>
      <p class="rv-hero-description">{t('hero.description')}</p>
      <div class="rv-actions"><a class="rv-button rv-primary" href={resolveLocalizedHref('/download', context)}><Icon name="down" size={18} />{t('hero.download')}</a><a class="rv-button rv-secondary" href={resolveLocalizedHref('/docs', context)}>{t('hero.guide')}<Icon name="arrow" size={17} /></a></div>
      <p class="rv-requirements">{t('hero.requirements')}<span aria-hidden="true">/</span>{t('hero.source')}</p>
    </div>
    <div class="rv-product-stage"><div class="rv-stage-grid" aria-hidden="true"></div><ConnectionPreview {context} /></div>
  </section>
  <div class="rv-capabilities" aria-label={t('features.title')}>
    {#each [{key:'native',icon:'command'},{key:'direct',icon:'wifi'},{key:'private',icon:'shield'},{key:'compatible',icon:'layers'}] as item}<span><Icon name={item.icon} size={18} />{t('strip.' + item.key)}</span>{/each}
  </div>
  <section class="rv-section rv-features" aria-labelledby="features-title">
    <div class="rv-section-heading"><div><p class="rv-eyebrow">{t('features.eyebrow')}</p><h2 id="features-title">{t('features.title')}</h2></div><p>{t('features.body')}</p></div>
    <div class="rv-feature-grid">
      {#each features as feature}<article><div class="rv-feature-icon"><Icon name={feature.icon} size={25} /></div><h3>{t('features.' + feature.id + '.title')}</h3><p>{t('features.' + feature.id + '.body')}</p><a class="rv-text-link" href={resolveLocalizedHref(feature.path, context)}>{t('features.' + feature.id + '.link')}<Icon name="arrow" size={16} /></a></article>{/each}
    </div>
  </section>
  <section class="rv-native rv-section" aria-labelledby="native-title">
    <div class="rv-native-copy"><p class="rv-eyebrow">{t('native.eyebrow')}</p><h2 id="native-title">{t('native.title')}</h2><p>{t('native.body')}</p><ul>{#each [1,2,3] as n}<li><Icon name="check" size={16} />{t('native.point' + n)}</li>{/each}</ul><a class="rv-text-link" href={resolveLocalizedHref('/docs/permissions', context)}>{t('native.link')}<Icon name="arrow" size={16} /></a></div>
    <div class="rv-screenshot"><img class="rv-image-light" src="/images/settings-light.png" width="1920" height="1291" alt={t('native.alt')} loading="lazy" /><img class="rv-image-dark" src="/images/settings-dark.png" width="1920" height="1291" alt={t('native.alt')} loading="lazy" /></div>
  </section>
  <section class="rv-relay rv-section" aria-labelledby="relay-title">
    <div class="rv-relay-diagram">
      <div class="rv-network-map" aria-hidden="true"><span><Icon name="laptop" size={37} /></span><i></i><span class="rv-relay-node"><Icon name="server" size={32} /></span><i></i><span><Icon name="monitor" size={37} /></span></div>
      <div class="rv-network-labels"><span>{t('relay.controller')}</span><span>{t('relay.server')}</span><span>{t('relay.host')}</span></div>
      <div class="rv-transport-pills"><span>VPS / QUIC</span><span>Cloudflare / HTTPS</span></div>
      <p>{t('relay.caption')}</p>
    </div>
    <div class="rv-relay-copy"><p class="rv-eyebrow">{t('relay.eyebrow')}</p><h2 id="relay-title">{t('relay.title')}</h2><p>{t('relay.body')}</p><a class="rv-text-link" href={resolveLocalizedHref('/docs/relay', context)}>{t('relay.link')}<Icon name="arrow" size={16} /></a></div>
  </section>
  <section class="rv-guides rv-section" aria-labelledby="guides-title">
    <p class="rv-eyebrow">{t('guides.eyebrow')}</p><h2 id="guides-title">{t('guides.title')}</h2>
    <div class="rv-guide-list">{#each guides as guide, index}<a href={resolveLocalizedHref(guide.path, context)}><span class="rv-guide-number">0{index + 1}</span><div><h3>{t('guides.' + guide.id)}</h3><p>{t('guides.' + guide.id + '.body')}</p></div><Icon name="arrow" size={22} /></a>{/each}</div>
  </section>
  <section class="rv-closing"><img src="/app-icon.png" alt="" width="74" height="74" loading="lazy" /><h2>{t('closing.title')}</h2><a class="rv-button rv-primary" href={resolveLocalizedHref('/download', context)}>{t('closing.link')}<Icon name="down" size={18} /></a><p>{t('hero.requirements')}</p></section>
</div>
