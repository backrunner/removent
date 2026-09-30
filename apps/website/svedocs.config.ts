import { defineConfig } from 'svedocs/config';
import { en, zh } from './src/lib/messages.ts';

export default defineConfig({
  site: {
    name: 'Removent', title: 'Removent',
    description: 'Your other Mac. Right here. Native remote desktop with direct connections and a private relay you control.',
    url: process.env.SITE_URL || 'https://removent.pwp.sh'
  },
  build: { mode: 'static' },
  theme: {
    defaultMode: 'system', readingStyle: 'plain',
    palette: { accent: '#0865db', neutral: 'slate' },
    fonts: { sans: '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", sans-serif', mono: '"SFMono-Regular", Consolas, monospace' },
    radius: '10px',
    brand: { label: 'Removent', href: '/', logo: '/app-icon.png', mark: false },
    nav: [
      { label: 'Overview', labelKey: 'site.overview', href: '/' },
      { label: 'Documentation', labelKey: 'nav.documentation', href: '/docs' },
      { label: 'Download', labelKey: 'site.download', href: '/download' }
    ],
    footer: { text: 'Removent', links: [] },
    code: { copyButton: true, wrap: false }
  },
  search: { provider: 'local', scope: 'current' }, ai: false,
  agent: { enabled: true, markdown: true, llms: true, negotiation: false },
  source: { editBaseUrl: 'https://github.com/backrunner/removent/edit/main/website' },
  checks: { assets: true, translations: true },
  i18n: {
    defaultLocale: 'en', prefixDefaultLocale: false,
    locales: [
      { code: 'en', label: 'English', hreflang: 'en', ogLocale: 'en_US' },
      { code: 'zh', label: '简体中文', hreflang: 'zh-CN', ogLocale: 'zh_CN' }
    ],
    messages: { en, zh }
  }
});
