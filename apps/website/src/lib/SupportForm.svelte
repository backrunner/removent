<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { supportSiteKey, supportTopics, type SupportLocale, type SupportTopic } from './support';
  import { supportCopy } from './support-copy';

  export let locale: SupportLocale = 'en';
  $: copy = supportCopy[locale];
  $: prefix = locale === 'zh' ? '/zh' : '';
  type ErrorCode = keyof typeof supportCopy.en.errors;
  type Turnstile = {
    render(container: HTMLElement, options: Record<string, unknown>): string;
    reset(id: string): void;
    remove(id: string): void;
  };
  const topicPaths = [
    'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6Zm0 0v6h6M12 11v4m0 3h.01',
    'm12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z',
    'M21 11.5a8.4 8.4 0 0 1-.9 3.8A8.5 8.5 0 0 1 12.5 20a8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8A8.5 8.5 0 0 1 8.7 3.9a8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8v.5Z',
    'M12 22s8-4 8-11V5l-8-3-8 3v6c0 7 8 11 8 11Zm-4-11 3 3 5-5'
  ];
  let email = '';
  let topic: SupportTopic = 'problem';
  let subject = '';
  let content = '';
  let appVersion = '';
  let osVersion = '';
  let deviceModel = '';
  let token = '';
  let busy = false;
  let errorCode: ErrorCode | '' = '';
  let verificationError: ErrorCode | '' = '';
  let ticketId = '';
  let successHeading: HTMLHeadingElement;
  let widget: HTMLDivElement;
  let widgetId: string | undefined;
  let turnstile: Turnstile | undefined;
  let syncVerification = () => false;
  let loadVerification = () => {};
  $: { locale; syncVerification(); }

  onMount(() => {
    let disposed = false;
    let renderedConfig = '';
    let generation = 0;
    const syncWidget = () => {
      // Never replace a challenge whose token is in flight.
      if (disposed || busy || ticketId || !turnstile || !widget.clientWidth) return false;
      const size = widget.clientWidth < 300 ? 'compact' : 'flexible';
      const language = locale === 'zh' ? 'zh-cn' : locale;
      const config = `${size}:${language}`;
      if (widgetId && renderedConfig === config) return false;
      const currentGeneration = ++generation;
      const isCurrent = () => !disposed && currentGeneration === generation;
      token = '';
      verificationError = '';
      try {
        if (widgetId) turnstile.remove(widgetId);
        widgetId = undefined;
        renderedConfig = config;
        widgetId = turnstile.render(widget, {
          sitekey: supportSiteKey, theme: 'auto', size, language, action: 'removent_support',
          callback: (value: string) => { if (isCurrent()) { token = value; verificationError = ''; } },
          'expired-callback': () => { if (isCurrent()) token = ''; },
          'error-callback': () => { if (isCurrent()) { token = ''; verificationError = 'captcha'; } },
          'timeout-callback': () => { if (isCurrent()) { token = ''; verificationError = 'timeout'; } }
        });
      } catch { verificationError = 'captcha'; }
      return true;
    };
    syncVerification = syncWidget;
    let resizeFrame = 0;
    const resizeObserver = new ResizeObserver(() => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(syncWidget);
    });
    resizeObserver.observe(widget);
    let script: HTMLScriptElement | undefined;
    const loadWidget = () => {
      if (disposed) return;
      verificationError = '';
      turnstile = (window as unknown as { turnstile?: Turnstile }).turnstile;
      if (turnstile) { syncWidget(); return; }
      script?.remove();
      script = document.createElement('script');
      script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
      script.async = true;
      script.defer = true;
      script.onload = () => {
        if (disposed) return;
        turnstile = (window as unknown as { turnstile: Turnstile }).turnstile;
        syncWidget();
      };
      script.onerror = () => { if (!disposed) verificationError = 'captcha'; };
      document.head.appendChild(script);
    };
    loadVerification = loadWidget;
    loadWidget();
    return () => {
      disposed = true;
      resizeObserver.disconnect();
      cancelAnimationFrame(resizeFrame);
      syncVerification = () => false;
      loadVerification = () => {};
      if (widgetId) turnstile?.remove(widgetId);
      script?.remove();
    };
  });

  function resetVerification() {
    token = '';
    if (syncVerification()) return;
    if (widgetId) { verificationError = ''; turnstile?.reset(widgetId); }
    else loadVerification();
  }

  async function submit() {
    if (busy || !token || errorCode === 'uncertain') return;
    busy = true;
    errorCode = '';
    const turnstileToken = token;
    token = '';
    try {
      const response = await fetch('/api/support', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, topic, locale, subject, content, appVersion, osVersion, deviceModel, turnstileToken }),
        signal: AbortSignal.timeout(95000)
      });
      const result = await response.json();
      if (!response.ok || typeof result.ticketId !== 'string' || !result.ticketId) {
        errorCode = typeof result.error === 'string' && Object.hasOwn(copy.errors, result.error) ? result.error as ErrorCode : 'uncertain';
      } else {
        ticketId = result.ticketId;
        email = subject = content = appVersion = osVersion = deviceModel = '';
        await tick();
        successHeading?.focus({ preventScroll: true });
        successHeading?.scrollIntoView({ block: 'center', behavior: 'instant' });
      }
    } catch { errorCode = 'uncertain'; }
    finally {
      busy = false;
      if (!ticketId) resetVerification();
    }
  }
