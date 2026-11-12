/* test262-style language case: nullish.js */
assert.sameValue(null ?? 'x', 'x'); assert.sameValue(0 ?? 'x', 0);
