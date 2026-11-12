/* test262-style language case: optional-chain.js */
const o = { a: 1 }; assert.sameValue(o?.a, 1); assert.sameValue(null?.a, undefined);
