/* generated language case */
function* g(){ yield 1; yield 2 } assert.sameValue(Array.from(g()).join(''), '12');
