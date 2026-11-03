/** 每個網站的規則:開啟時自動翻譯;YouTube 另有字幕翻譯、沒字幕時用聽譯 */
export interface SiteRule {
  auto: boolean;
  captions?: boolean;
  listen?: boolean;
}

export function isYouTube(host: string): boolean {
  return host === 'youtube.com' || host.endsWith('.youtube.com');
}

/** host 等於規則網域或是它的子網域才算命中;多個命中取最長(最具體)的 */
export function matchSite(host: string, sites: Record<string, SiteRule>): { key: string; rule: SiteRule } | undefined {
  let best: string | undefined;
  for (const key of Object.keys(sites)) {
    if ((host === key || host.endsWith(`.${key}`)) && (!best || key.length > best.length)) best = key;
  }
  return best === undefined ? undefined : { key: best, rule: sites[best]! };
}

/** 開頁時要自動做什麼;字幕只對 YouTube 有意義 */
export function startPlan(host: string, rule: SiteRule | undefined): { page: boolean; captions: boolean } {
  return { page: rule?.auto === true, captions: isYouTube(host) && rule?.captions === true };
}

/** popup 設定「這個網站」用的鍵:已有規則就沿用,YouTube 各子網域共用一條,其餘去掉 www. */
export function siteKey(host: string, sites: Record<string, SiteRule>): string {
  return matchSite(host, sites)?.key ?? (isYouTube(host) ? 'youtube.com' : host.replace(/^www\./, ''));
}

/** 改一條規則;全部關掉就整條刪除,清單只留有用的網站 */
export function updateSiteRule(sites: Record<string, SiteRule>, key: string, patch: Partial<SiteRule>): Record<string, SiteRule> {
  const r: SiteRule = { auto: false, ...sites[key], ...patch };
  const next = { ...sites, [key]: r };
  if (!r.auto && !r.captions && !r.listen) delete next[key];
  return next;
}