</script>

<section class="support-panel" aria-label={copy.eyebrow}>
  {#if ticketId}
    <div class="support-success" role="status">
      <svg width="30" height="30" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="m7 12 3 3 7-7"/></svg>
      <h2 bind:this={successHeading} tabindex="-1">{copy.success}</h2>
      <p>{copy.successDetail}</p>
      <code>{ticketId}</code>
      <a href={prefix || '/'}>{copy.home} <span aria-hidden="true">↗</span></a>
    </div>
  {/if}
  <form on:submit|preventDefault={submit} hidden={Boolean(ticketId)} aria-busy={busy}>
    <fieldset class="support-fields" disabled={busy}>
      <fieldset class="support-topics">
        <legend>{copy.topic}</legend>
        <div class="support-topic-grid">
          {#each supportTopics as item, index}
            <label class="support-topic" class:selected={topic === item}>
              <input type="radio" name="topic" value={item} bind:group={topic} />
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={topicPaths[index]}/></svg>
              <span>{copy.topics[item]}</span>
            </label>
          {/each}
        </div>
      </fieldset>
      <div class="support-grid support-contact">
        <label>{copy.email}<input name="email" type="email" autocomplete="email" bind:value={email} required maxlength="254" placeholder="you@example.com" /></label>
        <label>{copy.subject}<input name="subject" bind:value={subject} required maxlength="200" placeholder={copy.subjectHint} /></label>
      </div>
      <label>{copy.details}<textarea name="content" bind:value={content} required maxlength="10000" rows="5" placeholder={copy.prompts[topic]} aria-describedby="support-privacy-hint"></textarea></label>
      <p class="support-hint" id="support-privacy-hint">{copy.privacyHint}</p>
      <details class="support-device">
        <summary><span class="support-device-title">{copy.device} <span>{copy.optional}</span></span><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg></summary>
        <div class="support-grid support-device-fields">
          <label>{copy.appVersion}<input name="appVersion" bind:value={appVersion} maxlength="80" placeholder={copy.appHint} /></label>
          <label>{copy.osVersion}<input name="osVersion" bind:value={osVersion} maxlength="80" placeholder={copy.macosHint} /></label>
          <label>{copy.deviceModel}<input name="deviceModel" bind:value={deviceModel} maxlength="80" placeholder={copy.formatHint} /></label>
        </div>
      </details>
    </fieldset>
    <div class="support-verification-row">
      <div class="support-verification" bind:this={widget}></div>
      <p class="support-verification-hint" aria-live="polite">{token ? copy.verified : busy ? copy.sending : copy.verifying}</p>
    </div>
    {#if verificationError}
      <div class="support-error" role="alert"><p>{copy.errors[verificationError]}</p><button type="button" class="support-retry" on:click={resetVerification} disabled={busy}>{copy.retry}</button></div>
    {/if}
    {#if errorCode}<p class="support-error" role="alert">{copy.errors[errorCode]}</p>{/if}
    <div class="support-submit">
      <p>{copy.consent} <a href={`${prefix}/privacy`}>{copy.privacyLink}</a></p>
      <button class="support-send" type="submit" disabled={busy || !token || errorCode === 'uncertain'}>{busy ? copy.sending : copy.send}<span aria-hidden="true">↗</span></button>
    </div>
    <noscript><p>{copy.noScript}</p></noscript>
  </form>
</section>

<style>
  .support-panel { width: 100%; margin-block: 24px 40px; padding: 24px; border: 1px solid var(--sd-line); border-radius: 16px; background: var(--rv-surface); }
  fieldset { min-width: 0; margin: 0; padding: 0; border: 0; }
  .support-fields { display: grid; gap: 22px; }
  .support-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
  .support-contact { grid-template-columns: minmax(0, .9fr) minmax(0, 1.1fr); }
  label, legend { color: var(--sd-ink); font-size: .875rem; font-weight: 600; }
  label { display: grid; min-width: 0; gap: 8px; }
  legend { padding: 0; margin-bottom: 10px; }
  .support-topic-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
  .support-topic { position: relative; display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-start; gap: 8px; min-height: 48px; padding: 12px 10px; border: 1px solid var(--sd-line); border-radius: 10px; background: var(--sd-panel); color: var(--sd-muted); cursor: pointer; transition: background 120ms, border-color 120ms; }
  .support-topic:hover { background: var(--rv-surface); border-color: var(--sd-line); }
  .support-topic.selected { background: color-mix(in srgb, var(--sd-accent) 8%, var(--sd-panel)); border-color: var(--sd-accent); color: var(--sd-accent); box-shadow: inset 0 0 0 1px var(--sd-accent); }
  .support-topic:focus-within { outline: 3px solid color-mix(in srgb, var(--sd-accent) 30%, transparent); outline-offset: 3px; }
  .support-topic input { position: absolute; opacity: 0; width: 1px; height: 1px; padding: 0; }
  .support-topic :global(svg) { flex: 0 0 auto; }
  input, textarea { width: 100%; min-width: 0; border: 1px solid var(--sd-line); border-radius: 8px; background: var(--sd-panel); color: var(--sd-ink); font: inherit; font-size: 16px; font-weight: 400; padding: 12px; scroll-margin-block: 100px 24px; transition: border-color 120ms, box-shadow 120ms; }
  input:hover, textarea:hover { border-color: var(--sd-muted); }
  input:focus-visible, textarea:focus-visible { outline: 2px solid color-mix(in srgb, var(--sd-accent) 40%, transparent); outline-offset: 2px; border-color: var(--sd-accent); }
  textarea { line-height: 1.6; resize: vertical; }
  input::placeholder, textarea::placeholder { color: var(--sd-muted); opacity: 1; }
  .support-hint { margin: -12px 0 0; color: var(--sd-muted); font-size: .78rem; line-height: 1.6; }
  .support-device { border-top: 1px solid var(--sd-line); border-bottom: 1px solid var(--sd-line); padding: 8px 0; }
  summary { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 44px; padding: 8px 0; list-style: none; cursor: pointer; color: var(--sd-muted); font-size: .875rem; font-weight: 600; }
  summary::-webkit-details-marker { display: none; }
  summary:focus-visible { outline: 2px solid var(--sd-accent); outline-offset: 5px; border-radius: 2px; }
  .support-device-title { display: flex; flex-wrap: wrap; gap: 8px; }
  .support-device-title > span { color: var(--sd-muted); font-weight: 400; font-size: .78rem; }
  summary :global(svg) { flex: 0 0 auto; color: var(--sd-muted); transition: transform 120ms; }
  details[open] summary :global(svg) { transform: rotate(180deg); }
  .support-device-fields { grid-template-columns: repeat(3, minmax(0, 1fr)); margin-top: 10px; margin-bottom: 12px; }
  .support-verification-row { display: flex; align-items: center; gap: 24px; margin-top: 20px; }
  .support-verification { flex: 1 0 300px; min-height: 65px; }
  .support-verification-hint { flex: 1; color: var(--sd-muted); font-size: .78rem; line-height: 1.6; margin: 0; }
  .support-submit { display: flex; align-items: center; gap: 24px; margin-top: 20px; }
  .support-submit p { flex: 1; max-width: 440px; margin: 0; color: var(--sd-muted); font-size: .78rem; line-height: 1.6; }
  .support-submit a { color: var(--sd-muted); text-underline-offset: 3px; }
  .support-send { flex: 0 0 auto; margin-left: auto; min-height: 46px; border: 0; border-radius: 10px; padding: 0 20px; box-shadow: none; cursor: pointer; }
  .support-send:disabled { opacity: .5; cursor: not-allowed; }
  .support-error { margin: 14px 0 0; padding: 12px; border: 1px solid color-mix(in srgb, #c78a2b 50%, transparent); border-radius: 8px; background: color-mix(in srgb, #c78a2b 7%, transparent); color: var(--sd-ink); font-size: .875rem; line-height: 1.6; }
  .support-error p { margin: 0; }
  .support-retry { display: flex; align-items: center; gap: 6px; min-height: 44px; border: 0; margin-top: 4px; padding: 8px 0; background: transparent; color: var(--sd-ink); text-decoration: underline; cursor: pointer; }
  .support-success { display: grid; justify-items: start; gap: 12px; padding-top: 8px; }
  .support-success :global(svg) { color: var(--sd-accent); }
  .support-success h2 { margin: 0; color: var(--sd-ink); font-size: 1.25rem; }
  .support-success p { margin: 0; color: var(--sd-muted); line-height: 1.6; }
  .support-success code { max-width: 100%; overflow-wrap: anywhere; color: var(--sd-ink); }
  .support-success a { display: inline-flex; align-items: center; gap: 5px; margin-top: 8px; font-size: .9rem; }
  @media (max-width: 600px) {
    .support-panel { padding: 16px; }
    .support-fields { gap: 20px; }
    .support-grid, .support-device-fields { grid-template-columns: 1fr; }
    .support-topic-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
    .support-topic { min-height: 82px; flex-direction: column; align-items: flex-start; justify-content: flex-start; gap: 8px; }
    .support-verification-row { display: block; }
    .support-verification { width: 100%; min-width: 0; }
    .support-verification-hint { margin-top: 8px; }
    .support-submit { flex-direction: column; align-items: stretch; gap: 16px; }
    .support-send { width: 100%; margin: 0; justify-content: center; }
  }

  .support-send { display: inline-flex; align-items: center; gap: 12px; background: var(--sd-accent); color: #fff; font: inherit; font-size: .9rem; font-weight: 600; }
  .support-send:hover:enabled { background: color-mix(in srgb, var(--sd-accent) 85%, black); }
  .support-send:focus-visible, .support-retry:focus-visible, .support-success a:focus-visible { outline: 3px solid var(--sd-accent); outline-offset: 4px; }
  .support-topic span { line-height: 1.5; }
  .support-topic svg { color: var(--sd-accent); }
  .support-success { padding-block: 12px 8px; }
  .support-success h2 { scroll-margin-top: 120px; }
  .support-success code { padding: 10px 14px; border: 1px solid var(--sd-line); border-radius: 8px; background: var(--rv-surface); }
  .support-success a { min-height: 44px; align-items: center; color: var(--sd-accent); }
</style>
