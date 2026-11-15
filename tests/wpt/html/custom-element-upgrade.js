class UpgradeBox extends HTMLElement {
  constructor() {
    super();
    this.order = "";
  }
  static get observedAttributes() { return ["title"]; }
  attributeChangedCallback(name, oldValue, value) {
    this.order += "attr:" + name + ":" + oldValue + ":" + value + ";";
  }
  connectedCallback() {
    this.order += "connected;";
  }
}
var el = document.createElement("wpt-upgrade");
el.setAttribute("title", "Clip");
el.setAttribute("id", "skip");
document.body.appendChild(el);
customElements.define("wpt-upgrade", UpgradeBox);
if (el.order !== "attr:title:null:Clip;connected;") throw new Error("upgrade " + el.order);
el.setAttribute("title", "Next");
if (el.order !== "attr:title:null:Clip;connected;attr:title:Clip:Next;") throw new Error("set " + el.order);
