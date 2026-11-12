/* generated language case */
assert.sameValue(Error.isError(new Error("x")), true);
assert.sameValue(Error.isError({}), false);
var s = new SuppressedError(1, 2, "m");
assert.sameValue(s.error, 1);
assert.sameValue(s.suppressed, 2);
assert.sameValue(s instanceof Error, true);
