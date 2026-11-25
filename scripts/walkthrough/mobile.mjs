// Phone mode for the walkthrough drivers (scripts/*-walkthrough/client.mjs), shared so a fix to
// the check is made once. VF_MOBILE=1: a 390x844 touch phone, and each screenshot records what
// is wider than the screen and any tap target under 32 px (mobile walkthrough 2026-11-25).

export const MOBILE = process.env.VF_MOBILE === '1';
export const W = MOBILE ? 390 : 1280;
export const H = MOBILE ? 844 : 900;

/** Sets the page's size (and touch on a phone) through the DevTools protocol's `send`. */
export async function emulate(send) {
  await send('Emulation.setDeviceMetricsOverride', {
    width: W,
    height: H,
    deviceScaleFactor: MOBILE ? 2 : 1,
    mobile: MOBILE,
  });
  if (MOBILE) await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
}

// Runs in the page. The stand-in Stripe page (port 3398) is not the portal's and is skipped.
// An element is measured first; its ancestors are walked (for a clipping scroll box) and its
// text read only when it is flagged.
const CHECK = `(() => {
  if (!location.port || location.port === '3398') return null;
  const vw = document.documentElement.clientWidth;
  const issues = [];
  const label = (e) =>
    (e.innerText || e.value || e.getAttribute('aria-label') || '').trim().slice(0, 40).split(String.fromCharCode(10)).join(' ');
  const clipped = (e) => {
    for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
      const o = getComputedStyle(p).overflowX;
      if (o === 'auto' || o === 'scroll' || o === 'hidden') return true;
    }
    return false;
  };
  if (document.documentElement.scrollWidth > vw + 1)
    issues.push('page scrolls sideways: ' + document.documentElement.scrollWidth + 'px');
  for (const e of document.querySelectorAll('body *')) {
    const r = e.getBoundingClientRect();
    if (r.width === 0) continue;
    if ((r.right > vw + 1 || r.left < -1) && getComputedStyle(e).visibility !== 'hidden' && !clipped(e))
      issues.push('wide ' + e.tagName.toLowerCase() + ' ' + Math.round(r.left) + '..' + Math.round(r.right) + ' "' + label(e) + '"');
    if (['BUTTON', 'A', 'INPUT', 'SELECT'].includes(e.tagName) && r.height > 0 && r.height < 32 && !['checkbox', 'radio', 'hidden'].includes(e.type))
      issues.push('small target ' + Math.round(r.width) + 'x' + Math.round(r.height) + ' ' + e.tagName.toLowerCase() + ' "' + label(e) + '"');
  }
  return { path: location.pathname, issues: issues.slice(0, 40) };
})()`;

/** A recorder of layout problems: `check(name)` before each screenshot, `save(file)` at the
 *  end. Does nothing unless VF_MOBILE=1. `js` evaluates an expression in the page. */
export function layoutLog(js, writeFile) {
  const found = [];
  return {
    async check(shot) {
      if (!MOBILE) return;
      const r = await js(CHECK);
      if (r && r.issues.length) found.push({ shot, url: r.path, issues: r.issues });
    },
    async save(file) {
      if (MOBILE) await writeFile(file, JSON.stringify(found, null, 2));
    },
  };
}
