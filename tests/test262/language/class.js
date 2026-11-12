/* test262-style language case: class.js */
class A { n() { return 1; } } class B extends A { n() { return super.n() + 1; } } assert.sameValue(new B().n(), 2);
