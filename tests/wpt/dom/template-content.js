
var t = document.createElement('template');
t.innerHTML = '<b id="x">Hi</b>';
if (!t.content || t.content.nodeType !== 11) throw new Error('fragment');
if (t.childNodes.length !== 0) throw new Error('host children');
var copy = t.content.cloneNode(true);
if (!copy || copy.nodeType !== 11) throw new Error('clone');
document.body.appendChild(copy);
if (!document.getElementById('x')) throw new Error('moved');
