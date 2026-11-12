/* generated language case */
assert.sameValue([1,2,3,4].group(n => n%2).length !== undefined || Object.keys([1,2,3,4].group(n => String(n%2))).length, 2);
