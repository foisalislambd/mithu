
var parent = document.createElement('div');
var child = document.createElement('span');
parent.appendChild(child);
if (child.parentNode !== parent) throw new Error('parent');
var next = document.createElement('div');
next.appendChild(child);
if (parent.firstChild !== null || child.parentNode !== next) throw new Error('move');
