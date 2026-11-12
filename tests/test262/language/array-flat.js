/* test262-style language case: array-flat.js */
assert.sameValue([1, [2, 3]].flat().join(','), '1,2,3');
