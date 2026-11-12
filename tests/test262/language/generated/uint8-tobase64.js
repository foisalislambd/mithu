/* generated language case */
var bytes = new Uint8Array([72, 105]);
assert.sameValue(bytes.toBase64(), "SGk=");
assert.sameValue(Uint8Array.fromBase64("SGk=")[1], 105);
