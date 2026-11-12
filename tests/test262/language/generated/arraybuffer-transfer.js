/* generated language case */
var b = new ArrayBuffer(4);
new Uint8Array(b)[0] = 7;
var n = b.transfer();
assert.sameValue(new Uint8Array(n)[0], 7);
assert.sameValue(n.byteLength, 4);
