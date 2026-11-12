/* test262-style language case: for-of.js */
let s = 0; for (const n of [1, 2, 3]) s += n; assert.sameValue(s, 6);
