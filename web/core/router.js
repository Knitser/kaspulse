// router.js — hash router, ~40 lines. Routes:
//   #/            landing        #/feeds   full board
//   #/feed/{PAIR} feed detail    #/dev     API reference
//   #/docs        docs hub       unknown → landing
'use strict';

export function parseHash(h = location.hash) {
  const p = h.replace(/^#\/?/, '').replace(/\/+$/, '');
  if (p === '') return { route: 'landing', param: null };
  if (p === 'feeds') return { route: 'feeds', param: null };
  if (p === 'dev') return { route: 'dev', param: null };
  if (p === 'docs') return { route: 'docs', param: null };
  const m = p.match(/^feed\/([A-Za-z0-9._-]{1,32})$/);
  if (m) return { route: 'feed', param: m[1] };   // dash-form pair, case kept
  return { route: 'landing', param: null };
}

export function startRouter(render) {
  const go = () => render(parseHash());
  addEventListener('hashchange', go);
  go();   // render the current hash immediately (cold deep-links included)
}
