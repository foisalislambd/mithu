/* generated language case */
var o = {a: 1, b: [2]};
var c = structuredClone(o);
assert.sameValue(c.a, 1);
assert.sameValue(c.b[0], 2);
assert.sameValue(c === o, false);
