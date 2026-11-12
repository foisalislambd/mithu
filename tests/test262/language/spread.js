/* test262-style language case: spread.js */
assert.sameValue([1, ...[2, 3]].join(','), '1,2,3');
