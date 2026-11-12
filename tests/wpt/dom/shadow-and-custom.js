
var host = document.createElement('div');
var root = host.attachShadow({ mode: 'open' });
if (root.mode !== 'open' || host.shadowRoot !== root) throw new Error('shadow');
class Box extends HTMLElement {}
customElements.define('wpt-box', Box);
var el = document.createElement('wpt-box');
if (!(el instanceof Box)) throw new Error('upgrade');
el.style.cssText = 'user-select: contain;';
if (el.style.getPropertyValue('user-select') !== 'contain') throw new Error('cssom');
