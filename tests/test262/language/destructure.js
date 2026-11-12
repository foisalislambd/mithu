/* test262-style language case: destructure.js */
const { a, b = 3 } = { a: 1 }; assert.sameValue(a + b, 4);
