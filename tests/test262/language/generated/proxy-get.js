/* generated language case */
assert.sameValue(new Proxy({a:1},{get:(t,k)=>t[k]+1}).a, 2);
