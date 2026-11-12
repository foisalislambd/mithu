/* test262-style language case: proxy.js */
const p = new Proxy({ a: 1 }, { get: (t, k) => t[k] + 1 }); assert.sameValue(p.a, 2);
