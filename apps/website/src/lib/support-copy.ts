import type { SupportLocale } from './support';

const en = {
  eyebrow: 'Contact Removent support', topic: 'What can we help with?',
  topics: { problem: 'Report a problem', feature: 'Suggest a feature', question: 'Ask a question', privacy: 'Privacy request' },
  prompts: {
    problem: 'What were you doing? What happened, and what did you expect? Include the exact error if available.',
    feature: 'What would you like to do, and how would this help your workflow?',
    question: 'Tell us what you would like to know about Removent.',
    privacy: 'Describe your privacy question or request without including sensitive documents.'
  },
  email: 'Your email', subject: 'Subject', subjectHint: 'A short summary', details: 'How can we help?',
  privacyHint: 'Do not include passwords, pairing or connection codes, private screen contents, or Apple ID credentials.',
  device: 'App and device details', optional: 'Optional', appVersion: 'Removent version', osVersion: 'Operating system version', deviceModel: 'Device model',
  appHint: 'e.g. 0.1.3', macosHint: 'e.g. iOS 26', formatHint: 'e.g. iPhone, iPad Pro, Mac',
  verified: 'Security check complete.', verifying: 'Complete the security check to send your request.', sending: 'Sending…', send: 'Send request', retry: 'Retry verification',
  consent: 'Your email and message will be sent to our OnFire support system to handle your request.', privacyLink: 'Privacy policy',
  success: 'Request received', successDetail: 'Your request is in the Removent support queue. Keep this reference when you contact us again.',
  home: 'Back to Removent', noScript: 'Enable JavaScript to verify and send a request. The troubleshooting guides below remain available without JavaScript.',
  errors: {
    method: 'Reload the support page and try again.', origin: 'Please submit from removent.pwp.sh.',
    media: 'The request format is invalid. Reload the support page.', unavailable: 'Support is temporarily unavailable. Keep a copy of your message and try again later.',
    invalid: 'Check your email, subject, and message, then try again.', rate_limit: 'Too many attempts. Please wait a minute before trying again.',
    verification: 'The security check expired or failed. Please complete it again.',
    form: 'The support form has changed. Keep a copy of your message, then reload the page.',
    uncertain: 'We could not confirm whether your request was received. Keep a copy of your message and wait before sending another request to avoid duplicates.',
    captcha: 'The security check could not load. Retry verification or try again later. The troubleshooting guides below may help.', timeout: 'The security check timed out. Please retry.'
  }
};
type Copy = typeof en;
export const supportCopy: Record<SupportLocale, Copy> = {
  en,
  zh: {
    eyebrow: '联系 Removent 支持', topic: '你需要哪方面的帮助？',
    topics: { problem: '问题反馈', feature: '功能建议', question: '使用咨询', privacy: '隐私请求' },
    prompts: {
      problem: '请描述操作步骤、实际结果与预期结果。如果有错误提示，请提供完整文字。',
      feature: '你希望完成什么操作？这项功能会如何改善你的使用体验？',
      question: '请告诉我们你想了解的 Removent 使用问题。',
      privacy: '请描述你的隐私问题或请求，不要附带敏感文档。'
    },
    email: '你的邮箱', subject: '主题', subjectHint: '用一句话概括', details: '详细描述',
    privacyHint: '请勿包含密码、配对码、连接码、私人屏幕内容或 Apple ID 登录凭据。',
    device: '应用与设备信息', optional: '选填', appVersion: 'Removent 版本', osVersion: '系统版本', deviceModel: '设备型号',
    appHint: '例如 0.1.3', macosHint: '例如 iOS 26', formatHint: '例如 iPhone、iPad Pro、Mac',
    verified: '安全验证已完成。', verifying: '完成安全验证后即可发送。', sending: '正在发送…', send: '发送请求', retry: '重新验证',
    consent: '你的邮箱与消息将提交至我们的 OnFire 支持系统，用于处理本次请求。', privacyLink: '隐私政策',
    success: '请求已收到', successDetail: '你的请求已进入 Removent 支持队列。再次联系我们时，请提供以下工单编号。',
    home: '返回 Removent 首页', noScript: '请启用 JavaScript 以完成安全验证并发送请求。下方的故障排查指南无需 JavaScript 即可阅读。',
    errors: {
      method: '请重新加载支持页面后再试。', origin: '请从 removent.pwp.sh 提交。',
      media: '请求格式有误，请重新加载支持页面。', unavailable: '支持服务暂时不可用，请先保存消息副本，稍后再试。',
      invalid: '请检查邮箱、主题与详细描述后再试。', rate_limit: '尝试次数过多，请等待一分钟后再试。',
      verification: '安全验证已过期或未通过，请重新验证。',
      form: '支持表单已更新，请先保存消息副本，再重新加载页面。',
      uncertain: '暂时无法确认请求是否已收到。为避免重复提交，请保存消息副本，等待一段时间后再发送其他请求。',
      captcha: '安全验证未能加载，请重新验证或稍后再试。也可以先查看下方的故障排查指南。', timeout: '安全验证超时，请重试。'
    }
  }
};
